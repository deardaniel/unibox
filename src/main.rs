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

/// Bump when the way messages are rendered changes, so cached copies in the window are dropped.
const RENDER_VERSION: &str = "4";

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
        Ok(serde_json::json!({ "messages": app.db.thread_messages(account_id, &thrid)? }))
    })
    .await?;
    Ok(Json(out))
}

/// Headers for anything that came out of an email: never let it act as part of the app.
/// A stored message never changes, so the window may keep its copy.
fn untrusted(mime: &str) -> [(header::HeaderName, HeaderValue); 4] {
    [
        (header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=2592000, immutable")),
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
        .route("/api/sync", get(sync_status).post(sync_now))
        .route("/api/message/:id/attachment/:index/save", post(save_attachment))
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
    use tao::dpi::LogicalSize;
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
    use tao::window::{Icon, Theme, Window, WindowBuilder, WindowId};
    use wry::{NewWindowResponse, WebContext, WebView, WebViewBuilder};

    pub enum UserEvent {
        /// Bring the main window to the front.
        Focus,
        /// Open another window on this URL (a single conversation).
        Open(String),
        Title(WindowId, String),
        Close(WindowId),
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

    /// One app window showing `url`. The web view is listed first so it is dropped first.
    fn make_window(
        target: &EventLoopWindowTarget<UserEvent>,
        context: &mut WebContext,
        origin: &str,
        url: &str,
        size: (f64, f64),
    ) -> anyhow::Result<(WebView, Window)> {
        let window = WindowBuilder::new()
            .with_title("Unibox")
            .with_inner_size(LogicalSize::new(size.0, size.1))
            .with_min_inner_size(LogicalSize::new(560.0, 400.0))
            .with_window_icon(icon())
            .build(target)?;
        let id = window.id();
        // Match the page colour so a resize never flashes white around the edges.
        let background = if window.theme() == Theme::Dark { (0x13, 0x13, 0x13, 255) } else { (0xf6, 0xf5, 0xf2, 255) };
        let origin = origin.to_string();
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
            // The page asks to be closed with window.ipc.postMessage("close").
            .with_ipc_handler(move |request| {
                if request.body() == "close" {
                    send(UserEvent::Close(id));
                }
            })
            .build(&window)?;
        Ok((webview, window))
    }

    /// Show the UI in a window of its own and run until that window is closed. Returns an
    /// error only if the window could not be created at all.
    pub fn run(url: &str, dir: &Path) -> anyhow::Result<()> {
        let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
        let _ = PROXY.set(Mutex::new(event_loop.create_proxy()));
        let mut context = WebContext::new(Some(dir.join("webview")));
        let origin = url.to_string();
        let main = make_window(&event_loop, &mut context, &origin, url, (1320.0, 880.0))?;
        let main_id = main.1.id();
        let mut windows: HashMap<WindowId, (WebView, Window)> = HashMap::from([(main_id, main)]);
        event_loop.run(move |event, target, control_flow| {
            *control_flow = ControlFlow::Wait;
            match event {
                Event::WindowEvent { window_id, event: WindowEvent::CloseRequested, .. } => {
                    if window_id == main_id {
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
                    match make_window(target, &mut context, &origin, &url, (920.0, 860.0)) {
                        Ok(w) => drop(windows.insert(w.1.id(), w)),
                        Err(e) => eprintln!("window: {e:#}"),
                    }
                }
                Event::UserEvent(UserEvent::Title(id, title)) => {
                    if let Some((_, w)) = windows.get(&id) {
                        w.set_title(&title);
                    }
                }
                Event::UserEvent(UserEvent::Close(id)) if id != main_id => drop(windows.remove(&id)),
                _ => {}
            }
        })
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
