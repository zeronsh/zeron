use super::*;
use zeron_proto::{HarnessId, ReasoningLevel};

// Exhaustive on purpose: a new harness or effort level breaks the build
// here, so whoever adds it decides what the phone should do with it
// instead of finding out from a user's "unknown harness" error.
fn harness_wire(h: HarnessId) -> &'static str {
    match h {
        HarnessId::ClaudeCode => "claude-code",
        HarnessId::Codex => "codex",
        HarnessId::Cursor => "cursor",
        HarnessId::Devin => "devin",
        HarnessId::Grok => "grok",
        HarnessId::Hermes => "hermes",
        HarnessId::Pi => "pi",
        HarnessId::Opencode => "opencode",
        HarnessId::Antigravity => "antigravity",
        HarnessId::Mock => "mock",
    }
}

const ALL_HARNESSES: [HarnessId; 10] = [
    HarnessId::ClaudeCode,
    HarnessId::Codex,
    HarnessId::Cursor,
    HarnessId::Devin,
    HarnessId::Grok,
    HarnessId::Hermes,
    HarnessId::Pi,
    HarnessId::Opencode,
    HarnessId::Antigravity,
    HarnessId::Mock,
];

fn reasoning_wire(r: ReasoningLevel) -> &'static str {
    match r {
        ReasoningLevel::Minimal => "minimal",
        ReasoningLevel::Low => "low",
        ReasoningLevel::Medium => "medium",
        ReasoningLevel::High => "high",
        ReasoningLevel::XHigh => "xhigh",
        ReasoningLevel::Max => "max",
        ReasoningLevel::Ultra => "ultra",
        ReasoningLevel::Ultracode => "ultracode",
        ReasoningLevel::Ultrathink => "ultrathink",
    }
}

const ALL_REASONING: [ReasoningLevel; 9] = [
    ReasoningLevel::Minimal,
    ReasoningLevel::Low,
    ReasoningLevel::Medium,
    ReasoningLevel::High,
    ReasoningLevel::XHigh,
    ReasoningLevel::Max,
    ReasoningLevel::Ultra,
    ReasoningLevel::Ultracode,
    ReasoningLevel::Ultrathink,
];

fn ffi_config() -> ChatConfig {
    ChatConfig {
        harness: "claude-code".into(),
        model: Some("opus".into()),
        reasoning: Some("xhigh".into()),
        model_options: HashMap::from([("contextWindow".to_owned(), "1m".to_owned())]),
        sandbox: SandboxLevel::WorkspaceWrite,
    }
}

