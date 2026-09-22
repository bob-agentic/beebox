//! axum: the WebSocket, the share routes, the hook endpoint, static assets.
//!
//! One socket per browser tab. It forwards only the panes the connection's
//! grant makes visible, and rejects anything the grant does not allow —
//! regardless of what the client believes its capabilities are.

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
use crate::proto::{In, Out, PaneId};
use crate::pty::PtyEvent;
use crate::share::{Grant, Scope};

/// Per-subscriber send budget. Bounded by bytes, not frames: frames are
/// variable-size and one coalesced flush can be large, so counting frames
/// bounds nothing.
const SEND_BUDGET_BYTES: usize = 4 * 1024 * 1024;

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

    let index = get({
        let ui = ui.clone();
        move |State(app): State<Arc<App>>, Query(q): Query<KeyQuery>| {
            index_html(ui.clone(), app, q.key)
        }
    });

    let mut r = Router::new()
        .route("/ws", get(ws_upgrade))
        // Lets the share page ask "does this token still need pairing?"
        // before opening the socket, so the code prompt appears deliberately
        // rather than being inferred from a failed WebSocket.
        .route("/pair/{token}", get(pair_state))
        .route(
            "/hooks/{pane}/{secret}",
            post(hook).layer(axum::extract::DefaultBodyLimit::max(
                crate::hooks::MAX_BODY_BYTES,
            )),
        )
        .route("/a/{token}", index.clone())
        .route("/w/{token}", index.clone())
        .route("/t/{token}", index.clone())
        .route("/p/{token}", index.clone())
        .route("/", index);

    // `--ui` serves from disk for development; otherwise the embedded copy.
    // Unknown paths fall through to the SPA document above, so a share link
    // deep-links correctly.
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
                return (StatusCode::FORBIDDEN, "sharing is off").into_response();
            }
            next.run(req).await
        },
    );
    r.layer(gate).with_state(app)
}

