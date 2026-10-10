// Release builds on Windows are a GUI program: no console window behind the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod agent;
mod db;
mod imap;
mod mcp;
mod mime;
mod secrets;
mod sync;
mod facts;
mod trust;

use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use sync::App;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

const DEFAULT_PORT: u16 = 47821;

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, self.1).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError(StatusCode::BAD_GATEWAY, format!("{e:#}"))
    }
}

type Api<T> = Result<T, ApiError>;

fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "not found".into())
}

/// Run blocking work (SQLite, IMAP) off the async threads.
async fn blocking<T, F>(f: F) -> Api<T>
where
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(Into::into)
}

// ---- static UI -----------------------------------------------------------
fn asset(name: &str, built_in: &'static str) -> String {
    // UNIBOX_UI_DIR lets the UI be edited without rebuilding.
    std::env::var("UNIBOX_UI_DIR")
        .ok()
        .and_then(|d| std::fs::read_to_string(PathBuf::from(d).join(name)).ok())
        .unwrap_or_else(|| built_in.to_string())
}

/// Bump when the way messages are rendered changes, so cached copies in the window are dropped.
const RENDER_VERSION: &str = "11";

async fn index() -> impl IntoResponse {
    let html = asset("index.html", include_str!("../ui/index.html")).replace("__BUILD__", RENDER_VERSION);
    ([(header::CACHE_CONTROL, "no-cache")], Html(html))
}
async fn app_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")],
        asset("app.js", include_str!("../ui/app.js")),
    )
}
async fn style_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")],
        asset("style.css", include_str!("../ui/style.css")),
    )
}

// ---- accounts ------------------------------------------------------------
async fn list_accounts(State(app): State<Arc<App>>) -> Api<Json<Vec<db::Account>>> {
    Ok(Json(blocking(move || app.db.accounts()).await?))
}

#[derive(Deserialize)]
struct NewAccount {
    email: String,
    password: String,
}

async fn add_account(State(app): State<Arc<App>>, Json(body): Json<NewAccount>) -> Api<Json<serde_json::Value>> {
    let email = body.email.trim().to_lowercase();
    let password: String = body.password.chars().filter(|c| !c.is_whitespace()).collect();
    if !email.contains('@') || password.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "Enter an email address and an app password.".into()));
    }
    let id = blocking(move || {
        app.verify_login(&email, &password)?;
        secrets::set(&app.dir, &email, &password)?;
        let id = match app.db.accounts()?.into_iter().find(|a| a.email == email) {
            Some(existing) => existing.id, // re-adding just replaces the saved password
            None => app.db.add_account(&email)?,
        };
        app.sync_soon();
        Ok(id)
    })
    .await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

#[derive(Deserialize)]
struct AccountOrder {
    ids: Vec<i64>,
}

async fn order_accounts(State(app): State<Arc<App>>, Json(body): Json<AccountOrder>) -> Api<StatusCode> {
    blocking(move || app.db.set_account_order(&body.ids)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_account(State(app): State<Arc<App>>, Path(id): Path<i64>) -> Api<StatusCode> {
    blocking(move || {
        if let Some(a) = app.db.account(id)? {
            let _ = secrets::delete(&app.dir, &a.email);
            app.db.remove_account(id)?;
            app.emit("changed", &a.email);
        }
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- reading -------------------------------------------------------------
#[derive(Deserialize)]
struct ThreadQuery {
    account: Option<i64>,
    /// Show the set of conversations the assistant picked instead of the inbox.
    #[serde(default)]
    collection: bool,
    #[serde(default)]
    q: String,
    #[serde(default)]
    unread: bool,
    /// Comma-separated "account:thread" keys to keep listed while `unread` is on.
    #[serde(default)]
    keep: String,
    /// "people" for conversations a person wrote in, "automated" for those only systems sent.
    #[serde(default)]
    kind: String,
    /// List everything in All Mail, not only what is in the inbox.
    #[serde(default)]
    all: bool,
}

/// The list goes out as one array per conversation rather than an object, in the order of
/// `THREAD_COLUMNS` in the UI: with All Mail it can be a hundred thousand rows, and the
/// field names would be most of it. `senders` is left empty when it is just the sender.
async fn list_threads(State(app): State<Arc<App>>, Query(q): Query<ThreadQuery>) -> Api<Json<Vec<ThreadCells>>> {
    let rows = blocking(move || {
        if q.collection {
            let keys = app.agent.collection.lock().unwrap().as_ref().map(|c| c.keys.clone()).unwrap_or_default();
            return app.db.threads_by_keys(&keys);
        }
        let keep: Vec<&str> = q.keep.split(',').filter(|k| !k.is_empty()).collect();
        app.db.threads(q.account, q.q.trim(), q.unread, &q.kind, &serde_json::to_string(&keep)?, q.all)
    })
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|t| {
                let alone = &t.senders == if t.from_name.is_empty() { &t.from_addr } else { &t.from_name };
                (
                    t.account_id, t.thrid, t.date, t.subject, t.snippet, if alone { String::new() } else { t.senders }, t.count,
                    t.unread, t.starred as u8, t.has_attach as u8, t.from_name, t.from_addr, t.in_inbox as u8, t.people as u8,
                )
            })
            .collect(),
    ))
}

/// One conversation of the list, as the array it is sent as.
type ThreadCells = (i64, String, i64, String, String, String, i64, i64, u8, u8, String, String, u8, u8);

// ---- the assistant ---------------------------------------------------------
async fn agent_view(State(app): State<Arc<App>>) -> Api<Json<serde_json::Value>> {
    Ok(Json(blocking(move || app.agent_view()).await?))
}

#[derive(Deserialize)]
struct AgentMessage {
    text: String,
    /// What the window is showing right now, so "this email" and "these" mean something.
    #[serde(default)]
    state: serde_json::Value,
}

fn bad_request(e: anyhow::Error) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, format!("{e:#}"))
}