fn invalid_argument(err: CoreError) -> String {
    match err {
        CoreError::InvalidArgument { message } => message,
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

fn client_row(id: &str) -> zc::SessionRow {
    zc::SessionRow {
        id: id.into(),
        revision: 7,
        title: "New session".into(),
        has_title: false,
        preview: None,
        project: None,
        device_id: "dev-1".into(),
        device_name: None,
        device_online: false,
        harness: None,
        harness_label: None,
        model: None,
        model_label: None,
        reasoning: None,
        branch: None,
        cwd: None,
        indicator: zc::ChatIndicator::Idle,
        host_indicator: zc::ChatIndicator::Idle,
        working_since_ms: None,
        last_activity_ms: 1_000,
        time_label: "now".into(),
        created_at_ms: 900,
        unseen: false,
        archived: false,
        pinned: false,
        section_id: None,
        pull_request: None,
        send_state: None,
        parent_chat_id: None,
        room_gen: 2,
    }
}

// ── chat config ───────────────────────────────────────────────────────────

#[test]
fn chat_config_round_trips_through_the_client_type() {
    let client: zc::ChatConfig = ffi_config().try_into().unwrap();
    assert_eq!(client.harness, HarnessId::ClaudeCode);
    assert_eq!(client.model.as_deref(), Some("opus"));
    assert_eq!(client.reasoning, Some(ReasoningLevel::XHigh));
    assert_eq!(client.model_options["contextWindow"], "1m");
    assert_eq!(ChatConfig::from(&client), ffi_config());
}

#[test]
fn every_sandbox_level_survives_both_directions() {
    for (ffi, proto) in [
        (SandboxLevel::ReadOnly, zeron_proto::SandboxLevel::ReadOnly),
        (
            SandboxLevel::WorkspaceWrite,
            zeron_proto::SandboxLevel::WorkspaceWrite,
        ),
        (
            SandboxLevel::DangerFullAccess,
            zeron_proto::SandboxLevel::DangerFullAccess,
        ),
    ] {
        let client: zc::ChatConfig = ChatConfig {
            sandbox: ffi,
            ..ffi_config()
        }
        .try_into()
        .unwrap();
        assert_eq!(client.sandbox, proto);
        assert_eq!(ChatConfig::from(&client).sandbox, ffi);
    }
}

#[test]
fn every_known_harness_and_effort_id_round_trips() {
    for harness in ALL_HARNESSES {
        let client: zc::ChatConfig = ChatConfig {
            harness: harness_wire(harness).into(),
            ..ffi_config()
        }
        .try_into()
        .unwrap();
        assert_eq!(client.harness, harness);
        assert_eq!(ChatConfig::from(&client).harness, harness_wire(harness));
    }
    for level in ALL_REASONING {
        let client: zc::ChatConfig = ChatConfig {
            reasoning: Some(reasoning_wire(level).into()),
            ..ffi_config()
        }
        .try_into()
        .unwrap();
        assert_eq!(client.reasoning, Some(level));
        assert_eq!(
            ChatConfig::from(&client).reasoning.as_deref(),
            Some(reasoning_wire(level))
        );
    }
}

#[test]
fn absent_model_and_reasoning_stay_absent() {
    let client: zc::ChatConfig = ChatConfig {
        model: None,
        reasoning: None,
        model_options: HashMap::new(),
        ..ffi_config()
    }
    .try_into()
    .unwrap();
    assert_eq!(client.model, None);
    assert_eq!(client.reasoning, None);
    assert!(client.model_options.is_empty());
    let back = ChatConfig::from(&client);
    assert_eq!(back.model, None);
    assert_eq!(back.reasoning, None);
}

#[test]
fn unknown_harness_from_the_platform_is_an_error_not_a_panic() {
    // A newer desktop's harness can reach a picker via a live catalog
    // before this build knows it; writing it back must fail cleanly.
    let err = zc::ChatConfig::try_from(ChatConfig {
        harness: "quantum-agent".into(),
        ..ffi_config()
    })
    .unwrap_err();
    assert_eq!(invalid_argument(err), "unknown harness `quantum-agent`");
    // Wire ids are exact: no case folding, no empty default.
    for bad in ["Claude-Code", "claude_code", ""] {
        assert!(
            zc::ChatConfig::try_from(ChatConfig {
                harness: bad.into(),
                ..ffi_config()
            })
            .is_err(),
            "{bad:?}"
        );
    }
}

#[test]
fn unknown_reasoning_level_is_an_error_not_a_panic() {
    let err = zc::ChatConfig::try_from(ChatConfig {
        reasoning: Some("extreme".into()),
        ..ffi_config()
    })
    .unwrap_err();
    assert_eq!(invalid_argument(err), "unknown reasoning level `extreme`");
}

#[test]
fn model_ids_are_opaque_so_unknown_models_pass_through() {
    let client: zc::ChatConfig = ChatConfig {
        model: Some("gpt-9-turbo-preview".into()),
        ..ffi_config()
    }
    .try_into()
    .unwrap();
    assert_eq!(client.model.as_deref(), Some("gpt-9-turbo-preview"));
}

#[test]
fn non_string_model_options_from_a_newer_peer_are_stringified() {
    let mut client: zc::ChatConfig = ffi_config().try_into().unwrap();
    client.model_options.clear();
    client.model_options.insert("thinking".into(), true.into());
    client.model_options.insert("budget".into(), 4096.into());
    client.model_options.insert("tier".into(), "fast".into());
    let ffi = ChatConfig::from(&client);
    assert_eq!(ffi.model_options["thinking"], "true");
    assert_eq!(ffi.model_options["budget"], "4096");
    assert_eq!(ffi.model_options["tier"], "fast");
    // Writing the config back always sends strings, so a non-string
    // option changes type on a round trip through the phone.
    let back: zc::ChatConfig = ffi.try_into().unwrap();
    assert_eq!(back.model_options["thinking"], "true");
    assert_ne!(back.model_options, client.model_options);
}

#[test]
fn catalog_ids_the_pickers_offer_are_accepted_by_set_session_config() {
    // The picker fills a ChatConfig from `fallback_*`/live catalogs and the
    // platform writes it back through `TryFrom`; a level or harness id the
    // catalog offers but the proto enum lacks would fail every save.
    let mut harnesses: Vec<String> = zc::catalog::fallback_harnesses()
        .into_iter()
        .map(|h| h.id)
        .collect();
    harnesses.extend(ALL_HARNESSES.map(|h| harness_wire(h).to_owned()));
    for harness in harnesses {
        from_wire::<HarnessId>(&harness, "harness").unwrap_or_else(|_| panic!("{harness}"));
        for model in zc::catalog::fallback_models(&harness) {
            for level in &model.reasoning_levels {
                from_wire::<ReasoningLevel>(level, "reasoning level")
                    .unwrap_or_else(|_| panic!("{harness}/{}: {level}", model.id));
            }
            if let Some(default) = zc::catalog::default_reasoning(&model) {
                assert!(model.reasoning_levels.contains(&default));
            }
        }
    }
}

// ── new session ───────────────────────────────────────────────────────────

#[test]
fn new_session_maps_both_targets_and_optional_fields() {
    let project = zc::NewSession::try_from(NewSession {
        target: SessionTarget::Project {
            space_id: "space-1".into(),
        },
        config: Some(ffi_config()),
        branch: Some("main".into()),
        cwd: Some("/work/tree".into()),
        title: Some("Fix it".into()),
    })
    .unwrap();
    assert!(matches!(
        project.target,
        zc::SessionTarget::Project { ref space_id } if space_id == "space-1"
    ));
    assert_eq!(project.config.unwrap().harness, HarnessId::ClaudeCode);
    assert_eq!(project.branch.as_deref(), Some("main"));
    assert_eq!(project.cwd.as_deref(), Some("/work/tree"));
    assert_eq!(project.title.as_deref(), Some("Fix it"));

    let bare = zc::NewSession::try_from(NewSession {
        target: SessionTarget::Projectless {
            device_id: "mac-1".into(),
        },
        config: None,
        branch: None,
        cwd: None,
        title: None,
    })
    .unwrap();
    assert!(matches!(
        bare.target,
        zc::SessionTarget::Projectless { ref device_id } if device_id == "mac-1"
    ));
    assert!(bare.config.is_none() && bare.branch.is_none());
    assert!(bare.cwd.is_none() && bare.title.is_none());
}

#[test]
fn new_session_surfaces_a_bad_config_instead_of_dropping_it() {
    let err = zc::NewSession::try_from(NewSession {
        target: SessionTarget::Projectless {
            device_id: "mac-1".into(),
        },
        config: Some(ChatConfig {
            harness: "nope".into(),
            ..ffi_config()
        }),
        branch: None,
        cwd: None,
        title: None,
    })
    .unwrap_err();
    assert_eq!(invalid_argument(err), "unknown harness `nope`");
}

// ── errors / config / credentials ─────────────────────────────────────────

#[test]
fn every_client_error_keeps_its_kind_and_message() {
    use zc::ClientError as E;
    let m = || "m".to_owned();
    let cases = [
        (E::NotFound(m()), CoreError::NotFound { message: m() }),
        (
            E::InvalidArgument(m()),
            CoreError::InvalidArgument { message: m() },
        ),
        (
            E::HostUnavailable(m()),
            CoreError::HostUnavailable { message: m() },
        ),
        (E::Unsupported(m()), CoreError::Unsupported { message: m() }),
        (E::Network(m()), CoreError::Network { message: m() }),
        (E::HostError(m()), CoreError::HostError { message: m() }),
        (E::Auth(m()), CoreError::Auth { message: m() }),
        (E::Storage(m()), CoreError::Storage { message: m() }),
        (
            E::NotImplemented(m()),
            CoreError::NotImplemented { message: m() },
        ),
        (E::Closed, CoreError::Closed),
        (E::Internal(m()), CoreError::Internal { message: m() }),
    ];
    for (client, ffi) in cases {
        assert_eq!(CoreError::from(client), ffi);
    }
}

#[test]
fn core_config_and_credentials_map_field_for_field() {
    let config: zc::ClientConfig = CoreConfig {
        edge_url: "https://edge.example".into(),
        data_dir: "/tmp/zeron-test".into(),
        device_id: "ios-abcd1234".into(),
        device_name: "Test iPhone".into(),
        platform: "ios".into(),
        app_version: "1.2.3".into(),
    }
    .into();
    assert_eq!(config.edge_url, "https://edge.example");
    assert_eq!(config.data_dir, std::path::PathBuf::from("/tmp/zeron-test"));
    assert_eq!(config.device_id, "ios-abcd1234");
    assert_eq!(config.device_name, "Test iPhone");
    assert_eq!(config.platform, "ios");
    assert_eq!(config.app_version, "1.2.3");

    let tokens = AuthTokens {
        access_token: "a".into(),
        refresh_token: "r".into(),
    };
    assert_eq!(
        zc::Credentials::from(Credentials::WorkOs {
            user_id: "u".into(),
            org_id: "o".into(),
            tokens: tokens.clone(),
        }),
        zc::Credentials::WorkOs {
            user_id: "u".into(),
            org_id: "o".into(),
            tokens: zc::AuthTokens::from(tokens),
        }
    );
    assert_eq!(
        zc::Credentials::from(Credentials::Dev {
            user_id: "u".into(),
            org_id: "o".into(),
        }),
        zc::Credentials::Dev {
            user_id: "u".into(),
            org_id: "o".into(),
        }
    );
}

#[test]
fn demo_options_map_every_fixture_and_transcript_scale() {
    let map = |fixture, transcript_scale| {
        zc::DemoOptions::from(DemoOptions {
            fixture,
            transcript_scale,
            stream_speed: StreamSpeed::Fast,
            long_reply: true,
        })
    };
    let opts = map(
        DemoFixture::NoProjects,
        TranscriptScale::Turns { count: 42 },
    );
    assert_eq!(opts.fixture, zc::DemoFixture::NoProjects);
    assert_eq!(opts.transcript_scale, zc::TranscriptScale::Turns(42));
    assert_eq!(opts.stream_speed, zc::StreamSpeed::Fast);
    assert!(opts.long_reply);
    assert_eq!(
        map(DemoFixture::IosOnly, TranscriptScale::Huge).transcript_scale,
        zc::TranscriptScale::Huge
    );
    assert_eq!(
        map(DemoFixture::IosOnly, TranscriptScale::Big).fixture,
        zc::DemoFixture::IosOnly
    );
    let standard = map(DemoFixture::Standard, TranscriptScale::Normal);
    assert_eq!(standard.fixture, zc::DemoFixture::Standard);
    assert_eq!(standard.transcript_scale, zc::TranscriptScale::Normal);
    assert_eq!(
        zc::DemoOptions::from(DemoOptions {
            fixture: DemoFixture::Standard,
            transcript_scale: TranscriptScale::Big,
            stream_speed: StreamSpeed::Realistic,
            long_reply: false,
        })
        .stream_speed,
        zc::StreamSpeed::Realistic
    );
}

// ── events / connectivity ─────────────────────────────────────────────────

#[test]
fn client_events_convert_variant_for_variant() {
    assert_eq!(
        ClientEvent::from(zc::ClientEvent::WorkspaceChanged { revision: 3 }),
        ClientEvent::WorkspaceChanged { revision: 3 }
    );
    assert_eq!(
        ClientEvent::from(zc::ClientEvent::SessionChanged {
            chat_id: "c".into(),
            revision: 4,
        }),
        ClientEvent::SessionChanged {
            chat_id: "c".into(),
            revision: 4,
        }
    );
    assert_eq!(
        ClientEvent::from(zc::ClientEvent::ComposerChanged {
            chat_id: "c".into(),
            revision: 5,
        }),
        ClientEvent::ComposerChanged {
            chat_id: "c".into(),
            revision: 5,
        }
    );
    assert_eq!(
        ClientEvent::from(zc::ClientEvent::AuthRefreshed(zc::AuthTokens {
            access_token: "a2".into(),
            refresh_token: "r2".into(),
        })),
        ClientEvent::AuthRefreshed {
            tokens: AuthTokens {
                access_token: "a2".into(),
                refresh_token: "r2".into(),
            },
        }
    );
    assert_eq!(
        ClientEvent::from(zc::ClientEvent::AuthExpired {
            reason: "revoked".into(),
        }),
        ClientEvent::AuthExpired {
            reason: "revoked".into(),
        }
    );
    let connectivity = ClientEvent::from(zc::ClientEvent::ConnectivityChanged(zc::Connectivity {
        state: zc::ConnectivityState::Reconnecting,
        retry_at_ms: Some(9_000),
        last_failure: Some("relay closed".into()),
        degraded_chats: vec!["c1".into()],
    }));
    assert_eq!(
        connectivity,
        ClientEvent::ConnectivityChanged {
            connectivity: Connectivity {
                state: ConnectivityState::Reconnecting,
                retry_at_ms: Some(9_000),
                last_failure: Some("relay closed".into()),
                degraded_chats: vec!["c1".into()],
            },
        }
    );
}

#[test]
fn connectivity_states_and_send_states_do_not_cross_wires() {
    for (client, ffi) in [
        (zc::ConnectivityState::Disabled, ConnectivityState::Disabled),
        (zc::ConnectivityState::Offline, ConnectivityState::Offline),
        (
            zc::ConnectivityState::Reconnecting,
            ConnectivityState::Reconnecting,
        ),
        (
            zc::ConnectivityState::Connected,
            ConnectivityState::Connected,
        ),
    ] {
        let converted = Connectivity::from(zc::Connectivity {
            state: client,
            ..Default::default()
        });
        assert_eq!(converted.state, ffi);
        assert_eq!(converted.retry_at_ms, None);
        assert!(converted.degraded_chats.is_empty());
    }
    assert_eq!(SendState::from(zc::SendState::Sending), SendState::Sending);
    assert_eq!(SendState::from(zc::SendState::Queued), SendState::Queued);
    assert_eq!(SendState::from(zc::SendState::Failed), SendState::Failed);
}

// ── workspace ─────────────────────────────────────────────────────────────

#[test]
fn session_row_with_every_optional_field_absent_converts() {
    // The shape of a freshly created chat from an older peer: no title,
    // project, harness, model, branch or PR.
    let row = SessionRow::from(&client_row("c1"));
    assert_eq!(row.id, "c1");
    assert_eq!(row.revision, 7);
    assert!(!row.has_title);
    assert!(row.project.is_none() && row.pull_request.is_none());
    assert!(row.harness.is_none() && row.model.is_none() && row.reasoning.is_none());
    assert!(row.send_state.is_none() && row.parent_chat_id.is_none());
    assert_eq!(row.indicator, ChatIndicator::Idle);
    assert_eq!(row.room_gen, 2);
}

#[test]
fn session_row_with_every_optional_field_present_converts() {
    let mut client = client_row("c2");
    client.has_title = true;
    client.title = "Ship it".into();
    client.preview = Some("last words".into());
    client.project = Some(zc::ProjectRef {
        id: "space-1".into(),
        name: "zeron".into(),
        color_index: 3,
    });
    client.device_name = Some("Mac".into());
    client.device_online = true;
    client.harness = Some("codex".into());
    client.harness_label = Some("Codex".into());
    client.model = Some("gpt-x".into());
    client.model_label = Some("GPT X".into());
    client.reasoning = Some("ultra".into());
    client.branch = Some("feat".into());
    client.cwd = Some("/repo".into());
    client.indicator = zc::ChatIndicator::Working;
    client.host_indicator = zc::ChatIndicator::AwaitingInput;
    client.working_since_ms = Some(500);
    client.unseen = true;
    client.pinned = true;
    client.section_id = Some("sec".into());
    client.send_state = Some(zc::SendState::Queued);
    client.parent_chat_id = Some("parent".into());
    client.pull_request = Some(zc::ChangeRequestSummary {
        provider: "github".into(),
        number: 12,
        title: "PR".into(),
        url: "https://example.test/pr/12".into(),
        state: zc::ChangeRequestState::Merged,
        base_ref: "main".into(),
        head_ref: "feat".into(),
    });

    let row = SessionRow::from(&client);
    assert_eq!(row.project.unwrap().color_index, 3);
    // The override-vs-host split is what stop/busy logic keys off.
    assert_eq!(row.indicator, ChatIndicator::Working);
    assert_eq!(row.host_indicator, ChatIndicator::AwaitingInput);
    assert_eq!(row.send_state, Some(SendState::Queued));
    assert_eq!(row.reasoning.as_deref(), Some("ultra"));
    assert_eq!(row.working_since_ms, Some(500));
    assert_eq!(row.parent_chat_id.as_deref(), Some("parent"));
    let pr = row.pull_request.unwrap();
    assert_eq!((pr.number, pr.state), (12, PullRequestState::Merged));
    assert_eq!(
        (pr.base_ref.as_str(), pr.head_ref.as_str()),
        ("main", "feat")
    );
}

#[test]
fn every_change_request_and_indicator_state_maps() {
    for (client, ffi) in [
        (zc::ChangeRequestState::Open, PullRequestState::Open),
        (zc::ChangeRequestState::Merged, PullRequestState::Merged),
        (zc::ChangeRequestState::Closed, PullRequestState::Closed),
    ] {
        let mut row = client_row("c");
        row.pull_request = Some(zc::ChangeRequestSummary {
            provider: "github".into(),
            number: 1,
            title: String::new(),
            url: String::new(),
            state: client,
            base_ref: String::new(),
            head_ref: String::new(),
        });
        assert_eq!(SessionRow::from(&row).pull_request.unwrap().state, ffi);
    }
    for (client, ffi) in [
        (zc::ChatIndicator::Working, ChatIndicator::Working),
        (
            zc::ChatIndicator::AwaitingInput,
            ChatIndicator::AwaitingInput,
        ),
        (zc::ChatIndicator::Errored, ChatIndicator::Errored),
        (zc::ChatIndicator::Completed, ChatIndicator::Completed),
        (zc::ChatIndicator::Idle, ChatIndicator::Idle),
    ] {
        assert_eq!(ChatIndicator::from(client), ffi);
    }
}

#[test]
fn empty_workspace_snapshot_converts_to_empty_buckets() {
    let ffi = WorkspaceSnapshot::from(&zc::WorkspaceSnapshot::default());
    assert!(!ffi.synced && !ffi.pins_ready);
    assert!(ffi.front.pinned.is_empty() && ffi.front.sections.is_empty());
    assert!(ffi.front.recent.is_empty() && ffi.projects.is_empty());
    assert!(ffi.projectless.is_empty() && ffi.archived.is_empty());
    assert!(ffi.pull_requests.open.is_empty() && ffi.devices.is_empty());
}

#[test]
fn workspace_snapshot_keeps_rows_in_their_buckets_and_order() {
    let row = |id: &str| Arc::new(client_row(id));
    // Cache fields on the client snapshot are private, so start from Default.
    let mut snapshot = zc::WorkspaceSnapshot::default();
    snapshot.revision = 11;
    snapshot.synced = true;
    snapshot.pins_ready = true;
    snapshot.front = zc::FrontPage {
        pinned: vec![row("p1"), row("p2")],
        sections: vec![zc::SectionView {
            id: "s".into(),
            name: "Folder".into(),
            collapsed: true,
            sessions: vec![row("in-section")],
        }],
        recent: vec![row("r1")],
    };
    snapshot.projects = vec![zc::ProjectView {
        id: "space".into(),
        name: "zeron".into(),
        path: "/repo".into(),
        color_index: 1,
        device_id: "mac".into(),
        device_name: Some("Mac".into()),
        device_online: true,
        git_detected: true,
        created_at_ms: 5,
        indicator: zc::ChatIndicator::Completed,
        unseen_count: 2,
        sessions: vec![row("proj-chat")],
    }];
    snapshot.projectless = vec![row("loose")];
    snapshot.pull_requests = zc::PullRequestGroups {
        open: vec![row("open")],
        merged: vec![row("merged")],
        closed: vec![row("closed")],
    };
    snapshot.archived = vec![row("old")];
    snapshot.devices = vec![zc::DeviceView {
        id: "mac".into(),
        name: "Mac".into(),
        platform: "macos".into(),
        online: true,
        last_seen_ms: None,
        version: None,
        capabilities: vec!["queue-edit-lease".into()],
        is_execution_host: true,
        is_self: false,
        session_count: 4,
    }];
    let ids = |rows: &[SessionRow]| rows.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
    let ffi = WorkspaceSnapshot::from(&snapshot);
    assert_eq!(ffi.revision, 11);
    assert_eq!(ids(&ffi.front.pinned), ["p1", "p2"]);
    assert_eq!(ids(&ffi.front.sections[0].sessions), ["in-section"]);
    assert!(ffi.front.sections[0].collapsed);
    assert_eq!(ids(&ffi.front.recent), ["r1"]);
    assert_eq!(ids(&ffi.projects[0].sessions), ["proj-chat"]);
    assert_eq!(ffi.projects[0].indicator, ChatIndicator::Completed);
    assert_eq!(ids(&ffi.projectless), ["loose"]);
    assert_eq!(ids(&ffi.pull_requests.open), ["open"]);
    assert_eq!(ids(&ffi.pull_requests.merged), ["merged"]);
    assert_eq!(ids(&ffi.pull_requests.closed), ["closed"]);
    assert_eq!(ids(&ffi.archived), ["old"]);
    // A device with no last-seen/version (older peer) stays optional.
    assert_eq!(ffi.devices[0].last_seen_ms, None);
    assert_eq!(ffi.devices[0].capabilities, ["queue-edit-lease"]);
}

#[test]
fn search_hit_carries_the_matching_field() {
    for (client, ffi) in [
        (zc::SearchField::Title, SearchField::Title),
        (zc::SearchField::Project, SearchField::Project),
        (zc::SearchField::Branch, SearchField::Branch),
        (zc::SearchField::Preview, SearchField::Preview),
    ] {
        let hit = SearchHit::from(&zc::SearchHit {
            session: Arc::new(client_row("c")),
            score: 90,
            field: client,
        });
        assert_eq!(
            (hit.field, hit.score, hit.session.id.as_str()),
            (ffi, 90, "c")
        );
    }
}

// ── catalogs ──────────────────────────────────────────────────────────────

#[test]
fn harness_from_an_old_engine_reply_defaults_to_offered() {
    // Old engines send only `id` + `name` (aliased to `label`): installed
    // defaults to true and `enabled` to legacy-unknown, so it is offered.
    let old: zc::catalog::HarnessInfo =
        serde_json::from_str(r#"{"id":"claude-code","name":"Claude Code"}"#).unwrap();
    let ffi = HarnessInfo::from(old);
    assert_eq!(ffi.label, "Claude Code");
    assert!(ffi.installed && ffi.offered);
    assert_eq!(ffi.enabled, None);
    assert_eq!(ffi.mid_turn_steering, None);
    assert!(ffi.reasoning_levels.is_empty());
}

#[test]
fn harness_from_a_newer_engine_reply_ignores_unknown_fields() {
    let newer: zc::catalog::HarnessInfo = serde_json::from_str(
        r#"{"id":"quantum-agent","label":"Quantum","installed":true,"enabled":false,
            "supportsSteering":true,"steeringMode":"step-boundary",
            "reasoningLevels":["low","warp"],"somethingNew":{"a":1}}"#,
    )
    .unwrap();
    let ffi = HarnessInfo::from(newer);
    // Unknown ids and levels stay strings across the boundary.
    assert_eq!(ffi.id, "quantum-agent");
    assert_eq!(ffi.reasoning_levels, ["low", "warp"]);
    // Disabled on the device wins over installed.
    assert!(ffi.installed && !ffi.offered);
    assert_eq!(ffi.mid_turn_steering, Some(true));
}

#[test]
fn mid_turn_steering_is_unknown_until_the_catalog_says_so() {
    let mid_turn = |supports: Option<bool>, mode: Option<&str>| {
        HarnessInfo::from(zc::catalog::HarnessInfo {
            id: "h".into(),
            label: "H".into(),
            supports_steering: supports,
            steering_mode: mode.map(str::to_owned),
            reasoning_levels: Vec::new(),
            installed: true,
            enabled: None,
        })
        .mid_turn_steering
    };
    assert_eq!(mid_turn(None, None), None);
    assert_eq!(mid_turn(Some(true), None), None);
    assert_eq!(mid_turn(Some(true), Some("turn-boundary")), Some(false));
    assert_eq!(mid_turn(Some(true), Some("step-boundary")), Some(true));
    assert_eq!(mid_turn(Some(false), Some("step-boundary")), Some(false));
}

#[test]
fn not_installed_harness_is_not_offered() {
    let ffi = HarnessInfo::from(zc::catalog::HarnessInfo {
        id: "codex".into(),
        label: "Codex".into(),
        supports_steering: None,
        steering_mode: None,
        reasoning_levels: Vec::new(),
        installed: false,
        enabled: Some(true),
    });
    assert!(!ffi.offered);
}

#[test]
fn model_with_only_id_and_label_decodes_with_empty_extras() {
    let bare: zc::catalog::ModelInfo =
        serde_json::from_str(r#"{"id":"m","label":"M","futureField":1}"#).unwrap();
    let ffi = ModelInfo::from(bare);
    assert_eq!(ffi.description, None);
    assert!(ffi.reasoning_levels.is_empty() && ffi.options.is_empty());
    assert_eq!(ffi.default_reasoning, None);
}

#[test]
fn model_default_reasoning_prefers_high_then_medium_then_first() {
    let default_for = |levels: &[&str]| {
        ModelInfo::from(zc::catalog::ModelInfo {
            id: "m".into(),
            label: "M".into(),
            description: None,
            reasoning_levels: levels.iter().map(|l| (*l).to_owned()).collect(),
            options: Vec::new(),
        })
        .default_reasoning
    };
    assert_eq!(
        default_for(&["low", "high", "medium"]).as_deref(),
        Some("high")
    );
    assert_eq!(default_for(&["low", "medium"]).as_deref(), Some("medium"));
    assert_eq!(default_for(&["warp", "low"]).as_deref(), Some("warp"));
    assert_eq!(default_for(&[]), None);
}

#[test]
fn model_options_and_choices_are_preserved() {
    let ffi = ModelInfo::from(zc::catalog::ModelInfo {
        id: "opus".into(),
        label: "Opus".into(),
        description: Some("Big".into()),
        reasoning_levels: vec!["high".into()],
        options: vec![zc::catalog::ModelOption {
            id: "contextWindow".into(),
            label: "Context Window".into(),
            choices: vec![
                zc::catalog::ModelOptionChoice {
                    id: "200k".into(),
                    label: "200K".into(),
                },
                zc::catalog::ModelOptionChoice {
                    id: "1m".into(),
                    label: "1M".into(),
                },
            ],
            default_choice: "200k".into(),
        }],
    });
    assert_eq!(ffi.description.as_deref(), Some("Big"));
    let option = &ffi.options[0];
    assert_eq!(option.default_choice, "200k");
    assert_eq!(
        option
            .choices
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["200k", "1m"]
    );
}

// ── host RPC + attachments ────────────────────────────────────────────────

#[test]
fn repo_ref_round_trips_including_a_missing_worktree() {
    for worktree_path in [None, Some("/wt/feat".to_owned())] {
        let ffi = RepoRef {
            name: "feat".into(),
            current: worktree_path.is_none(),
            worktree_path,
        };
        let client: zc::rpc::RepoRef = ffi.clone().into();
        assert_eq!(RepoRef::from(client), ffi);
    }
}

#[test]
fn folder_listing_keeps_entry_flags_and_truncation() {
    let listing = FolderListing::from(zc::rpc::FolderListing {
        path: "/home".into(),
        entries: vec![
            zc::rpc::FolderEntry {
                name: "code".into(),
                is_dir: true,
                is_repo: true,
            },
            zc::rpc::FolderEntry {
                name: "notes.txt".into(),
                is_dir: false,
                is_repo: false,
            },
        ],
        truncated: true,
    });
    assert!(listing.truncated);
    assert_eq!(
        listing.entries[0],
        FolderEntry {
            name: "code".into(),
            is_dir: true,
            is_repo: true,
        }
    );
    assert!(!listing.entries[1].is_dir);
}

#[test]
fn parsed_user_message_splits_the_attachment_trailer() {
    let content =
        zc::attachments::with_attachments("look at this", &["/host/uploads/shot.png".to_owned()]);
    let parsed = ParsedUserMessage::from(zc::attachments::parse_user_message(&content));
    assert_eq!(parsed.text, "look at this");
    assert_eq!(parsed.images.len(), 1);
    assert_eq!(parsed.images[0].path, "/host/uploads/shot.png");
    assert_eq!(parsed.images[0].name, "shot.png");
    assert_eq!(parsed.images[0].appshot, None);

    // Attachment-only sends carry a placeholder body that must not show.
    let only = zc::attachments::with_attachments("  ", &["pending://u1/a.png".to_owned()]);
    let parsed = ParsedUserMessage::from(zc::attachments::parse_user_message(&only));
    assert_eq!(parsed.text, "");
    assert_eq!(parsed.images[0].path, "pending://u1/a.png");

    // Plain text has no images and passes through.
    let plain = ParsedUserMessage::from(zc::attachments::parse_user_message("hello"));
    assert_eq!((plain.text.as_str(), plain.images.len()), ("hello", 0));
}
