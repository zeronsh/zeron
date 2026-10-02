use super::*;
use crate::client_ffi::types::ChatIndicator;

fn ffi_request() -> SendRequest {
    SendRequest {
        text: "ship it".into(),
        attachments: vec![OutgoingAttachment {
            name: "shot.png".into(),
            mime_type: "image/png".into(),
            data: vec![0x89, b'P', b'N', b'G'],
        }],
        worktree: Some(WorktreeSpec {
            repo_path: "/repo".into(),
            base: "origin/main".into(),
            space_id: Some("space-1".into()),
        }),
        busy: BusyPolicy::Steer,
    }
}

fn client_lease() -> zc::QueueEditLease {
    zc::QueueEditLease {
        row_id: "row-1".into(),
        lease_id: "lease-1".into(),
        text: "draft".into(),
        base_text_hash: "abc123".into(),
        expires_at_ms: 1_700_000_030_000,
    }
}

fn client_composer() -> zc::ComposerState {
    zc::ComposerState {
        chat_id: "chat-1".into(),
        revision: 9,
        title: "New session".into(),
        host: zc::HostInfo {
            device_id: "mac-1".into(),
            name: None,
            online: false,
            capabilities: zc::HostCapabilities::default(),
        },
        live: zc::LiveStatus {
            indicator: zc::ChatIndicator::Idle,
            turn_running: false,
            working_since_ms: None,
            streaming: false,
            can_interrupt: false,
        },
        queue: Vec::new(),
        pending_sends: Vec::new(),
        send_state: None,
        delivery_degraded: false,
        room: zc::RoomState::default(),
        transfer_progress: None,
        open_input: None,
        queue_error: None,
        last_submitted_message_id: None,
        context_usage: None,
    }
}

fn client_queue_item(id: &str, gate: Option<zc::QueueGate>) -> zc::QueueItem {
    zc::QueueItem {
        id: id.into(),
        text: "raw text".into(),
        visible_text: "text".into(),
        attachments: vec!["pending://u1/a.png".into()],
        hold_for_turn_end: true,
        issued_by: "ios-1".into(),
        from_this_device: true,
        issued_at_ms: 100,
        edited_at_ms: Some(200),
        gate,
        action_pending: true,
    }
}

// ── send ──────────────────────────────────────────────────────────────────

#[test]
fn send_request_keeps_every_field() {
    let client: zc::SendRequest = ffi_request().into();
    assert_eq!(client.text, "ship it");
    assert_eq!(client.busy, zc::BusyPolicy::Steer);
    assert_eq!(client.attachments.len(), 1);
    let attachment = &client.attachments[0];
    assert_eq!(attachment.name, "shot.png");
    assert_eq!(attachment.mime_type, "image/png");
    assert_eq!(attachment.data, [0x89, b'P', b'N', b'G']);
    let worktree = client.worktree.unwrap();
    assert_eq!(worktree.repo_path, "/repo");
    assert_eq!(worktree.base, "origin/main");
    assert_eq!(worktree.space_id.as_deref(), Some("space-1"));
}

#[test]
fn plain_send_request_matches_the_clients_text_only_default() {
    let client: zc::SendRequest = SendRequest {
        text: "hi".into(),
        attachments: Vec::new(),
        worktree: None,
        busy: BusyPolicy::Queue,
    }
    .into();
    assert_eq!(client, zc::SendRequest::text("hi"));
}

#[test]
fn send_request_worktree_without_a_project_stays_optional() {
    let client: zc::SendRequest = SendRequest {
        worktree: Some(WorktreeSpec {
            repo_path: "/repo".into(),
            base: "main".into(),
            space_id: None,
        }),
        ..ffi_request()
    }
    .into();
    assert_eq!(client.worktree.unwrap().space_id, None);
}

#[test]
fn worktree_spec_from_an_older_client_decodes_without_space_id() {
    // `space_id` was added later; a frame without it must still parse.
    let spec: zc::WorktreeSpec =
        serde_json::from_str(r#"{"repoPath":"/repo","base":"main"}"#).unwrap();
    assert_eq!(spec.space_id, None);
}

#[test]
fn multiple_attachments_keep_their_order() {
    let client: zc::SendRequest = SendRequest {
        attachments: ["a.png", "b.png", "c.png"]
            .into_iter()
            .map(|name| OutgoingAttachment {
                name: name.into(),
                mime_type: "image/png".into(),
                data: name.as_bytes().to_vec(),
            })
            .collect(),
        ..ffi_request()
    }
    .into();
    let names: Vec<_> = client.attachments.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["a.png", "b.png", "c.png"]);
    assert_eq!(client.attachments[2].data, b"c.png");
}

