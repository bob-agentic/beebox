//! HTTP-level tests for the hook ingest: the security gates and the vertical
//! path from a raw Claude payload to a broadcast status delta.

use std::net::SocketAddr;
use std::sync::Arc;

use beebox_core::app::{AgentDelta, App};
use beebox_core::proto::{AgentKind, AgentPhase, AgentSetting};
use beebox_core::store::Store;

async fn served_app() -> (Arc<App>, SocketAddr) {
    let app = App::new(Store::in_memory().unwrap(), 100);
    app.bootstrap("/tmp".into()).await.unwrap();
    // The six toggles default OFF; these tests exercise the pipeline with
    // Claude enabled. The gate itself is covered by its own tests below.
    app.set_agent_setting(AgentKind::Claude, AgentSetting::Status, true).await;
    app.set_agent_setting(AgentKind::Claude, AgentSetting::Resume, true).await;
    let router = beebox_core::http::router(app.clone(), None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (app, addr)
}

async fn first_pane(app: &App) -> u64 {
    app.tree.lock().await.workspaces[0].tabs[0].panes[0].id
}

async fn post(addr: SocketAddr, path: &str, body: &str) -> u16 {
    let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = stream;
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).await.unwrap();
    resp.split_whitespace().nth(1).unwrap().parse().unwrap()
}

#[tokio::test]
async fn a_valid_claude_event_drives_the_state_machine() {
    let (app, addr) = served_app().await;
    let pane = first_pane(&app).await;
    let secret = app.hook_secret(pane).await;
    let mut rx = app.subscribe_agent();

    let code = post(
        addr,
        &format!("/hooks/{pane}/{secret}"),
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"abc-123","prompt":"fix it"}"#,
    )
    .await;
    assert_eq!(code, 204);

    match rx.recv().await.unwrap() {
        AgentDelta::Status { pane: p, status } => {
            assert_eq!(p, pane);
            assert_eq!(status.phase, AgentPhase::Running);
            assert!(status.started_at_ms.is_some());
        }
        other => panic!("expected a status delta, got {other:?}"),
    }
    // The session id landed for resume, already validated.
    assert_eq!(
        app.tree.lock().await.pane(pane).unwrap().session_ref.as_deref(),
        Some("abc-123")
    );
}

#[tokio::test]
async fn wrong_and_rotated_secrets_are_refused() {
    let (app, addr) = served_app().await;
    let pane = first_pane(&app).await;
    let old = app.hook_secret(pane).await;

    let body = r#"{"hook_event_name":"UserPromptSubmit"}"#;
    assert_eq!(post(addr, &format!("/hooks/{pane}/nope"), body).await, 403);
    assert_eq!(post(addr, "/hooks/99999/whatever", body).await, 403);

    // Respawning the pane rotates the secret; the old one must die with the
    // old process so stale hooks cannot paint the new one.
    app.mark_exited(pane).await;
    app.ensure_running(pane).await.unwrap();
    let fresh = app.hook_secret(pane).await;
    assert_ne!(old, fresh, "spawn must rotate the secret");
    assert_eq!(post(addr, &format!("/hooks/{pane}/{old}"), body).await, 403);
    assert_eq!(post(addr, &format!("/hooks/{pane}/{fresh}"), body).await, 204);
}