async fn agent_send(State(app): State<Arc<App>>, Json(m): Json<AgentMessage>) -> Api<StatusCode> {
    tokio::task::spawn_blocking(move || app.agent_send(&m.text, &m.state))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(bad_request)?;
    Ok(StatusCode::ACCEPTED)
}

async fn agent_reset(State(app): State<Arc<App>>) -> Api<StatusCode> {
    app.agent_reset().map_err(bad_request)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn agent_stop(State(app): State<Arc<App>>) -> StatusCode {
    app.agent_stop();
    StatusCode::NO_CONTENT
}

async fn agent_clear_collection(State(app): State<Arc<App>>) -> StatusCode {
    app.agent_clear_collection();
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
struct AgentConfig {
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    model: String,
    api_key: Option<String>,
}

async fn agent_configure(State(app): State<Arc<App>>, Json(c): Json<AgentConfig>) -> Api<StatusCode> {
    app.agent_configure(&c.base_url, &c.model, c.api_key.as_deref()).map_err(bad_request)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct ThreadOptions {
    /// Set when the UI is only rendering ahead of time: answer from the cache, don't call Gmail.
    #[serde(default)]
    peek: bool,
}

async fn get_thread(
    State(app): State<Arc<App>>,
    Path((account_id, thrid)): Path<(i64, String)>,
    Query(options): Query<ThreadOptions>,
) -> Api<Json<serde_json::Value>> {
    if !options.peek {
        // Re-check All Mail in the background; the UI hears about anything new over /api/events.
        let (bg, bg_thrid) = (app.clone(), thrid.clone());
        tokio::task::spawn_blocking(move || {
            if let Err(e) = bg.hydrate_thread(account_id, &bg_thrid) {
                eprintln!("thread {bg_thrid}: {e:#}");
                bg.emit("error", &format!("Couldn't load the full conversation: {e:#}"));
            }
        });
    }
    let out = blocking(move || {
        let mut messages = Vec::new();
        let (pictures, unsubscribed) = (app.db.picture_senders()?, app.db.unsubscribed()?);
        for m in app.db.thread_messages(account_id, &thrid)? {
            // what Gmail made of the sender, read from the top of the stored message
            let head = if m.has_body { app.db.message_head(m.id, 65536)? } else { None };
            let mut row = serde_json::to_value(&m)?;
            row["trust"] = serde_json::to_value(head.map(|h| trust::assess(&h)))?;
            let sender = m.from_addr.to_lowercase();
            row["pictures"] = pictures.contains(&sender).into();
            row["unsubscribed"] = unsubscribed.contains(&sender).into();
            messages.push(row);
        }
        Ok(serde_json::json!({ "messages": messages }))
    })
    .await?;
    Ok(Json(out))
}

/// Headers for anything that came out of an email: never let it act as part of the app.
/// A stored message never changes, so the window may keep its copy.
fn untrusted(mime: &str) -> [(header::HeaderName, HeaderValue); 4] {
    untrusted_page(mime, true)
}

/// Without `pictures`, nothing is fetched from outside: only pictures that came inside the
/// message are shown, so the sender cannot tell that it was opened.
fn untrusted_page(mime: &str, pictures: bool) -> [(header::HeaderName, HeaderValue); 4] {
    [
        (header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=2592000, immutable")),
        (header::CONTENT_TYPE, HeaderValue::from_str(mime).unwrap_or(HeaderValue::from_static("application/octet-stream"))),
        (header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
        (
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(if pictures {
                "default-src 'none'; img-src http: https: data: 'self'; style-src 'unsafe-inline'; font-src data:; \
                 sandbox allow-same-origin allow-popups allow-popups-to-escape-sandbox"
            } else {
                "default-src 'none'; img-src data: 'self'; style-src 'unsafe-inline'; font-src data:; \
                 sandbox allow-same-origin allow-popups allow-popups-to-escape-sandbox"
            }),
        ),
    ]
}

#[derive(Deserialize)]
struct ViewOptions {
    /// "text" for the plain-text version, "source" for the raw message; otherwise the formatted one.
    #[serde(default)]
    view: String,
    /// Let the message fetch its pictures from the sender's servers.
    #[serde(default)]
    pictures: bool,
}

async fn message_html(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Query(options): Query<ViewOptions>,
) -> Api<Response> {
    let pictures = options.pictures;
    let html = blocking(move || {
        Ok(app.db.raw_body(id)?.map(|raw| match options.view.as_str() {
            "text" => mime::render_text(&raw),
            "source" => mime::render_source(&raw),
            _ => mime::render_html(&raw, id),
        }))
    })
    .await?;
    let html = html.ok_or_else(not_found)?;
    Ok((untrusted_page("text/html; charset=utf-8", pictures), html).into_response())
}

/// The inbox conversations with mail from the same sender as this message.
async fn same_source(State(app): State<Arc<App>>, Path(id): Path<i64>) -> Api<Json<Vec<db::ThreadRow>>> {
    Ok(Json(blocking(move || app.db.threads_from_same_source(id)).await?))
}

#[derive(Deserialize)]
struct SubscriptionQuery {
    account: Option<i64>,
}

/// Every sender that offers a way to unsubscribe, with how much it sends.
async fn subscriptions(State(app): State<Arc<App>>, Query(q): Query<SubscriptionQuery>) -> Api<Json<Vec<serde_json::Value>>> {
    Ok(Json(blocking(move || app.db.subscriptions(q.account)).await?))
}

/// Leave a mailing list by the sender's own one-click address, and remember having done so.
async fn unsubscribe(State(app): State<Arc<App>>, Path(id): Path<i64>) -> Api<StatusCode> {
    let logger = app.clone();
    blocking(move || {
        // from the message itself when it is held, otherwise from what its headers said
        let sender = match app.db.raw_body(id)? {
            Some(raw) => facts::unsubscribe_now(&raw)?,
            None => {
                let (sender, said) = app.db.message_facts(id)?.ok_or_else(|| anyhow::anyhow!("No such message."))?;
                let u = &said["unsubscribe"];
                if u["one_click"] != true {
                    anyhow::bail!("This sender offers no one-click unsubscribe.");
                }
                facts::unsubscribe_post(u["url"].as_str().unwrap_or_default())?;
                sender.to_lowercase()
            }
        };
        app.log(&format!("unsubscribed from {sender}"));
        app.db.mark_unsubscribed(&sender)
    })
    .await
    .map_err(|e| {
        logger.log(&format!("unsubscribe failed: {}", e.1));
        ApiError(StatusCode::BAD_GATEWAY, e.1)
    })?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct PictureSender {
    sender: String,
    allow: bool,
}

/// Always (or no longer) show pictures in mail from one sender.
async fn picture_sender(State(app): State<Arc<App>>, Json(body): Json<PictureSender>) -> Api<StatusCode> {
    blocking(move || app.db.set_picture_sender(&body.sender, body.allow)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn message_parts(State(app): State<Arc<App>>, Path(id): Path<i64>) -> Api<Json<mime::Parts>> {
    let parts = blocking(move || Ok(app.db.raw_body(id)?.map(|raw| mime::parts(&raw)))).await?;
    Ok(Json(parts.ok_or_else(not_found)?))
}

async fn message_cid(State(app): State<Arc<App>>, Path((id, cid)): Path<(i64, String)>) -> Api<Response> {
    let part = blocking(move || Ok(app.db.raw_body(id)?.and_then(|raw| mime::inline_part(&raw, &cid)))).await?;
    let (mime, bytes) = part.ok_or_else(not_found)?;
    // Only plain bitmap formats are shown inline; anything else could carry script.
    if !["image/png", "image/jpeg", "image/gif", "image/webp", "image/bmp"].contains(&mime.as_str()) {
        return Err(not_found());
    }
    Ok((untrusted(&mime), bytes).into_response())
}

/// An attachment shown in place. Only picture, PDF and text formats get this far (see
/// `mime::preview_kind`), each served under a type that cannot run anything.
async fn attachment_preview(State(app): State<Arc<App>>, Path((id, index)): Path<(i64, usize)>) -> Api<Response> {
    let part = blocking(move || Ok(app.db.raw_body(id)?.and_then(|raw| mime::attachment_preview(&raw, index)))).await?;
    let (mime, bytes) = part.ok_or_else(not_found)?;
    if mime == "application/pdf" {
        // The built-in PDF viewer refuses to load inside a sandbox, so this one response goes
        // without it. A PDF cannot reach the page around it.
        return Ok((
            [
                (header::CONTENT_TYPE, HeaderValue::from_static("application/pdf")),
                (header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
                (header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=2592000, immutable")),
            ],
            bytes,
        )
            .into_response());
    }
    Ok((untrusted(mime), bytes).into_response())
}

async fn message_attachment(State(app): State<Arc<App>>, Path((id, index)): Path<(i64, usize)>) -> Api<Response> {
    let part = blocking(move || Ok(app.db.raw_body(id)?.and_then(|raw| mime::attachment(&raw, index)))).await?;
    let (name, _mime, bytes) = part.ok_or_else(not_found)?;
    let ascii: String = name.chars().map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' { c } else { '_' }).collect();
    let encoded: String = name
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect();
    let disposition = format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}");
    let mut resp = (untrusted("application/octet-stream"), bytes).into_response();
    if let Ok(v) = HeaderValue::from_str(&disposition) {
        resp.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(resp)
}

// ---- actions -------------------------------------------------------------
#[derive(Deserialize)]
struct Action {
    op: String,
}

async fn thread_action(
    State(app): State<Arc<App>>,
    Path((account_id, thrid)): Path<(i64, String)>,
    Json(body): Json<Action>,
) -> Api<Json<serde_json::Value>> {
    let logger = app.clone();
    let msgids = blocking(move || app.act(account_id, &thrid, &body.op)).await.map_err(|e| {
        logger.log(&format!("action failed: {}", e.1));
        e
    })?;
    // which messages it touched, so an archive can be undone exactly
    Ok(Json(serde_json::json!({ "msgids": msgids })))
}

#[derive(Deserialize)]
struct Unarchive {
    account_id: i64,
    msgids: Vec<String>,
}

async fn unarchive(State(app): State<Arc<App>>, Json(body): Json<Unarchive>) -> Api<StatusCode> {
    blocking(move || app.unarchive(body.account_id, &body.msgids)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn sync_status(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    Json(app.sync_status())
}

async fn sync_now(State(app): State<Arc<App>>) -> StatusCode {
    app.sync_soon();
    StatusCode::ACCEPTED
}

#[derive(Deserialize)]
struct OpenUrl {
    url: String,
}

/// Hand a link to the system's default browser. Only web and mail links are accepted,
/// because this string ends up on a command line.
async fn open_url(Json(body): Json<OpenUrl>) -> Api<StatusCode> {
    let url = body.url.trim();
    if !is_external_link(url) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "unsupported link".into()));
    }
    #[cfg(windows)]
    {
        open_external(url).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(StatusCode::NO_CONTENT)
    }
    #[cfg(not(windows))]
    Err(ApiError(StatusCode::NOT_IMPLEMENTED, "no system browser here".into()))
}

#[derive(Deserialize)]
#[cfg_attr(not(windows), allow(dead_code))]
struct ThreadRef {
    account_id: i64,
    thrid: String,
}

/// Open one conversation in a window of its own.
async fn open_thread_window(headers: HeaderMap, Json(t): Json<ThreadRef>) -> Api<StatusCode> {
    if !t.thrid.chars().all(|c| c.is_ascii_digit()) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "bad thread id".into()));
    }
    #[cfg(windows)]
    {
        // local_only has already checked that Host is this server
        let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default();
        window::open(format!("http://{host}/?thread={}:{}", t.account_id, t.thrid));
        Ok(StatusCode::NO_CONTENT)
    }
    #[cfg(not(windows))]
    {
        let _ = headers;
        Err(ApiError(StatusCode::NOT_IMPLEMENTED, "no native windows here".into()))
    }
}

async fn focus_window() -> StatusCode {
    #[cfg(windows)]
    window::focus();
    StatusCode::NO_CONTENT
}

/// Save an attachment into the Downloads folder and show it there. The app window has
/// no download bar of its own, and this never opens the file, so nothing runs by surprise.
async fn save_attachment(State(app): State<Arc<App>>, Path((id, index)): Path<(i64, usize)>) -> Api<Json<serde_json::Value>> {
    let saved = blocking(move || {
        let Some((name, _, bytes)) = app.db.raw_body(id)?.and_then(|raw| mime::attachment(&raw, index)) else {
            return Ok(None);
        };
        let dir = dirs::download_dir().ok_or_else(|| anyhow::anyhow!("no Downloads folder"))?;
        // Keep only a plain file name: no folders, no characters Windows rejects.
        let clean: String = name
            .chars()
            .map(|c| if c.is_control() || "\\/:*?\"<>|".contains(c) { '_' } else { c })
            .collect();
        let clean = clean.trim_matches(|c| c == '.' || c == ' ').to_string();
        let clean = if clean.is_empty() { "attachment".to_string() } else { clean };
        let (stem, ext) = match clean.rsplit_once('.') {
            Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
            _ => (clean.clone(), String::new()),
        };
        let mut path = dir.join(&clean);
        let mut n = 1;
        while path.exists() {
            path = dir.join(format!("{stem} ({n}){ext}"));
            n += 1;
        }
        std::fs::write(&path, bytes)?;
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{}", path.display())).spawn();
        }
        Ok(Some(path.display().to_string()))
    })
    .await?;
    Ok(Json(serde_json::json!({ "path": saved.ok_or_else(not_found)? })))
}

async fn events(State(app): State<Arc<App>>) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(app.events.subscribe()).filter_map(|m| m.ok().map(|d| Ok(Event::default().data(d))));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// The server only ever listens on localhost, but a web page in any browser on this PC
/// can still aim requests at it. Refuse anything not addressed to us by name (DNS
/// rebinding) and any write that another site's page initiated.
async fn local_only(headers: HeaderMap, method: Method, req: Request, next: Next) -> Response {
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    let name = host.rsplit_once(':').map(|(n, _)| n).unwrap_or(host);
    if !["127.0.0.1", "localhost"].contains(&name) {
        return (StatusCode::FORBIDDEN, "forbidden").into_response();
    }
    // The MCP endpoint is for programs, not pages: a browser always names the page behind a
    // POST in Origin, and a JSON content type cannot be sent cross-site without asking first.
    if req.uri().path() == "/mcp" {
        let json = headers.get(header::CONTENT_TYPE).and_then(|h| h.to_str().ok()).is_some_and(|t| t.starts_with("application/json"));
        if headers.contains_key(header::ORIGIN) || !json {
            return (StatusCode::FORBIDDEN, "forbidden").into_response();
        }
        return next.run(req).await;
    }
    if method != Method::GET {
        let origin = headers.get(header::ORIGIN).and_then(|h| h.to_str().ok());
        let same = origin.map(|o| o == format!("http://{host}"));
        if same != Some(true) {
            return (StatusCode::FORBIDDEN, "forbidden").into_response();
        }
    }
    next.run(req).await
}

/// MCP over HTTP: one JSON-RPC message (or a batch) in, its answer out. See `mcp`.
async fn mcp_message(State(app): State<Arc<App>>, Json(body): Json<serde_json::Value>) -> Api<Response> {
    let reply = blocking(move || Ok(app.mcp(&body))).await?;
    Ok(match reply {
        Some(reply) => Json(reply).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    })
}

fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/style.css", get(style_css))
        .route("/api/accounts", get(list_accounts).post(add_account))
        .route("/api/accounts/order", post(order_accounts))
        .route("/api/accounts/:id", delete(remove_account))
        .route("/api/threads", get(list_threads))
        .route("/api/thread/:account/:thrid", get(get_thread))
        .route("/api/thread/:account/:thrid/action", post(thread_action))
        .route("/api/message/:id/html", get(message_html))
        .route("/api/message/:id/parts", get(message_parts))
        .route("/api/message/:id/unsubscribe", post(unsubscribe))
        .route("/api/message/:id/same-source", get(same_source))
        .route("/api/pictures", post(picture_sender))
        .route("/api/message/:id/cid/:cid", get(message_cid))
        .route("/api/message/:id/attachment/:index", get(message_attachment))
        .route("/api/unarchive", post(unarchive))
        .route("/api/subscriptions", get(subscriptions))
        .route("/mcp", post(mcp_message))
        .route("/api/agent", get(agent_view).post(agent_send))
        .route("/api/agent/reset", post(agent_reset))
        .route("/api/agent/stop", post(agent_stop))
        .route("/api/agent/config", post(agent_configure))
        .route("/api/agent/collection/clear", post(agent_clear_collection))
        .route("/api/sync", get(sync_status).post(sync_now))
        .route("/api/message/:id/attachment/:index/save", post(save_attachment))
        .route("/api/message/:id/attachment/:index/preview", get(attachment_preview))
        .route("/api/open", post(open_url))
        .route("/api/focus", post(focus_window))
        .route("/api/window", post(open_thread_window))
        .route("/api/events", get(events))
        .layer(middleware::from_fn(local_only))
        .with_state(app)
}

// ---- window --------------------------------------------------------------
/// Only web and mail links may be handed to the system, because the string ends up on a command line.
fn is_external_link(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    ["https://", "http://", "mailto:"].iter().any(|s| lower.starts_with(s))
        && url.len() <= 4000
        && !url.chars().any(|c| c.is_control() || c == '"' || c.is_whitespace())
}

#[cfg(windows)]
fn open_external(url: &str) -> std::io::Result<()> {
    std::process::Command::new("rundll32.exe").args(["url.dll,FileProtocolHandler", url]).spawn().map(drop)
}

#[cfg(windows)]
mod window {
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    use tao::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
    use tao::platform::windows::WindowExtWindows;
    use tao::window::{Icon, Theme, Window, WindowBuilder, WindowId};
    use wry::{NewWindowResponse, WebContext, WebView, WebViewBuilder};

    pub enum UserEvent {
        /// Bring the main window to the front.
        Focus,
        /// Open another window on this URL (a single conversation).
        Open(String),
        Title(WindowId, String),
        Close(WindowId),
        /// The unread badge for the taskbar button, as (side, RGBA pixels); `None` clears it.
        Badge(WindowId, Option<(u32, Vec<u8>)>),
    }
    static PROXY: OnceLock<Mutex<EventLoopProxy<UserEvent>>> = OnceLock::new();

    fn send(event: UserEvent) {
        if let Some(p) = PROXY.get() {
            let _ = p.lock().unwrap().send_event(event);
        }
    }
    pub fn focus() {
        send(UserEvent::Focus);
    }
    pub fn open(url: String) {
        send(UserEvent::Open(url));
    }

    /// The app icon, drawn here so the binary needs no image files: an envelope on a purple tile.
    fn icon() -> Option<Icon> {
        const N: i32 = 64;
        let dist = |px: f32, py: f32, a: (f32, f32), b: (f32, f32)| {
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let t = (((px - a.0) * dx + (py - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
            ((px - a.0 - t * dx).powi(2) + (py - a.1 - t * dy).powi(2)).sqrt()
        };
        let (l, t, r, b) = (14.0, 21.0, 50.0, 44.0);
        let strokes = [
            ((l, t), (r, t)), ((r, t), (r, b)), ((r, b), (l, b)), ((l, b), (l, t)),
            ((l, t), (32.0, 35.0)), ((32.0, 35.0), (r, t)),
        ];
        let mut rgba = Vec::with_capacity((N * N * 4) as usize);
        for y in 0..N {
            for x in 0..N {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                // rounded square: distance from the nearest corner centre
                let (cx, cy) = (px.clamp(13.0, 51.0), py.clamp(13.0, 51.0));
                let inside = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() <= 13.0;
                let on_line = strokes.iter().any(|(p, q)| dist(px, py, *p, *q) <= 2.0);
                rgba.extend_from_slice(match (inside, on_line) {
                    (false, _) => &[0, 0, 0, 0],
                    (true, true) => &[255, 255, 255, 255],
                    (true, false) => &[0x5b, 0x2e, 0xe8, 255],
                });
            }
        }
        Icon::from_rgba(rgba, N as u32, N as u32).ok()
    }

    /// What the page sent after "badge:": nothing to clear it, or "<side>:<hex RGBA pixels>".
    /// `Err` if it is neither.
    fn parse_badge(spec: &str) -> Result<Option<(u32, Vec<u8>)>, ()> {
        if spec.is_empty() {
            return Ok(None);
        }
        let (side, hex) = spec.split_once(':').ok_or(())?;
        let side: u32 = side.parse().map_err(drop)?;
        if !(8..=64).contains(&side) || hex.len() != (side * side * 8) as usize || !hex.is_ascii() {
            return Err(());
        }
        let pixels: Result<Vec<u8>, _> =
            (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16)).collect();
        Ok(Some((side, pixels.map_err(drop)?)))
    }

    /// One app window showing `url`. The web view is listed first so it is dropped first.
    fn make_window(
        target: &EventLoopWindowTarget<UserEvent>,
        context: &mut WebContext,
        origin: &str,
        url: &str,
        size: (f64, f64),
        visible: bool,
    ) -> anyhow::Result<(WebView, Window)> {
        let window = WindowBuilder::new()
            .with_title("Unibox")
            .with_inner_size(LogicalSize::new(size.0, size.1))
            .with_min_inner_size(LogicalSize::new(560.0, 400.0))
            .with_window_icon(icon())
            .with_visible(visible)
            .build(target)?;
        let id = window.id();
        // Match the page colour so a resize never flashes white around the edges.
        let background = if window.theme() == Theme::Dark { (0x13, 0x13, 0x13, 255) } else { (0xf6, 0xf5, 0xf2, 255) };
        let origin = origin.to_string();
        let app = origin.clone();
        let webview = WebViewBuilder::new_with_web_context(context)
            .with_url(url)
            .with_background_color(background)
            // Pages never open windows themselves; outside links go to the real browser.
            .with_new_window_req_handler(|target, _| {
                if super::is_external_link(&target) {
                    let _ = super::open_external(&target);
                }
                NewWindowResponse::Deny
            })
            // A window only ever shows the app.
            .with_navigation_handler(move |target| {
                target.starts_with(&origin) || target.starts_with("about:") || target.starts_with("data:")
            })
            .with_document_title_changed_handler(move |title| send(UserEvent::Title(id, title)))
            // The page asks to be closed with window.ipc.postMessage("close"), and sets the
            // taskbar badge with "badge:…". Only the app's own pages are listened to.
            .with_ipc_handler(move |request| {
                if !request.uri().to_string().starts_with(&app) {
                    return;
                }
                if request.body() == "close" {
                    send(UserEvent::Close(id));
                } else if let Some(Ok(badge)) = request.body().strip_prefix("badge:").map(parse_badge) {
                    send(UserEvent::Badge(id, badge));
                }
            })
            .build(&window)?;
        Ok((webview, window))
    }

    /// The main window's place on screen as last saved: (x, y, width, height, maximized).
    fn saved_place(file: &Path) -> Option<((i32, i32, i32, i32), bool)> {
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
        let n = |k: &str| v.get(k)?.as_i64().map(|n| n as i32);
        Some(((n("x")?, n("y")?, n("w")?, n("h")?), v.get("maximized")?.as_bool()?))
    }

    /// Remember where the main window is. While it is maximized only that fact is saved, so
    /// un-maximizing next time goes back to the size it had before; minimized is ignored.
    fn save_place(window: &Window, file: &Path) {
        if window.is_minimized() {
            return;
        }
        let mut place = saved_place(file).map(|(p, _)| p);
        let maximized = window.is_maximized();
        if !maximized || place.is_none() {
            if let Ok(pos) = window.outer_position() {
                let size = window.outer_size();
                place = Some((pos.x, pos.y, size.width as i32, size.height as i32));
            }
        }
        if let Some((x, y, w, h)) = place {
            let json = serde_json::json!({ "x": x, "y": y, "w": w, "h": h, "maximized": maximized });
            let _ = std::fs::write(file, json.to_string());
        }
    }

    /// Put the main window back where it was, if that is still on a screen.
    fn restore_place(window: &Window, file: &Path) {
        let Some((saved, maximized)) = saved_place(file) else { return };
        let screens: Vec<_> = window
            .available_monitors()
            .map(|m| (m.position().x, m.position().y, m.size().width as i32, m.size().height as i32))
            .collect();
        if let Some((x, y, w, h)) = super::place_window(saved, &screens) {
            window.set_outer_position(PhysicalPosition::new(x, y));
            // sizes are saved with the frame; take it off again
            let frame = window.outer_size();
            let inner = window.inner_size();
            let (fw, fh) = (frame.width.saturating_sub(inner.width), frame.height.saturating_sub(inner.height));
            window.set_inner_size(PhysicalSize::new((w as u32).saturating_sub(fw), (h as u32).saturating_sub(fh)));
        }
        if maximized {
            window.set_maximized(true);
        }
    }

    /// Show the UI in a window of its own and run until that window is closed. Returns an
    /// error only if the window could not be created at all.
    pub fn run(url: &str, dir: &Path) -> anyhow::Result<()> {
        let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
        let _ = PROXY.set(Mutex::new(event_loop.create_proxy()));
        let mut context = WebContext::new(Some(dir.join("webview")));
        let origin = url.to_string();
        let main = make_window(&event_loop, &mut context, &origin, url, (1320.0, 880.0), false)?;
        let main_id = main.1.id();
        let place_file = dir.join("window.json");
        restore_place(&main.1, &place_file);
        main.1.set_visible(true);
        // Set when the main window moves or is resized; its place is written out once that
        // settles (and not only on close, so it survives the app being killed).
        let mut save_at: Option<Instant> = None;
        let mut windows: HashMap<WindowId, (WebView, Window)> = HashMap::from([(main_id, main)]);
        event_loop.run(move |event, target, control_flow| {
            if save_at.is_some_and(|t| Instant::now() >= t) {
                save_at = None;
                if let Some((_, w)) = windows.get(&main_id) {
                    save_place(w, &place_file);
                }
            }
            *control_flow = save_at.map_or(ControlFlow::Wait, ControlFlow::WaitUntil);
            match event {
                Event::WindowEvent { window_id, event: WindowEvent::Moved(_) | WindowEvent::Resized(_), .. }
                    if window_id == main_id =>
                {
                    let at = Instant::now() + Duration::from_millis(400);
                    save_at = Some(at);
                    *control_flow = ControlFlow::WaitUntil(at);
                }
                Event::WindowEvent { window_id, event: WindowEvent::CloseRequested, .. } => {
                    if window_id == main_id {
                        if let Some((_, w)) = windows.get(&main_id) {
                            save_place(w, &place_file);
                        }
                        *control_flow = ControlFlow::Exit;
                    } else {
                        windows.remove(&window_id);
                    }
                }
                Event::UserEvent(UserEvent::Focus) => {
                    if let Some((_, w)) = windows.get(&main_id) {
                        w.set_minimized(false);
                        w.set_focus();
                    }
                }
                Event::UserEvent(UserEvent::Open(url)) => {
                    match make_window(target, &mut context, &origin, &url, (920.0, 860.0), true) {
                        Ok(w) => drop(windows.insert(w.1.id(), w)),
                        Err(e) => eprintln!("window: {e:#}"),
                    }
                }
                Event::UserEvent(UserEvent::Title(id, title)) => {
                    if let Some((_, w)) = windows.get(&id) {
                        w.set_title(&title);
                    }
                }
                Event::UserEvent(UserEvent::Badge(id, badge)) if id == main_id => {
                    if let Some((_, w)) = windows.get(&id) {
                        let icon = badge.and_then(|(side, pixels)| Icon::from_rgba(pixels, side, side).ok());
                        w.set_overlay_icon(icon.as_ref());
                    }
                }
                Event::UserEvent(UserEvent::Close(id)) if id != main_id => drop(windows.remove(&id)),
                _ => {}
            }
        })
    }
}

/// Where to put a window that was last at `saved` (x, y, width, height in screen pixels),
/// given the screens there are now (same form). The size is capped to the screen it lands
/// on and the window is nudged fully onto it; `None` if it was on a screen that is gone,
/// in which case the system should place it afresh. A few pixels over the edge are left
/// alone: Windows counts an invisible resize border as part of the window, so one sitting
/// flush against the left edge reports x = -7.
#[cfg_attr(not(windows), allow(dead_code))]
fn place_window(saved: (i32, i32, i32, i32), screens: &[(i32, i32, i32, i32)]) -> Option<(i32, i32, i32, i32)> {
    let (x, y, w, h) = saved;
    let overlap = |s: &(i32, i32, i32, i32)| {
        let ox = ((x + w).min(s.0 + s.2) - x.max(s.0)).max(0) as i64;
        let oy = ((y + h).min(s.1 + s.3) - y.max(s.1)).max(0) as i64;
        ox * oy
    };
    // the screen holding most of the window
    let screen = screens.iter().max_by_key(|s| overlap(s)).filter(|s| overlap(s) > 0)?;
    const SLACK: i32 = 16;
    let cap = |len: i32, room: i32| if len <= room + 2 * SLACK { len } else { room };
    let (w, h) = (cap(w, screen.2), cap(h, screen.3));
    let fit = |at: i32, len: i32, from: i32, room: i32| {
        let (lo, hi) = (from.min(from + room - len), from.max(from + room - len));
        if (lo - SLACK..=hi + SLACK).contains(&at) { at } else { at.clamp(lo, hi) }
    };
    Some((fit(x, w, screen.0, screen.2), fit(y, h, screen.1, screen.3), w, h))
}

#[cfg(test)]
mod tests {
    use super::place_window;

    #[test]
    fn windows_come_back_on_screen() {
        let one = [(0, 0, 1920, 1080)];
        let two = [(0, 0, 1920, 1080), (1920, 0, 2560, 1440)];
        // fits: unchanged
        assert_eq!(place_window((100, 80, 1300, 900), &one), Some((100, 80, 1300, 900)));
        // on the second screen, which is still there
        assert_eq!(place_window((2500, 200, 1300, 900), &two), Some((2500, 200, 1300, 900)));
        // the second screen is gone
        assert_eq!(place_window((2500, 200, 1300, 900), &one), None);
        // hanging off the right and bottom edges: pulled back in
        assert_eq!(place_window((1500, 700, 1300, 900), &one), Some((620, 180, 1300, 900)));
        // off the top left
        assert_eq!(place_window((-200, -50, 1300, 900), &one), Some((0, 0, 1300, 900)));
        // bigger than the screen now is
        assert_eq!(place_window((0, 0, 3000, 2000), &one), Some((0, 0, 1920, 1080)));
        // flush against the left edge, as Windows reports it, is left alone
        assert_eq!(place_window((-7, 0, 1364, 1039), &one), Some((-7, 0, 1364, 1039)));
        // filling the screen with the invisible border hanging over each side
        assert_eq!(place_window((-7, 0, 1934, 1047), &one), Some((-7, 0, 1934, 1047)));
        // straddling two screens goes to the one holding more of it
        assert_eq!(place_window((1800, 100, 1000, 800), &two), Some((1920, 100, 1000, 800)));
    }
}

/// Tell the copy that is already running to come to the front.
fn focus_running_instance(port: u16) {
    use std::io::{Read, Write};
    if let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) {
        let host = format!("127.0.0.1:{port}");
        let _ = write!(
            s,
            "POST /api/focus HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let _ = s.read(&mut [0u8; 64]);
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let serve_only = args.iter().any(|a| a == "--serve");
    let port: u16 = flag("--port").and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT);
    if args.iter().any(|a| a == "--mcp") {
        // started by another program to talk MCP over stdin/stdout; the open app does the work
        return mcp::stdio(port);
    }
    let dir = flag("--data")
        .map(PathBuf::from)
        .or_else(|| dirs::data_local_dir().map(|d| d.join("Unibox")))
        .ok_or_else(|| anyhow::anyhow!("no data directory"))?;
    std::fs::create_dir_all(&dir)?;
    let url = format!("http://127.0.0.1:{port}/");

    let listener = match std::net::TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(_) if !serve_only => {
            focus_running_instance(port);
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
    listener.set_nonblocking(true)?;

    let imap_server = std::env::var("UNIBOX_IMAP").unwrap_or_else(|_| "imap.gmail.com:993".into());
    let app = App::new(db::Db::open(&dir.join("mail.db"))?, dir.clone(), imap_server);
    app.start();

    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let serve = async move {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        axum::serve(listener, router(app)).await?;
        Ok::<_, anyhow::Error>(())
    };

    #[cfg(windows)]
    if !serve_only {
        // The server runs behind the window; closing the window ends the program.
        std::thread::spawn(move || runtime.block_on(serve));
        if let Err(e) = window::run(&url, &dir) {
            // No WebView2 runtime, most likely: fall back to the default browser.
            eprintln!("window: {e:#}");
            let _ = open_external(&url);
            loop {
                std::thread::park();
            }
        }
        return Ok(());
    }

    println!("Unibox is running at {url}");
    runtime.block_on(serve)
}