#[test]
fn oversized_attachment_reaches_the_client_intact() {
    // The 24 MB cap is enforced in `zeron-client` (`stage_attachments`); the
    // FFI layer must hand it the untouched byte count, not truncate or drop,
    // or an over-limit file would be silently sent clipped.
    let limit = zc::attachments::MAX_ATTACHMENT_BYTES;
    for len in [limit, limit + 1] {
        let client: zc::SendRequest = SendRequest {
            attachments: vec![OutgoingAttachment {
                name: "big.png".into(),
                mime_type: "image/png".into(),
                data: vec![0; len],
            }],
            ..ffi_request()
        }
        .into();
        assert_eq!(client.attachments[0].data.len(), len);
    }
    let empty: zc::SendRequest = SendRequest {
        attachments: vec![OutgoingAttachment {
            name: "empty.png".into(),
            mime_type: "image/png".into(),
            data: Vec::new(),
        }],
        ..ffi_request()
    }
    .into();
    assert!(empty.attachments[0].data.is_empty());
}

#[test]
fn send_outcomes_keep_their_ids() {
    assert_eq!(
        SendOutcome::from(zc::SendOutcome::Started {
            message_id: "m".into(),
        }),
        SendOutcome::Started {
            message_id: "m".into(),
        }
    );
    assert_eq!(
        SendOutcome::from(zc::SendOutcome::Steered {
            message_id: "m".into(),
        }),
        SendOutcome::Steered {
            message_id: "m".into(),
        }
    );
    assert_eq!(
        SendOutcome::from(zc::SendOutcome::Queued {
            queue_id: "q".into(),
        }),
        SendOutcome::Queued {
            queue_id: "q".into(),
        }
    );
}

// ── queue edit lease ──────────────────────────────────────────────────────

#[test]
fn queue_edit_lease_round_trips_without_losing_the_hash_or_expiry() {
    let ffi = QueueEditLease::from(client_lease());
    assert_eq!(ffi.row_id, "row-1");
    assert_eq!(ffi.lease_id, "lease-1");
    assert_eq!(ffi.text, "draft");
    // The host rejects a commit whose base hash drifted, so both must survive
    // the trip through the platform untouched.
    assert_eq!(ffi.base_text_hash, "abc123");
    assert_eq!(ffi.expires_at_ms, 1_700_000_030_000);
    assert_eq!(zc::QueueEditLease::from(ffi), client_lease());
}

#[test]
fn queue_edit_lease_text_with_unicode_and_newlines_is_preserved() {
    let text = "line one\n\u{1F680} line two\r\n\ttabbed";
    let lease = zc::QueueEditLease {
        text: text.into(),
        ..client_lease()
    };
    assert_eq!(
        zc::QueueEditLease::from(QueueEditLease::from(lease)).text,
        text
    );
}

// ── composer state ────────────────────────────────────────────────────────

#[test]
fn bare_composer_state_converts_with_every_optional_absent() {
    // A brand-new chat on a host that advertises no capabilities.
    let ffi = ComposerState::from(&client_composer());
    assert_eq!(ffi.chat_id, "chat-1");
    assert_eq!(ffi.revision, 9);
    assert_eq!(ffi.host.name, None);
    assert_eq!(
        ffi.host.capabilities,
        HostCapabilities {
            message_queue: false,
            queue_actions: false,
            queue_attachments: false,
            clean_attachment_text: false,
            queue_edit_lease: false,
            queued_attachments: false,
            mid_turn_steering: None,
        }
    );
    assert_eq!(ffi.live.indicator, ChatIndicator::Idle);
    assert!(ffi.queue.is_empty() && ffi.pending_sends.is_empty());
    assert!(ffi.send_state.is_none() && ffi.open_input.is_none());
    assert!(ffi.context_usage.is_none() && ffi.transfer_progress.is_none());
    assert!(ffi.queue_error.is_none() && ffi.last_submitted_message_id.is_none());
}

#[test]
fn host_capabilities_map_one_flag_at_a_time() {
    // Each flag flipped alone, so a swapped pair of adjacent bools (all
    // `bool`, all the same type) fails a test instead of shipping.
    type Setter = fn(&mut zc::HostCapabilities);
    type Getter = fn(&HostCapabilities) -> bool;
    let flags: [(Setter, Getter); 6] = [
        (|c| c.message_queue = true, |c| c.message_queue),
        (|c| c.queue_actions = true, |c| c.queue_actions),
        (|c| c.queue_attachments = true, |c| c.queue_attachments),
        (
            |c| c.clean_attachment_text = true,
            |c| c.clean_attachment_text,
        ),
        (|c| c.queue_edit_lease = true, |c| c.queue_edit_lease),
        (|c| c.queued_attachments = true, |c| c.queued_attachments),
    ];
    for (index, (set, _)) in flags.iter().enumerate() {
        let mut client = client_composer();
        set(&mut client.host.capabilities);
        let caps = ComposerState::from(&client).host.capabilities;
        for (other, (_, get)) in flags.iter().enumerate() {
            assert_eq!(get(&caps), other == index, "set {index}, read {other}");
        }
    }
    let mut client = client_composer();
    client.host.capabilities.mid_turn_steering = Some(false);
    assert_eq!(
        ComposerState::from(&client)
            .host
            .capabilities
            .mid_turn_steering,
        Some(false)
    );
}

