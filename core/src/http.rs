//! axum: the WebSocket, the share routes, the hook endpoint, static assets.
//!
//! One socket per browser tab. It forwards only the panes the connection's
//! grant makes visible, and rejects anything the grant does not allow —
//! regardless of what the client believes its capabilities are.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::app::App;
use crate::proto::{In, Out, PaneId, PtyId};
use crate::pty::PtyEvent;
use crate::share::{Grant, Scope};

/// The built UI, compiled into the binary. This is what makes the desktop app
/// self-contained: no sidecar directory to ship, nothing to go missing.
#[derive(rust_embed::Embed)]
#[folder = "../ui/dist"]
struct Assets;

/// Serves until the process ends, on a listener the caller has already bound.
/// Both front ends go through this, so neither needs to depend on axum; and
/// binding first is what lets the desktop shell open its window only once the
/// port is actually accepting.
pub async fn serve_on(
    app: Arc<App>,
    listener: tokio::net::TcpListener,
    ui: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    axum::serve(
        listener,
        router(app, ui).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

pub fn router(app: Arc<App>, ui: Option<std::path::PathBuf>) -> Router {
    // The four share scopes all serve the same document; the token in the path
    // decides what the socket behind it may see.
    #[derive(Deserialize)]
    struct KeyQuery {
        key: Option<String>,
    }

    // The owner's page needs the key; without it there is nothing here.
    let index = get({
        let ui = ui.clone();
        move |State(app): State<Arc<App>>, Query(q): Query<KeyQuery>| {
            let ui = ui.clone();
            async move {
                match q.key.filter(|k| app.is_owner_key(k)) {
                    Some(k) => index_html(ui, &app, Some(k)).await,
                    None => not_found(),
                }
            }
        }
    });
    // A share page only for a link that exists, under its own scope's prefix.
    let share = get({
        let ui = ui.clone();
        move |State(app): State<Arc<App>>, Path((prefix, token)): Path<(String, String)>| {
            let ui = ui.clone();
            async move {
                let grant = app.store.lock().await.grant(&token).expect("read state.db");
                match grant {
                    Some(g) if g.scope.url_prefix() == prefix => index_html(ui, &app, None).await,
                    _ => not_found(),
                }
            }
        }
    });

    let mut r = Router::new()
        .route("/ws", get(ws_upgrade))
        .route(
            "/hooks/{pane}/{secret}",
            post(hook).layer(axum::extract::DefaultBodyLimit::max(
                crate::hooks::MAX_BODY_BYTES,
            )),
        )
        .route("/{prefix}/{token}", share)
        .route("/", index)
        .fallback(|| async { not_found() });

    // `--ui` serves from disk for development; otherwise the embedded copy.
    r = match ui {
        Some(dir) => r
            .nest_service("/assets", tower_http::services::ServeDir::new(dir.join("assets")))
            .nest_service("/fonts", tower_http::services::ServeDir::new(dir.join("fonts"))),
        None => r.route("/assets/{*path}", get(embedded)).route("/fonts/{*path}", get(embedded)),
    };
    // The exposure gate. Loopback is always served (the terminal itself rides
    // on this server); everyone else gets nothing until the owner flips the
    // Web Server toggle. Checked per request, so flipping it needs no rebind.
    let gate = axum::middleware::from_fn_with_state(
        app.clone(),
        |State(app): State<Arc<App>>,
         ConnectInfo(addr): ConnectInfo<SocketAddr>,
         req: axum::extract::Request,
         next: axum::middleware::Next| async move {
            if !addr.ip().is_loopback() && !app.is_exposed() {
                return not_found();
            }
            next.run(req).await
        },
    );
    r.layer(gate).with_state(app)
}

/// What anything unknown, revoked or not let in gets: a page that could have
/// come from any web server. Nothing in it says what is running here, so
/// probing the port teaches nobody that a terminal sits behind it.
const NOT_FOUND_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>404 Not Found</title>
<style>
html,body{margin:0;height:100%;background:#0f0f13;color:#8b8b95;font-family:system-ui,-apple-system,sans-serif}
body{display:flex;align-items:center;justify-content:center;text-align:center}
b{display:block;font:600 56px ui-monospace,Menlo,monospace;color:#3a3a44;letter-spacing:4px}
p{margin:10px 0 0;font-size:14px}
</style></head>
<body><div><b>404</b><p>This page isn’t available.</p></div></body></html>
"#;

pub fn not_found() -> Response {
    (StatusCode::NOT_FOUND, Html(NOT_FOUND_HTML)).into_response()
}

async fn index_html(ui: Option<std::path::PathBuf>, app: &App, key: Option<String>) -> Response {
    // `--ui` serves from disk for development, otherwise the embedded copy.
    // Either way the key must still be injected — returning the disk copy
    // early left the page unable to authenticate its own socket.
    let html = match &ui {
        Some(dir) => tokio::fs::read_to_string(dir.join("index.html")).await.ok(),
        None => None,
    }
    .or_else(|| Assets::get("index.html").map(|f| String::from_utf8_lossy(&f.data).into_owned()))
    // Only reachable if the UI was never built.
    .unwrap_or_else(|| include_str!("../assets/index.html").to_string());

    Html(inject_key(html, app, key)).into_response()
}

/// Hands the owner key to a page that presented it in the URL. Everything else
/// gets the page with no key, so it can only connect via a share token.
fn inject_key(html: String, app: &App, key: Option<String>) -> String {
    let Some(k) = key.filter(|k| app.is_owner_key(k)) else {
        return html;
    };
    // Must run before the bundle, which reads the key at module scope. Vite
    // puts its own script in <head>, so inject right after the opening tag —
    // matching `<head>` alone misses the `<head >`-with-attributes case and,
    // worse, lands after nothing at all when the tag is the literal `<head>`.
    match html.find("<head>") {
        Some(i) => {
            let at = i + "<head>".len();
            format!(
                "{}<script>window.__BEEBOX_KEY__={:?}</script>{}",
                &html[..at],
                k,
                &html[at..]
            )
        }
        None => html,
    }
}

/// Serves one embedded asset. The path is reconstructed from the request so a
/// single handler covers both `/assets` and `/fonts`.
async fn embedded(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    match Assets::get(path) {
        Some(f) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], f.data.into_owned()).into_response()
        }
        None => not_found(),
    }
}

#[derive(Deserialize)]
struct WsQuery {
    /// Share token, from a `/a /w /t /p` link.
    token: Option<String>,
    /// Owner key. Required for full access — the default bind is every
    /// interface, so "no token" must mean *no access*, not *all access*.
    key: Option<String>,
    /// A secret the client made up once and keeps. The first device to
    /// open a link binds it with this; any other device is turned away.
    device: Option<String>,
    /// How many lines of scrollback this client can hold. Sending more than
    /// it has room for means parsing work thrown away on arrival; sending
    /// less leaves its buffer half empty. Clamped, since it arrives from the
    /// page and a huge value would mean serialising the whole ring.
    replay: Option<usize>,
    /// Lets this connection set the terminal's size, which normally only the
    /// owner may do. For a phone: a 175-column layout on a 44-column screen
    /// overlaps itself, so the choice is between resizing the terminal and
    /// not being able to read it.
    sizing: Option<bool>,
    /// The size this client will render at, if it already knows. A resizing
    /// client that only says so after mounting gets the replay at the old
    /// width and re-wraps it at the new one — which is where zsh's
    /// reverse-video `%` came from on a phone: the marker is erased by
    /// padding to an exact column count, and the padding was counted for a
    /// different terminal.
    cols: Option<u16>,
    rows: Option<u16>,
}

/// Only a hash of a device's secret is stored, so the database alone does
/// not let anyone pose as the device.
fn hash_secret(secret: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    Sha256::digest(secret.as_bytes()).iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Every base URL the daemon can be reached on — the dev-server banner
/// treatment. Loopback is skipped (a share link is for another device), and
/// only IPv4: v6 addresses are unwieldy to read aloud or retype, and every
/// network this targets (LAN, Tailscale) offers a v4 address anyway.
fn share_hosts(port: u16) -> Vec<String> {
    let Ok(ifs) = if_addrs::get_if_addrs() else { return Vec::new() };
    let mut v4: Vec<String> = ifs
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.ip() {
            std::net::IpAddr::V4(ip) => Some(format!("http://{ip}:{port}")),
            std::net::IpAddr::V6(_) => None,
        })
        .collect();
    v4.sort();
    v4.dedup();
    v4
}

async fn ws_upgrade(
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
    State(app): State<Arc<App>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(q): Query<WsQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    // A plain GET here would otherwise get axum's own 400, which says a
    // WebSocket lives at this path.
    let Ok(ws) = ws else { return not_found() };
    let device = device_name(headers.get(axum::http::header::USER_AGENT));
    let grant = match (&q.token, &q.key) {
        (Some(t), _) => {
            let store = app.store.lock().await;
            let grant = store.grant(t).expect("read state.db");
            let mine = match (&grant, &q.device) {
                (Some(_), Some(secret)) => {
                    store.claim(t, &hash_secret(secret), &device).expect("write state.db")
                }
                _ => false,
            };
            match (grant, mine) {
                (Some(g), true) => g,
                // Another device's. Its page loaded, so say so over the
                // socket — a browser's WebSocket never shows the page an
                // HTTP status.
                (Some(_), false) => return ws.on_upgrade(refuse),
                (None, _) => return not_found(),
            }
        }
        (None, Some(k)) if app.is_owner_key(k) => app.owner_grant().await,
        _ => return not_found(),
    };

    let replay = q.replay;
    let sizing = q.sizing.unwrap_or(false);
    let first_size = q.cols.zip(q.rows);
    ws.on_upgrade(move |socket| serve(socket, app, grant, addr, replay, sizing, first_size))
}

/// Tells a client its link is no good, and closes.
async fn refuse(mut socket: WebSocket) {
    let frame = Out::Closed { reason: crate::proto::CloseReason::Revoked };
    if let Ok(bytes) = rmp_serde::to_vec_named(&frame) {
        let _ = socket.send(Message::Binary(bytes.into())).await;
    }
    let _ = socket.send(Message::Close(None)).await;
}

/// A readable device label for the connection manager. Coarse on purpose: it
/// exists so you recognise your own phone, not to fingerprint anyone.
fn device_name(ua: Option<&axum::http::HeaderValue>) -> String {
    let ua = ua.and_then(|v| v.to_str().ok()).unwrap_or("");
    let os = if ua.contains("iPhone") {
        "iPhone"
    } else if ua.contains("iPad") {
        "iPad"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("Mac OS X") || ua.contains("Macintosh") {
        "macOS"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        "Unknown"
    };
    // Order matters: Chrome and Edge both claim Safari.
    let browser = if ua.contains("Edg/") {
        "Edge"
    } else if ua.contains("Chrome/") {
        "Chrome"
    } else if ua.contains("Firefox/") {
        "Firefox"
    } else if ua.contains("Safari/") {
        "Safari"
    } else {
        "App"
    };
    format!("{browser} / {os}")
}

async fn serve(
    socket: WebSocket,
    app: Arc<App>,
    grant: Grant,
    addr: SocketAddr,
    // How much scrollback this client can hold, if it said.
    replay: Option<usize>,
    // Whether this connection may resize the terminal.
    sizing: bool,
    // The size it will render at, when it knew before connecting.
    first_size: Option<(u16, u16)>,
) {
    let (mut sink, mut stream) = {
        use futures_util::StreamExt;
        socket.split()
    };

    // One writer task owns the sink so every producer can be `!Send`-free.
    // Backpressure is the bounded channel: a slow client stalls this
    // connection's loop, its pty subscription lags, and it is resynced from
    // the ring. No byte budget — a large resync is a single frame, and cutting
    // the connection over it only brought the same resync back on reconnect.
    let (tx, mut rx) = mpsc::channel::<Out>(256);
    let writer = tokio::spawn(async move {
        use futures_util::SinkExt;
        while let Some(frame) = rx.recv().await {
            let Ok(bytes) = rmp_serde::to_vec_named(&frame) else { continue };
            if sink.send(Message::Binary(bytes.into())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    let (session, mut closed) = app.add_conn(&grant, addr.ip().to_string()).await;

    // Subscribed before anything is read, so nothing can fall between a
    // snapshot and the stream that carries on from it. What arrives twice is
    // dropped against `floor` below.
    let mut pty_rx = app.ptys.subscribe();
    let mut tree_rx = app.subscribe_tree();
    let mut agent_rx = app.subscribe_agent();

    // Someone arriving on a link needs the panes it was shown to have a
    // process — one whose restore failed, say. Before the first frame, so the
    // tree it gets already names the new pty. Not a pane whose process ended:
    // that one is being kept for its output, which a restart would wipe.
    if !grant.host {
        app.start_never_run(app.visible(&grant).await).await;
    }

    // First frame: the scope-filtered tree.
    let (tree, caps) = app.view_for(&grant).await;
    if tx.send(Out::Tree { tree, caps }).await.is_err() {
        app.remove_conn(session).await;
        return;
    }
    if grant.host {
        let _ = tx.send(Out::Peers { peers: app.peers().await }).await;
        // Agents toggles are the owner's; a share never sees or sets them.
        let _ = tx
            .send(Out::AgentSettings {
                settings: app.agent_settings().await,
                codex_hooks: app.codex_hooks_state().to_string(),
            })
            .await;
        let _ = tx.send(Out::WebServer { exposed: app.is_exposed() }).await;
    }

    // Where each pty's last snapshot to this client ended. Output up to it is
    // already on the client's screen; the stream only adds what follows.
    let mut floor: HashMap<PtyId, u64> = HashMap::new();

    // Before the replay, not after: a client that will resize should be sent
    // history already laid out for the width it is about to use.
    if sizing {
        if let Some((c, r)) = first_size {
            for pane in app.visible(&grant).await {
                let _ = app.set_viewport(&grant, true, pane, c, r).await;
            }
        }
    }

    // Replay each visible pane so a client joining mid-stream sees state
    // rather than a blank terminal. `modes` restores alt screen / bracketed
    // paste / application cursor keys, which a raw tail would have lost.
    for pane in app.visible(&grant).await {
        if let Some(pty) = app.pty_of(pane).await {
            if let Some((modes, data, through)) = app.ptys.attach_snapshot(pty, replay) {
                floor.insert(pty, through);
                let _ = tx.send(Out::Resync { pty, modes, data, through }).await;
            }
        }
    }

    loop {
        tokio::select! {
            // Closed by the owner. Its own arm, so it takes effect at once
            // rather than waiting for the next broadcast to come round.
            why = &mut closed => {
                if let Ok(Some(reason)) = why {
                    let _ = tx.send(Out::Closed { reason }).await;
                }
                break;
            }

            // Terminal output, filtered to this connection's scope.
            ev = pty_rx.recv() => match ev {
                Ok(PtyEvent::Output { pane, pty, data, end }) => {
                    if floor.get(&pty).is_some_and(|&f| end <= f) {
                        continue;
                    }
                    if app.visible(&grant).await.contains(&pane)
                        && tx.send(Out::Output { pty, data: data.to_vec() }).await.is_err()
                    {
                        break;
                    }
                }
                Ok(PtyEvent::Title { pane, text }) => {
                    if app.visible(&grant).await.contains(&pane) {
                        let _ = tx.send(Out::Title { pane, text }).await;
                    }
                }
                Ok(PtyEvent::Exited { pane, code }) => {
                    // The tree side of this is handled once, by the app's own
                    // watcher; here we only forward it to this socket.
                    if app.visible(&grant).await.contains(&pane) {
                        let _ = tx.send(Out::Exited { pane, code }).await;
                    }
                }
                // Lagged: the ring, not the channel, is the source of truth.
                // The receiver resumes at the oldest event still queued, which
                // the snapshot already holds — hence the floor.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    for pane in app.visible(&grant).await {
                        if let Some(pty) = app.pty_of(pane).await {
                            if let Some((modes, data, through)) = app.ptys.attach_snapshot(pty, replay) {
                                floor.insert(pty, through);
                                let _ = tx.send(Out::Resync { pty, modes, data, through }).await;
                            }
                        }
                    }
                }
                Err(_) => break,
            },

            // Agent status/title deltas, filtered to this connection's scope.
            // Lag is harmless here: the next Tree resend carries the full
            // status view anyway.
            delta = agent_rx.recv() => match delta {
                Ok(d) => {
                    let frame = match d {
                        crate::app::AgentDelta::Status { pane, status } => {
                            if !app.visible(&grant).await.contains(&pane) { continue }
                            Out::Status { pane, status }
                        }
                        crate::app::AgentDelta::SessionTitle { pane, text } => {
                            if !app.visible(&grant).await.contains(&pane) { continue }
                            Out::SessionTitle { pane, text }
                        }
                        crate::app::AgentDelta::Cwd { pane, path, git } => {
                            if !app.visible(&grant).await.contains(&pane) { continue }
                            Out::Cwd { pane, path, git }
                        }
                        crate::app::AgentDelta::Settings { settings } => {
                            if !grant.host { continue }
                            Out::AgentSettings {
                                settings,
                                codex_hooks: app.codex_hooks_state().to_string(),
                            }
                        }
                    };
                    if tx.send(frame).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => break,
            },

            // Any structural change: re-send this connection's own view.
            changed = tree_rx.recv() => match changed {
                Ok(_) => {
                    let (tree, caps) = app.view_for(&grant).await;
                    if tx.send(Out::Tree { tree, caps }).await.is_err() {
                        break;
                    }
                    if grant.host {
                        let _ = tx.send(Out::Peers { peers: app.peers().await }).await;
                        let _ = tx.send(Out::WebServer { exposed: app.is_exposed() }).await;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => break,
            },

            msg = {
                use futures_util::StreamExt;
                stream.next()
            } => {
                let Some(Ok(msg)) = msg else { break };
                let Message::Binary(bytes) = msg else { continue };
                let Ok(inbound) = rmp_serde::from_slice::<In>(&bytes) else { continue };

                if handle(&app, &grant, sizing, inbound, &tx).await.is_break() {
                    break;
                }
            }
        }
    }

    app.remove_conn(session).await;
    drop(tx);
    let _ = writer.await;
}

async fn handle(
    app: &Arc<App>,
    grant: &Grant,
    // Whether this connection may set the terminal's size.
    sizing: bool,
    msg: In,
    tx: &mpsc::Sender<Out>,
) -> std::ops::ControlFlow<()> {
    use std::ops::ControlFlow::{Break, Continue};

    match msg {
        In::Ping => {
            if tx.send(Out::Pong).await.is_err() {
                return Break(());
            }
        }

        In::Input { pane, data } => {
            // Both halves matter: the write bit and visibility. A read-only
            // viewer cannot type, and a writable one cannot reach outside its
            // scope.
            let tree = app.tree.lock().await;
            let allowed = grant.may_type(pane, &tree);
            drop(tree);
            if allowed {
                if let Some(pty) = app.pty_of(pane).await {
                    let _ = app.ptys.write(pty, &data);
                }
            }
        }

        In::Viewport { pane, cols, rows } => {
            // Advisory unless this connection was given sizing rights.
            if let Some((c, r)) = app.set_viewport(grant, sizing, pane, cols, rows).await {
                let _ = tx.send(Out::Size { pane, cols: c, rows: r }).await;
            }
        }

        In::CreateGrant { scope, writable } => {
            if !grant.host {
                return Continue(());
            }
            let scope: Scope = scope.into();
            let token = random_token();
            let g = Grant {
                token: token.clone(),
                scope,
                writable,
                // A link, never the machine.
                host: false,
            };
            app.store.lock().await.put_grant(&g).expect("write state.db");
            // Creating a link implies wanting it reachable: open the web
            // server with it, so the copied URL works without a second trip
            // to the status-bar toggle.
            if !app.is_exposed() {
                app.set_exposed(true).await;
            }
            let _ = tx
                .send(Out::Grant {
                    url: format!("/{}/{}", scope.url_prefix(), token),
                    hosts: share_hosts(app.hook_port_now()),
                })
                .await;
        }

        other => {
            // Everything else is owner-only and checked inside.
            if let Err(e) = app.handle_host(grant, other).await {
                tracing::debug!("owner action refused: {e}");
            }
        }
    }
    Continue(())
}

/// Agent hooks. Three gates before anything is parsed: the caller must be on
/// loopback (the daemon may listen on 0.0.0.0, hooks must not), the pane must
/// be known, and the per-pane random secret must match in constant time —
/// the port may be reachable from wherever the user chose to expose it, and a
/// guessable path would let anything forge status.
///
/// Unknown events and malformed bodies answer 204/400 without side effects:
/// hook failures must never break the agent.
async fn hook(
    State(app): State<Arc<App>>,
    Path((pane, secret)): Path<(PaneId, String)>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    body: axum::body::Bytes,
) -> Response {
    // From anywhere else, not even the admission that hooks exist.
    if !addr.ip().is_loopback() {
        return not_found();
    }
    if !app.check_hook_secret(pane, &secret).await {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    // The adapter stamps the capture time; without one, receipt time is the
    // best ordering signal available.
    let at_ms = v
        .get("at_ms")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_else(crate::hooks::now_ms);

    // Which agent this payload speaks for. Explicit tag first (the BeeBox
    // adapters send one); a bare Claude hook body is recognised by its
    // `hook_event_name`.
    let agent = v.get("agent").and_then(serde_json::Value::as_str);
    let ev = match agent {
        Some("claude") | None => crate::hooks::normalize_claude(&v, at_ms),
        Some("codex") => crate::hooks::normalize_codex(&v, at_ms),
        // OpenCode is out of scope for now.
        Some(_) => None,
    };

    if let Some(ev) = ev {
        // Parse first, mutate briefly: the handler never holds the tree lock
        // across file IO (the transcript read happened inside normalize).
        app.apply_agent_event(pane, ev).await;
    }
    StatusCode::NO_CONTENT.into_response()
}

fn random_token() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    (0..4)
        .map(|_| format!("{:04x}", rng.random::<u16>()))
        .collect::<Vec<_>>()
        .join("-")
}

pub fn asset_headers() -> [(header::HeaderName, &'static str); 1] {
    [(header::CACHE_CONTROL, "no-cache")]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_enough_to_not_be_guessed() {
        let a = random_token();
        let b = random_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 19, "4 groups of 4 hex digits plus separators");
    }
}
