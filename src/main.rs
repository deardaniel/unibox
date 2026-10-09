// Release builds on Windows are a GUI program: no console window behind the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod db;
mod imap;
mod mime;
mod secrets;
mod sync;

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

async fn index() -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "no-cache")], Html(asset("index.html", include_str!("../ui/index.html"))))
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
    #[serde(default)]
    q: String,
    #[serde(default)]
    offset: i64,
}

async fn list_threads(State(app): State<Arc<App>>, Query(q): Query<ThreadQuery>) -> Api<Json<Vec<db::ThreadRow>>> {
    Ok(Json(blocking(move || app.db.threads(q.account, q.q.trim(), 60, q.offset.max(0))).await?))
}

async fn get_thread(
    State(app): State<Arc<App>>,
    Path((account_id, thrid)): Path<(i64, String)>,
) -> Api<Json<serde_json::Value>> {
    // Top up the conversation from All Mail in the background; the UI hears about it over /api/events.
    let (bg, bg_thrid) = (app.clone(), thrid.clone());
    tokio::task::spawn_blocking(move || {
        if let Err(e) = bg.hydrate_thread(account_id, &bg_thrid) {
            eprintln!("thread {bg_thrid}: {e:#}");
            bg.emit("error", &format!("Couldn't load the full conversation: {e:#}"));
        }
    });
    let out = blocking(move || {
        let rows = app.db.thread_messages(account_id, &thrid)?;
        let mut messages = Vec::new();
        for m in rows {
            let attachments = match app.db.raw_body(m.id)? {
                Some(raw) => mime::attachments(&raw),
                None => Vec::new(),
            };
            let mut v = serde_json::to_value(&m)?;
            v["attachments"] = serde_json::to_value(attachments)?;
            messages.push(v);
        }
        Ok(serde_json::json!({ "messages": messages }))
    })
    .await?;
    Ok(Json(out))
}

/// Headers for anything that came out of an email: never let it act as part of the app.
fn untrusted(mime: &str) -> [(header::HeaderName, HeaderValue); 3] {
    [
        (header::CONTENT_TYPE, HeaderValue::from_str(mime).unwrap_or(HeaderValue::from_static("application/octet-stream"))),
        (header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
        (
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'none'; img-src http: https: data: 'self'; style-src 'unsafe-inline'; font-src data:; \
                 sandbox allow-same-origin allow-popups allow-popups-to-escape-sandbox",
            ),
        ),
    ]
}

async fn message_html(State(app): State<Arc<App>>, Path(id): Path<i64>) -> Api<Response> {
    let html = blocking(move || Ok(app.db.raw_body(id)?.map(|raw| mime::render_html(&raw, id)))).await?;
    let html = html.ok_or_else(not_found)?;
    Ok((untrusted("text/html; charset=utf-8"), html).into_response())
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
) -> Api<StatusCode> {
    blocking(move || app.act(account_id, &thrid, &body.op)).await?;
    Ok(StatusCode::NO_CONTENT)
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
    let ok_scheme = ["https://", "http://", "mailto:"].iter().any(|s| url.to_ascii_lowercase().starts_with(s));
    if !ok_scheme || url.len() > 4000 || url.chars().any(|c| c.is_control() || c == '"' || c.is_whitespace()) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "unsupported link".into()));
    }
    #[cfg(windows)]
    {
        std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn()
            .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(StatusCode::NO_CONTENT)
    }
    #[cfg(not(windows))]
    Err(ApiError(StatusCode::NOT_IMPLEMENTED, "no system browser here".into()))
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
    if method != Method::GET {
        let origin = headers.get(header::ORIGIN).and_then(|h| h.to_str().ok());
        let same = origin.map(|o| o == format!("http://{host}"));
        if same != Some(true) {
            return (StatusCode::FORBIDDEN, "forbidden").into_response();
        }
    }
    next.run(req).await
}

fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/style.css", get(style_css))
        .route("/api/accounts", get(list_accounts).post(add_account))
        .route("/api/accounts/:id", delete(remove_account))
        .route("/api/threads", get(list_threads))
        .route("/api/thread/:account/:thrid", get(get_thread))
        .route("/api/thread/:account/:thrid/action", post(thread_action))
        .route("/api/message/:id/html", get(message_html))
        .route("/api/message/:id/cid/:cid", get(message_cid))
        .route("/api/message/:id/attachment/:index", get(message_attachment))
        .route("/api/sync", post(sync_now))
        .route("/api/open", post(open_url))
        .route("/api/events", get(events))
        .layer(middleware::from_fn(local_only))
        .with_state(app)
}

// ---- window --------------------------------------------------------------
/// Show the UI in a chromeless Edge (or Chrome) window with its own profile, and
/// return the browser process so the app can quit when that window closes.
#[cfg(windows)]
fn open_window(url: &str, dir: &std::path::Path) -> Option<std::process::Child> {
    let roots = ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"];
    let exes = ["Microsoft\\Edge\\Application\\msedge.exe", "Google\\Chrome\\Application\\chrome.exe"];
    for exe in exes {
        for root in roots {
            let Ok(base) = std::env::var(root) else { continue };
            let path = PathBuf::from(base).join(exe);
            if !path.exists() {
                continue;
            }
            let child = std::process::Command::new(path)
                .arg(format!("--app={url}"))
                .arg(format!("--user-data-dir={}", dir.join("window").display()))
                .args(["--no-first-run", "--no-default-browser-check", "--window-size=1280,860"])
                .spawn();
            if let Ok(child) = child {
                return Some(child);
            }
        }
    }
    let _ = std::process::Command::new("rundll32.exe").args(["url.dll,FileProtocolHandler", url]).spawn();
    None
}

#[cfg(not(windows))]
fn open_window(url: &str, _dir: &std::path::Path) -> Option<std::process::Child> {
    println!("Unibox is running at {url}");
    None
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let serve_only = args.iter().any(|a| a == "--serve");
    let port: u16 = flag("--port").and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT);
    let dir = flag("--data")
        .map(PathBuf::from)
        .or_else(|| dirs::data_local_dir().map(|d| d.join("Unibox")))
        .ok_or_else(|| anyhow::anyhow!("no data directory"))?;
    std::fs::create_dir_all(&dir)?;
    let url = format!("http://127.0.0.1:{port}/");

    let listener = match std::net::TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(_) if !serve_only => {
            // Already running: just bring up another window onto it.
            open_window(&url, &dir);
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
    listener.set_nonblocking(true)?;

    let imap_server = std::env::var("UNIBOX_IMAP").unwrap_or_else(|_| "imap.gmail.com:993".into());
    let app = App::new(db::Db::open(&dir.join("mail.db"))?, dir.clone(), imap_server);
    app.start();

    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let server = axum::serve(listener, router(app));
        if serve_only {
            println!("Unibox is running at {url}");
            server.await?;
        } else if let Some(mut window) = open_window(&url, &dir) {
            tokio::select! {
                r = server => r?,
                _ = tokio::task::spawn_blocking(move || window.wait()) => {}
            }
        } else {
            server.await?;
        }
        Ok::<_, anyhow::Error>(())
    })
}
