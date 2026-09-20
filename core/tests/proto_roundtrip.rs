//! Confirms the wire format survives a round trip and that byte payloads stay
//! compact — the whole reason for choosing MessagePack over JSON.

use beebox_core::proto::*;

#[test]
fn output_frame_roundtrips_and_stays_compact() {
    let payload = vec![0xE4u8; 4096]; // 4KB of raw terminal bytes
    let frame = Out::Output { pty: 7, data: payload.clone() };

    let encoded = rmp_serde::to_vec_named(&frame).unwrap();
    // Framing overhead only. Without `serde_bytes` this measures +100%: serde
    // encodes Vec<u8> as an array of integers, which is worse than the
    // JSON+base64 MessagePack was chosen to avoid.
    assert!(encoded.len() < payload.len() + 64, "encoded {} bytes", encoded.len());

    match rmp_serde::from_slice::<Out>(&encoded).unwrap() {
        Out::Output { pty, data } => {
            assert_eq!(pty, 7);
            assert_eq!(data, payload);
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn nested_layout_roundtrips() {
    // The prototype's layout: one pane beside a stack of two.
    let layout = Node::Split {
        dir: Dir::Vertical,
        sizes: vec![0.5, 0.5],
        children: vec![
            Node::Leaf { pane: 1 },
            Node::Split {
                dir: Dir::Horizontal,
                sizes: vec![0.5, 0.5],
                children: vec![Node::Leaf { pane: 2 }, Node::Leaf { pane: 3 }],
            },
        ],
    };

    let bytes = rmp_serde::to_vec_named(&layout).unwrap();
    let back: Node = rmp_serde::from_slice(&bytes).unwrap();

    let Node::Split { children, dir, .. } = back else { panic!("expected split") };
    assert_eq!(dir, Dir::Vertical);
    assert!(matches!(children[1], Node::Split { .. }));
}

#[test]
fn input_names_its_pane_explicitly() {
    // No implicit "current pane" on the server: two viewers can focus
    // different panes without fighting.
    let msg = In::Input { pane: 3, data: b"ls\r".to_vec() };
    let bytes = rmp_serde::to_vec_named(&msg).unwrap();

    match rmp_serde::from_slice::<In>(&bytes).unwrap() {
        In::Input { pane, data } => {
            assert_eq!(pane, 3);
            assert_eq!(data, b"ls\r");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn resync_carries_mode_prefix_and_offset() {
    // Replay must re-establish alt screen / bracketed paste / app cursor keys,
    // which a raw ring tail would have lost.
    let frame = Out::Resync {
        pty: 2,
        modes: b"\x1b[?1049h\x1b[?2004h".to_vec(),
        data: b"partial output".to_vec(),
        through: 1_048_576,
    };
    let bytes = rmp_serde::to_vec_named(&frame).unwrap();

    match rmp_serde::from_slice::<Out>(&bytes).unwrap() {
        Out::Resync { pty, modes, through, .. } => {
            assert_eq!(pty, 2);
            assert_eq!(modes, b"\x1b[?1049h\x1b[?2004h");
            assert_eq!(through, 1_048_576);
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn exited_pane_keeps_its_identity() {
    // A dead process leaves the pane in place so it can be re-run; `PaneId`
    // must therefore be distinct from `PtyId`.
    let view = PaneView {
        id: 5,
        pty: None,
        title: "npm run dev".into(),
        agent: Some(AgentKind::Claude),
        status: AgentStatusView {
            phase: AgentPhase::Success,
            agent: Some(AgentKind::Claude),
            revision: 9,
            at_ms: 1_789_920_000_123,
            started_at_ms: Some(1_789_919_990_000),
            tool_detail: None,
            summary: Some("Refactored the auth middleware.".into()),
        },
        session_title: Some("Fix auth middleware".into()),
        cwd: "/repo/svc".into(),
        git: Some(GitInfo { branch: "main".into(), added: 3, modified: 1 }),
        cols: 96,
        rows: 38,
    };
    let bytes = rmp_serde::to_vec_named(&view).unwrap();
    let back: PaneView = rmp_serde::from_slice(&bytes).unwrap();

    assert_eq!(back.id, 5);
    assert!(back.pty.is_none());
    assert_eq!(back.git.unwrap().added, 3);
    assert_eq!(back.status.phase, AgentPhase::Success);
    assert_eq!(back.status.revision, 9);
    assert_eq!(back.session_title.as_deref(), Some("Fix auth middleware"));
}

#[test]
fn agent_status_delta_roundtrips_with_the_full_view() {
    // Live deltas carry the whole view, not just the phase, so a client can
    // rebuild tooltip and read-state without waiting for a tree resend.
    let frame = Out::Status {
        pane: 3,
        status: AgentStatusView {
            phase: AgentPhase::NeedsInput,
            agent: Some(AgentKind::Claude),
            revision: 4,
            at_ms: 1000,
            started_at_ms: Some(900),
            tool_detail: Some("Edit src/auth.rs".into()),
            summary: None,
        },
    };
    let bytes = rmp_serde::to_vec_named(&frame).unwrap();
    match rmp_serde::from_slice::<Out>(&bytes).unwrap() {
        Out::Status { pane, status } => {
            assert_eq!(pane, 3);
            assert_eq!(status.phase, AgentPhase::NeedsInput);
            assert_eq!(status.tool_detail.as_deref(), Some("Edit src/auth.rs"));
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn phase_and_kind_tags_match_the_typescript_mirror() {
    // The TS mirror hard-codes these strings; a serde rename here must fail
    // loudly rather than at runtime in the browser.
    let json = serde_json::to_string(&AgentPhase::NeedsInput).unwrap();
    assert_eq!(json, r#""needs_input""#);
    assert_eq!(serde_json::to_string(&AgentPhase::NeverRan).unwrap(), r#""never_ran""#);
    assert_eq!(serde_json::to_string(&AgentKind::Claude).unwrap(), r#""claude""#);
    assert_eq!(serde_json::to_string(&AgentKind::Opencode).unwrap(), r#""opencode""#);
    assert_eq!(serde_json::to_string(&AgentKind::Codex).unwrap(), r#""codex""#);
}