#[test]
fn composer_state_with_everything_populated_converts() {
    let mut client = client_composer();
    client.host.name = Some("Mac".into());
    client.host.online = true;
    client.live = zc::LiveStatus {
        indicator: zc::ChatIndicator::Working,
        turn_running: true,
        working_since_ms: Some(42),
        streaming: true,
        can_interrupt: true,
    };
    client.queue = vec![
        client_queue_item("q1", None),
        client_queue_item(
            "q2",
            Some(zc::QueueGate::Editing {
                owner_device_id: "ios-2".into(),
                expires_at_ms: 999,
                mine: false,
            }),
        ),
        client_queue_item(
            "q3",
            Some(zc::QueueGate::ReviewRequired {
                owner_device_id: "ios-3".into(),
            }),
        ),
    ];
    client.pending_sends = vec![zc::PendingSend {
        message_id: "m1".into(),
        text: "raw".into(),
        visible_text: "vis".into(),
        images: vec!["pending://u/a.png".into()],
        kind: zc::PendingKind::Steer,
        sent_at_ms: 55,
        state: zc::SendState::Failed,
    }];
    client.send_state = Some(zc::SendState::Failed);
    client.delivery_degraded = true;
    client.room = zc::RoomState {
        connected: true,
        retry_at_ms: Some(77),
        degraded: true,
    };
    client.transfer_progress = Some(0.25);
    client.open_input = Some(zc::InputRequest {
        entry_id: "e".into(),
        request_id: "r".into(),
        questions: vec![zc::UserInputQuestion {
            id: "qid".into(),
            header: "Pick".into(),
            question: "Which?".into(),
            options: vec!["a".into(), "b".into()],
            multi_select: true,
        }],
    });
    client.queue_error = Some("host said no".into());
    client.last_submitted_message_id = Some("m1".into());
    client.context_usage = Some(zc::ContextUsage {
        tokens: Some(1_000),
        window: None,
    });

    let ffi = ComposerState::from(&client);
    assert_eq!(ffi.host.name.as_deref(), Some("Mac"));
    assert_eq!(ffi.live.working_since_ms, Some(42));
    assert!(ffi.live.turn_running && ffi.live.streaming && ffi.live.can_interrupt);

    let ids: Vec<_> = ffi.queue.iter().map(|q| q.id.as_str()).collect();
    assert_eq!(ids, ["q1", "q2", "q3"]);
    let first = &ffi.queue[0];
    assert_eq!(first.text, "raw text");
    assert_eq!(first.visible_text, "text");
    assert_eq!(first.attachments, ["pending://u1/a.png"]);
    assert!(first.hold_for_turn_end && first.from_this_device && first.action_pending);
    assert_eq!((first.issued_at_ms, first.edited_at_ms), (100, Some(200)));
    assert_eq!(first.gate, None);
    assert_eq!(
        ffi.queue[1].gate,
        Some(QueueGate::Editing {
            owner_device_id: "ios-2".into(),
            expires_at_ms: 999,
            mine: false,
        })
    );
    assert_eq!(
        ffi.queue[2].gate,
        Some(QueueGate::ReviewRequired {
            owner_device_id: "ios-3".into(),
        })
    );

    let pending = &ffi.pending_sends[0];
    assert_eq!(pending.kind, PendingKind::Steer);
    assert_eq!(pending.state, SendState::Failed);
    assert_eq!(pending.images, ["pending://u/a.png"]);
    assert_eq!(ffi.send_state, Some(SendState::Failed));
    assert!(ffi.delivery_degraded);
    assert_eq!(
        ffi.room,
        RoomState {
            connected: true,
            retry_at_ms: Some(77),
            degraded: true,
        }
    );
    assert_eq!(ffi.transfer_progress, Some(0.25));
    let input = ffi.open_input.unwrap();
    assert_eq!(
        (input.entry_id.as_str(), input.request_id.as_str()),
        ("e", "r")
    );
    assert_eq!(input.questions[0].options, ["a", "b"]);
    assert!(input.questions[0].multi_select);
    assert_eq!(ffi.queue_error.as_deref(), Some("host said no"));
    assert_eq!(ffi.last_submitted_message_id.as_deref(), Some("m1"));
    assert_eq!(
        ffi.context_usage,
        Some(ContextUsage {
            tokens: Some(1_000),
            window: None,
        })
    );
}

#[test]
fn pending_send_kinds_and_states_map() {
    for (client_kind, kind) in [
        (zc::PendingKind::Run, PendingKind::Run),
        (zc::PendingKind::Steer, PendingKind::Steer),
    ] {
        for (client_state, state) in [
            (zc::SendState::Sending, SendState::Sending),
            (zc::SendState::Queued, SendState::Queued),
            (zc::SendState::Failed, SendState::Failed),
        ] {
            let ffi = PendingSend::from(&zc::PendingSend {
                message_id: "m".into(),
                text: "t".into(),
                visible_text: "t".into(),
                images: Vec::new(),
                kind: client_kind,
                sent_at_ms: 1,
                state: client_state,
            });
            assert_eq!((ffi.kind, ffi.state), (kind, state));
        }
    }
}