async fn index_html(ui: Option<std::path::PathBuf>, app: Arc<App>, key: Option<String>) -> Response {
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

    Html(inject_key(html, &app, key)).into_response()
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
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
struct WsQuery {
    /// Share token, from a `/a /w /t /p` link.
    token: Option<String>,
    /// Owner key. Required for full access — the default bind is every
    /// interface, so "no token" must mean *no access*, not *all access*.
    key: Option<String>,
    /// One-time pairing code, prompted for by the client when the grant
    /// demands one.
    pair: Option<String>,
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
}

/// The pairing code travels the second channel (spoken, messaged); the wire
/// carries only a salted hash of it. Constant-time compare, same as the keys.
pub fn hash_pair_code(code: &str, token: &str) -> String {
    use sha2::{Digest, Sha256};
    // The token doubles as a per-grant salt: two grants with the same code do
    // not share a hash.
    let digest = Sha256::digest(format!("{token}:{code}").as_bytes());
    use std::fmt::Write as _;
    digest.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
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

/// `{"pairing": true|false}` for a share token. Reveals only whether the code
/// prompt is needed — nothing about the grant's scope or validity beyond what
/// loading the share URL already implies.
async fn pair_state(
    State(app): State<Arc<App>>,
    Path(token): Path<String>,
) -> Response {
    match app.store.lock().await.grant(&token) {
        Ok(Some(g)) => axum::Json(serde_json::json!({ "pairing": g.pair_hash.is_some() }))
            .into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(app): State<Arc<App>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(q): Query<WsQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    let grant = match (&q.token, &q.key) {
        (Some(t), _) => match app.store.lock().await.grant(t) {
            Ok(Some(g)) => g,
            _ => return (StatusCode::FORBIDDEN, "unknown share token").into_response(),
        },
        (None, Some(k)) if app.is_owner_key(k) => app.owner_grant().await,
        _ => {
            return (
                StatusCode::UNAUTHORIZED,
                "owner key required — open the URL printed at startup",
            )
                .into_response()
        }
    };

    // The pairing gate. A grant that still carries a hash has not been paired:
    // the client must present the code, once. On the first success the hash is
    // cleared — the code is single-use, and from then on the link alone works.
    if let Some(want) = &grant.pair_hash {
        let given = q
            .pair
            .as_deref()
            .map(|p| hash_pair_code(&p.trim().to_uppercase(), &grant.token));
        match given {
            Some(h) if constant_time_eq(&h, want) => {
                let _ = app.store.lock().await.clear_pairing(&grant.token);
            }
            _ => {
                // 428: the client knows to prompt for the code and retry.
                return (StatusCode::PRECONDITION_REQUIRED, "pairing code required")
                    .into_response();
            }
        }
    }

    let device = device_name(headers.get(axum::http::header::USER_AGENT));
    // A kicked client must not simply reconnect, or the kick lasts
    // milliseconds.
    if app.is_banned(&grant, &addr.ip().to_string(), &device).await {
        return (StatusCode::FORBIDDEN, "disconnected by the owner").into_response();
    }
    let replay = q.replay;
    let sizing = q.sizing.unwrap_or(false);
    ws.on_upgrade(move |socket| serve(socket, app, grant, addr, device, replay, sizing))
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
    device: String,
    // How much scrollback this client can hold, if it said.
    replay: Option<usize>,
    // Whether this connection may resize the terminal.
    sizing: bool,
) {
    let (mut sink, mut stream) = {
        use futures_util::StreamExt;
        socket.split()
    };

    // One writer task owns the sink so every producer can be `!Send`-free and
    // backpressure is measured in one place.
    let (tx, mut rx) = mpsc::channel::<Out>(256);
    let writer = tokio::spawn(async move {
        use futures_util::SinkExt;
        let mut queued = 0usize;
        while let Some(frame) = rx.recv().await {
            let Ok(bytes) = rmp_serde::to_vec_named(&frame) else { continue };
            queued += bytes.len();
            if queued > SEND_BUDGET_BYTES {
                // The client is too far behind to catch up frame by frame; it
                // will resync from the ring on reconnect.
                break;
            }
            if sink.send(Message::Binary(bytes.into())).await.is_err() {
                break;
            }
            queued = 0;
        }
        let _ = sink.close().await;
    });

    let (session, mut kicked) = app.add_conn(&grant, addr.ip().to_string(), device).await;

    // First frame: the scope-filtered tree.
    let (tree, caps) = app.view_for(&grant).await;
    if tx.send(Out::Tree { tree, caps }).await.is_err() {
        app.remove_conn(session).await;
        return;
    }
    if grant.may_mutate() {
        let _ = tx.send(Out::Peers { peers: app.peers(session).await }).await;
        // Agents toggles are owner-level; viewers never see or set them.
        let _ = tx
            .send(Out::AgentSettings {
                settings: app.agent_settings().await,
                codex_hooks: app.codex_hooks_state().to_string(),
            })
            .await;
        let _ = tx.send(Out::WebServer { exposed: app.is_exposed() }).await;
    }

    // Replay each visible pane so a client joining mid-stream sees state
    // rather than a blank terminal. `modes` restores alt screen / bracketed
    // paste / application cursor keys, which a raw tail would have lost.
    //
    // The JUST_SPAWNED skip below only makes sense for the owner: the owner's
    // browser is the one that just issued the spawn, so replaying the pane's
    // first few bytes would print the prompt twice. A viewer never spawned
    // anything — it is joining an existing session and must see whatever is on
    // screen now, however little that is. A workspace/tab share exposes every
    // tab, but the owner may only ever have opened one; the others sit with a
    // resumed prompt well under the threshold, and skipping them is exactly
    // why switching to them showed a blank sheet. So for a viewer we always
    // replay, and first make sure the pane has a process at all (idempotent
    // with resume_all — a no-op when it is already running).
    const JUST_SPAWNED: u64 = 4096;
    let owner = grant.may_mutate();
    for pane in app.visible(&grant).await {
        if !owner {
            let _ = app.ensure_running(pane).await;
        }
        if let Some(pty) = app.pty_of(pane).await {
            if let Some((modes, data, through)) = app.ptys.attach_snapshot(pty, replay) {
                if !owner || through > JUST_SPAWNED {
                    let _ = tx.send(Out::Resync { pty, modes, data, through }).await;
                }
            }
        }
    }

    let mut pty_rx = app.ptys.subscribe();
    let mut tree_rx = app.subscribe_tree();
    let mut agent_rx = app.subscribe_agent();
    let _ = addr;

    loop {
        tokio::select! {
            // Kicked. Its own arm, so it takes effect at once rather than
            // waiting for the next broadcast to come round.
            _ = &mut kicked => {
                let _ = tx.send(Out::Closed { reason: crate::proto::CloseReason::Kicked }).await;
                break;
            }

            // Terminal output, filtered to this connection's scope.
            ev = pty_rx.recv() => match ev {
                Ok(PtyEvent::Output { pane, pty, data }) => {
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
                    app.mark_exited(pane).await;
                    if app.visible(&grant).await.contains(&pane) {
                        let _ = tx.send(Out::Exited { pane, code }).await;
                    }
                }
                // Lagged: the ring, not the channel, is the source of truth.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    for pane in app.visible(&grant).await {
                        if let Some(pty) = app.pty_of(pane).await {
                            if let Some((modes, data, through)) = app.ptys.attach_snapshot(pty, replay) {
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
                            if !grant.may_mutate() { continue }
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
                    if grant.may_mutate() {
                        let _ = tx.send(Out::Peers { peers: app.peers(session).await }).await;
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

                if handle(&app, &grant, sizing, inbound, &tx, session).await.is_break() {
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
    me: crate::proto::SessionId,
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

        In::CreateGrant { scope, writable, pairing } => {
            if !grant.may_mutate() {
                return Continue(());
            }
            let scope: Scope = scope.into();
            let token = random_token();
            // Workspace level and above always pair: those scopes reveal which
            // projects and tabs exist.
            let needs_pairing = pairing || scope.pairing_forced();
            let code = needs_pairing.then(random_pair_code);

            let g = Grant {
                token: token.clone(),
                scope,
                writable,
                // Only the hash is stored; the code itself goes back to the
                // owner once, to be spoken over a second channel.
                pair_hash: code.as_deref().map(|c| hash_pair_code(c, &token)),
            };
            if app.store.lock().await.put_grant(&g).is_ok() {
                // Creating a link implies wanting it reachable: open the web
                // server with it, so the copied URL works without a second
                // trip to the status-bar toggle.
                if !app.is_exposed() {
                    app.set_exposed(true).await;
                }
                let _ = tx
                    .send(Out::Grant {
                        url: format!("/{}/{}", scope.url_prefix(), token),
                        pair_code: code,
                        hosts: share_hosts(app.hook_port_now()),
                    })
                    .await;
            }
        }

        In::Kick { session: target } => {
            if grant.may_mutate() {
                app.kick(target).await;
            }
        }

        In::KickAll => {
            if grant.may_mutate() {
                // Never the connection that asked, or "disconnect all" would
                // close the window you clicked it in.
                app.kick_all_except(me).await;
            }
        }

        other => {
            // Everything else is owner-only and checked inside.
            if let Err(e) = app.handle_owner(grant, other).await {
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
) -> StatusCode {
    if !addr.ip().is_loopback() {
        return StatusCode::FORBIDDEN;
    }
    if !app.check_hook_secret(pane, &secret).await {
        return StatusCode::FORBIDDEN;
    }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return StatusCode::BAD_REQUEST;
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
    StatusCode::NO_CONTENT
}

fn random_token() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    (0..4)
        .map(|_| format!("{:04x}", rng.random::<u16>()))
        .collect::<Vec<_>>()
        .join("-")
}

/// Six characters, no vowels and no look-alikes: it gets read aloud.
fn random_pair_code() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"23456789BCDFGHJKLMNPQRSTVWXZ";
    let mut rng = rand::rng();
    (0..6)
        .map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char)
        .collect()
}

pub fn asset_headers() -> [(header::HeaderName, &'static str); 1] {
    [(header::CACHE_CONTROL, "no-cache")]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_codes_avoid_ambiguous_characters() {
        // The code is spoken over a second channel, so O/0 and I/1 would cost
        // more than the entropy they add.
        for _ in 0..200 {
            let c = random_pair_code();
            assert_eq!(c.len(), 6);
            assert!(
                !c.contains(['O', '0', 'I', '1', 'A', 'E', 'U']),
                "ambiguous or word-forming: {c}"
            );
        }
    }

    #[test]
    fn tokens_are_long_enough_to_not_be_guessed() {
        let a = random_token();
        let b = random_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 19, "4 groups of 4 hex digits plus separators");
    }
}