#[tokio::test]
async fn malformed_bodies_are_rejected_and_unknown_events_ignored() {
    let (app, addr) = served_app().await;
    let pane = first_pane(&app).await;
    let secret = app.hook_secret(pane).await;
    let path = format!("/hooks/{pane}/{secret}");

    assert_eq!(post(addr, &path, "not json").await, 400);
    // Unknown events succeed silently: a hook failure must never bother the
    // agent, and there is nothing to do with an event we do not track.
    assert_eq!(post(addr, &path, r#"{"hook_event_name":"PreCompact"}"#).await, 204);
    assert_eq!(
        app.tree.lock().await.pane(pane).unwrap().status.view().phase,
        AgentPhase::NeverRan,
        "unknown events must not move the state"
    );
}

#[tokio::test]
async fn oversized_bodies_are_refused() {
    let (app, addr) = served_app().await;
    let pane = first_pane(&app).await;
    let secret = app.hook_secret(pane).await;

    let big = format!(
        r#"{{"hook_event_name":"UserPromptSubmit","prompt":"{}"}}"#,
        "x".repeat(80 * 1024)
    );
    let code = post(addr, &format!("/hooks/{pane}/{secret}"), &big).await;
    assert_eq!(code, 413, "64KiB body limit");
}

#[tokio::test]
async fn out_of_order_events_do_not_roll_the_ui_back() {
    let (app, addr) = served_app().await;
    let pane = first_pane(&app).await;
    let secret = app.hook_secret(pane).await;
    let path = format!("/hooks/{pane}/{secret}");

    // Turn runs and finishes...
    post(addr, &path, r#"{"hook_event_name":"UserPromptSubmit","at_ms":1000}"#).await;
    post(addr, &path, r#"{"hook_event_name":"Stop","at_ms":2000}"#).await;
    // ...then a delayed PreToolUse from mid-turn arrives.
    post(
        addr,
        &path,
        r#"{"hook_event_name":"PreToolUse","at_ms":1500,"tool_name":"Bash","tool_input":{"command":"ls"}}"#,
    )
    .await;

    assert_eq!(
        app.tree.lock().await.pane(pane).unwrap().status.view().phase,
        AgentPhase::Success,
        "a stale running event must not overwrite a finished turn"
    );
}

async fn post_owned(addr: SocketAddr, path: String, body: String) -> u16 {
    post(addr, &path, &body).await
}

#[tokio::test]
async fn the_full_claude_turn_reaches_failed_on_tool_error() {
    let (app, addr) = served_app().await;
    let pane = first_pane(&app).await;
    let secret = app.hook_secret(pane).await;
    let path = format!("/hooks/{pane}/{secret}");

    for (i, body) in [
        r#"{"hook_event_name":"SessionStart","session_id":"s-1"}"#,
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s-1","prompt":"do the thing"}"#,
        r#"{"hook_event_name":"PreToolUse","tool_name":"Edit","tool_input":{"file_path":"/r/src/auth.rs"}}"#,
        r#"{"hook_event_name":"PostToolUse","tool_response":{"is_error":true}}"#,
        r#"{"hook_event_name":"Stop"}"#,
    ]
    .iter()
    .enumerate()
    {
        // Explicit timestamps keep the ordering unambiguous even when two
        // posts land within one millisecond.
        let stamped = body.replacen('{', &format!("{{\"at_ms\":{},", 1000 + i as i64), 1);
        assert_eq!(post_owned(addr, path.clone(), stamped).await, 204);
    }

    let tree = app.tree.lock().await;
    let view = tree.pane(pane).unwrap().status.view().clone();
    drop(tree);
    assert_eq!(view.phase, AgentPhase::Failed, "one tool error fails the turn");
    assert_eq!(view.agent, Some(beebox_core::proto::AgentKind::Claude));
    assert!(view.started_at_ms.is_some());
}

#[tokio::test]
async fn status_toggle_gates_events_and_turning_off_clears_the_dot() {
    let (app, addr) = served_app().await;
    let pane = first_pane(&app).await;
    let secret = app.hook_secret(pane).await;
    let path = format!("/hooks/{pane}/{secret}");

    post(addr, &path, r#"{"hook_event_name":"UserPromptSubmit","at_ms":1000}"#).await;
    assert_eq!(
        app.tree.lock().await.pane(pane).unwrap().status.view().phase,
        AgentPhase::Running
    );

    // OFF wipes the live dot immediately — no stale-dot bug.
    let mut rx = app.subscribe_agent();
    app.set_agent_setting(AgentKind::Claude, AgentSetting::Status, false).await;
    assert_eq!(
        app.tree.lock().await.pane(pane).unwrap().status.view().phase,
        AgentPhase::NeverRan
    );
    let mut saw_clear = false;
    while let Ok(d) = rx.try_recv() {
        if let AgentDelta::Status { status, .. } = d {
            saw_clear |= status.phase == AgentPhase::NeverRan;
        }
    }
    assert!(saw_clear, "clients must be told to hide the dot at once");

    // Events while OFF do nothing.
    post(addr, &path, r#"{"hook_event_name":"UserPromptSubmit","at_ms":2000}"#).await;
    assert_eq!(
        app.tree.lock().await.pane(pane).unwrap().status.view().phase,
        AgentPhase::NeverRan
    );

    // Back ON: state resumes from the next live event, cleanly.
    app.set_agent_setting(AgentKind::Claude, AgentSetting::Status, true).await;
    post(addr, &path, r#"{"hook_event_name":"UserPromptSubmit","at_ms":3000}"#).await;
    assert_eq!(
        app.tree.lock().await.pane(pane).unwrap().status.view().phase,
        AgentPhase::Running
    );
}

#[tokio::test]
async fn resume_toggle_gates_session_ids_both_ways() {
    let (app, addr) = served_app().await;
    let pane = first_pane(&app).await;
    let secret = app.hook_secret(pane).await;
    let path = format!("/hooks/{pane}/{secret}");

    // ON: id is stored.
    post(addr, &path, r#"{"hook_event_name":"UserPromptSubmit","session_id":"sid-1","at_ms":1000}"#).await;
    assert_eq!(
        app.tree.lock().await.pane(pane).unwrap().session_ref.as_deref(),
        Some("sid-1")
    );

    // OFF: existing ids are cleared (memory and DB) and new ones refused.
    app.set_agent_setting(AgentKind::Claude, AgentSetting::Resume, false).await;
    assert!(app.tree.lock().await.pane(pane).unwrap().session_ref.is_none());
    post(addr, &path, r#"{"hook_event_name":"UserPromptSubmit","session_id":"sid-2","at_ms":2000}"#).await;
    assert!(app.tree.lock().await.pane(pane).unwrap().session_ref.is_none());
}

#[tokio::test]
async fn resume_types_the_command_into_a_respawned_pane() {
    // The full launch path: an agent pane with a stored session id, resume ON,
    // respawn — the resolver must type `claude --resume <id>` into the PTY.
    // Proven by watching the PTY echo the injected input.
    use beebox_core::pty::PtyEvent;
    let (app, addr) = served_app().await;
    let pane = first_pane(&app).await;
    let secret = app.hook_secret(pane).await;
    post(
        addr,
        &format!("/hooks/{pane}/{secret}"),
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"resume-me-1","at_ms":1000}"#,
    )
    .await;

    let mut rx = app.ptys.subscribe();
    app.mark_exited(pane).await;
    app.ensure_running(pane).await.unwrap();

    let mut seen = String::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while !seen.contains("claude --resume resume-me-1") {
        let left = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or_else(|| panic!("resume command never reached the pty:\n{seen}"));
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Ok(PtyEvent::Output { data, .. })) => {
                seen.push_str(&String::from_utf8_lossy(&data))
            }
            Ok(Ok(_)) => {}
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
            Ok(Err(_)) | Err(_) => panic!("pty gone:\n{seen}"),
        }
    }

    // And with the toggle OFF the id is gone, so a respawn opens a plain
    // shell — no resume, no error, no dialog.
    app.set_agent_setting(AgentKind::Claude, AgentSetting::Resume, false).await;
    assert!(app.tree.lock().await.pane(pane).unwrap().session_ref.is_none());
}

#[tokio::test]
async fn codex_events_flow_through_the_same_pipeline() {
    let (app, addr) = served_app().await;
    app.set_agent_setting(AgentKind::Codex, AgentSetting::Status, true).await;
    let pane = first_pane(&app).await;
    let secret = app.hook_secret(pane).await;
    let path = format!("/hooks/{pane}/{secret}");

    post(addr, &path, r#"{"agent":"codex","hook_event_name":"UserPromptSubmit","session_id":"cx-1","at_ms":1000}"#).await;
    post(addr, &path, r#"{"agent":"codex","hook_event_name":"PermissionRequest","at_ms":2000}"#).await;
    {
        let tree = app.tree.lock().await;
        let v = tree.pane(pane).unwrap().status.view().clone();
        assert_eq!(v.phase, AgentPhase::NeedsInput, "PermissionRequest is the amber dot");
        assert_eq!(v.agent, Some(AgentKind::Codex));
    }
    post(addr, &path, r#"{"agent":"codex","hook_event_name":"PostToolUse","tool_response":{"is_error":false},"at_ms":3000}"#).await;
    post(addr, &path, r#"{"agent":"codex","hook_event_name":"Stop","at_ms":4000}"#).await;
    assert_eq!(
        app.tree.lock().await.pane(pane).unwrap().status.view().phase,
        AgentPhase::Success
    );
}

#[tokio::test]
async fn loopback_is_always_served_but_hooks_still_need_the_secret() {
    // The exposure gate must not break the terminal's own transport: loopback
    // requests pass with sharing off (the default).
    let (app, addr) = served_app().await;
    assert!(!app.is_exposed(), "sharing must default to off");
    let pane = first_pane(&app).await;
    let secret = app.hook_secret(pane).await;
    assert_eq!(
        post(addr, &format!("/hooks/{pane}/{secret}"), r#"{"hook_event_name":"SessionStart"}"#)
            .await,
        204
    );
}
