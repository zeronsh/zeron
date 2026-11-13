use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn row(id: &str, label: &str) -> Model {
    Model {
        default: false,
        id: id.into(),
        label: label.into(),
        description: None,
        reasoning_levels: vec![ReasoningLevel::High],
        options: vec![],
    }
}

/// A built-in style catalog: best-first, Opus preselected.
fn catalog() -> Vec<Model> {
    let mut models = vec![
        row("claude-fable-5-2", "Fable 5.2"),
        row("claude-opus-5-5", "Opus 5.5"),
        row("claude-sonnet-5-5", "Sonnet 5.5"),
        row("claude-haiku-4-5", "Haiku 4.5"),
    ];
    models[1].default = true;
    models
}

fn ids(models: &[Model]) -> Vec<&str> {
    models.iter().map(|m| &*m.id).collect()
}

fn hosted(models: serde_json::Value) -> HostedHarness {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "schema": 1, "harnesses": {"claude-code": {"models": models}}
    }))
    .unwrap();
    validate(&bytes)
        .unwrap()
        .harnesses
        .remove("claude-code")
        .unwrap()
}

fn default_of(models: &[Model]) -> Vec<&str> {
    models
        .iter()
        .filter(|m| m.default)
        .map(|m| &*m.id)
        .collect()
}

// ── merge rules ─────────────────────────────────────────────────────────

#[test]
fn an_empty_or_unrelated_list_changes_nothing() {
    for list in [
        hosted(serde_json::json!([])),
        hosted(serde_json::json!([{"id": "gateway/other", "label": "Other"}])),
    ] {
        assert_eq!(
            apply(catalog(), &list, Some("2.1.322"), Source::Cli),
            catalog()
        );
    }
}

#[test]
fn relabels_keep_what_the_row_launches() {
    let list = hosted(serde_json::json!([
        {"id": "claude-haiku-4-5", "label": " Haiku 4.5 (legacy) ", "description": "Older"}
    ]));
    let models = apply(catalog(), &list, None, Source::Cli);
    let haiku = models.iter().find(|m| m.id == "claude-haiku-4-5").unwrap();
    assert_eq!(haiku.label, "Haiku 4.5 (legacy)");
    assert_eq!(haiku.description.as_deref(), Some("Older"));
    assert_eq!(haiku.reasoning_levels, catalog()[3].reasoning_levels);
    assert_eq!(ids(&models), ids(&catalog()));
}

#[test]
fn order_moves_listed_rows_and_others_follow_their_neighbour() {
    let mut models = catalog();
    // A CLI-reported row the list doesn't name, right after Sonnet.
    models.insert(3, row("gateway/sol", "Sol"));
    let list = hosted(serde_json::json!([
        {"id": "claude-opus-5-5", "order": 10},
        {"id": "claude-fable-5-2", "order": 20},
        {"id": "claude-sonnet-5-5", "order": 30},
        {"id": "claude-haiku-4-5", "order": 40}
    ]));
    assert_eq!(
        ids(&apply(models, &list, None, Source::Cli)),
        [
            "claude-opus-5-5",
            "claude-fable-5-2",
            "claude-sonnet-5-5",
            "gateway/sol",
            "claude-haiku-4-5"
        ]
    );
}

#[test]
fn hidden_rows_go_but_never_the_preselected_one_or_every_row() {
    let list = hosted(serde_json::json!([
        {"id": "claude-haiku-4-5", "hidden": true},
        {"id": "claude-opus-5-5", "hidden": true}
    ]));
    let models = apply(catalog(), &list, None, Source::Cli);
    assert_eq!(
        ids(&models),
        ["claude-fable-5-2", "claude-opus-5-5", "claude-sonnet-5-5"]
    );
    let all = hosted(serde_json::json!([
        {"id": "a", "hidden": true}, {"id": "b", "hidden": true}
    ]));
    let only = vec![row("a", "A"), row("b", "B")];
    assert_eq!(apply(only.clone(), &all, None, Source::Cli), only);
}

#[test]
fn the_cli_default_wins_and_a_hosted_default_only_replaces_the_built_in_guess() {
    let list = hosted(serde_json::json!([{"id": "claude-fable-5-2", "default": true}]));
    assert_eq!(
        default_of(&apply(catalog(), &list, None, Source::Cli)),
        ["claude-opus-5-5"]
    );
    assert_eq!(
        default_of(&apply(catalog(), &list, None, Source::BuiltIn)),
        ["claude-fable-5-2"]
    );
    // A default for a row that isn't listed is ignored.
    let missing = hosted(serde_json::json!([{"id": "claude-nova-1", "default": true}]));
    assert_eq!(
        default_of(&apply(catalog(), &missing, None, Source::BuiltIn)),
        ["claude-opus-5-5"]
    );
}

#[test]
fn an_alias_row_folds_into_its_model_and_hands_over_the_default() {
    let mut models = catalog();
    models.push(row("claude-opus-5-5-20261101", "Opus snapshot"));
    for model in &mut models {
        model.default = model.id == "claude-opus-5-5-20261101";
    }
    let list = hosted(serde_json::json!([
        {"id": "claude-opus-5-5", "aliases": ["claude-opus-5-5-20261101"]}
    ]));
    let merged = apply(models, &list, None, Source::Cli);
    assert_eq!(ids(&merged), ids(&catalog()));
    assert_eq!(default_of(&merged), ["claude-opus-5-5"]);
    // With the model itself not listed, the alias row is all there is: kept.
    let only_alias = vec![row("claude-opus-5-5-20261101", "Opus snapshot")];
    assert_eq!(
        apply(only_alias.clone(), &list, None, Source::Cli),
        only_alias
    );
}

// ── the CLI-version gate ────────────────────────────────────────────────

#[test]
fn a_new_model_is_added_only_for_a_cli_known_to_be_new_enough() {
    // The list orders the rows it names; the added row goes before Fable 5.2.
    let list = hosted(
        serde_json::json!([{"id": "claude-fable-5-2", "order": 10}, {
            "id": "claude-fable-5-3", "label": "Fable 5.3", "order": 5, "minCli": "2.1.330",
            "efforts": ["low", "high", "max", "high"],
            "options": ["fastMode", "somethingNew"]
        }]),
    );
    for cli in [None, Some("2.1.329"), Some("not a version")] {
        assert_eq!(
            ids(&apply(catalog(), &list, cli, Source::Cli)),
            ids(&catalog()),
            "{cli:?}"
        );
    }
    for cli in ["2.1.330", "2.2.0"] {
        let models = apply(catalog(), &list, Some(cli), Source::Cli);
        let added = &models[0];
        assert_eq!(added.id, "claude-fable-5-3", "{cli}");
        assert_eq!(added.label, "Fable 5.3");
        assert!(!added.default);
        assert_eq!(
            added.reasoning_levels,
            [
                ReasoningLevel::Low,
                ReasoningLevel::High,
                ReasoningLevel::Max
            ]
        );
        // The unknown option name is skipped, the known one built.
        assert_eq!(added.options.len(), 1);
        assert_eq!(added.options[0].id, "fastMode");
    }
    // Without minCli an entry never adds a row.
    let ungated = hosted(serde_json::json!([{"id": "claude-fable-5-3", "label": "Fable 5.3"}]));
    assert_eq!(
        ids(&apply(catalog(), &ungated, Some("9.9.9"), Source::Cli)),
        ids(&catalog())
    );
}

#[test]
fn min_cli_on_an_existing_row_hides_it_from_an_older_known_cli() {
    let list = hosted(serde_json::json!([
        {"id": "claude-fable-5-2", "minCli": "2.1.290"},
        {"id": "claude-opus-5-5", "minCli": "2.1.290"}
    ]));
    let old = apply(catalog(), &list, Some("2.1.200"), Source::Cli);
    // Fable goes; Opus is preselected, so it stays.
    assert_eq!(
        ids(&old),
        ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-4-5"]
    );
    for cli in [None, Some("2.1.290")] {
        assert_eq!(
            ids(&apply(catalog(), &list, cli, Source::Cli)),
            ids(&catalog())
        );
    }
}

// ── validation ──────────────────────────────────────────────────────────

fn reject(value: serde_json::Value) -> String {
    validate(&serde_json::to_vec(&value).unwrap()).unwrap_err()
}

fn list_of(models: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"schema": 1, "harnesses": {"claude-code": {"models": models}}})
}

#[test]
fn bad_lists_are_rejected_whole() {
    for bytes in [
        &b""[..],
        b"null",
        b"[]",
        b"{",
        b"{\"harnesses\":{}}",
        b"\xff\xfe",
    ] {
        assert!(validate(bytes).is_err(), "{bytes:?}");
    }
    assert!(reject(serde_json::json!({"schema": 2, "harnesses": {}})).contains("schema"));
    for (models, why) in [
        (serde_json::json!([{"id": ""}]), "bad id"),
        (serde_json::json!([{"id": "a b"}]), "bad id"),
        (serde_json::json!([{"id": "x".repeat(129)}]), "bad id"),
        (serde_json::json!([{"id": "a"}, {"id": "a"}]), "twice"),
        (serde_json::json!([{"id": "a", "label": "  "}]), "label"),
        (
            serde_json::json!([{"id": "a", "label": "x".repeat(65)}]),
            "label",
        ),
        (
            serde_json::json!([{"id": "a", "label": "Bad\nlabel"}]),
            "label",
        ),
        (
            serde_json::json!([{"id": "a", "description": "x".repeat(201)}]),
            "description",
        ),
        (serde_json::json!([{"id": "a", "minCli": "2.1"}]), "minCli"),
        (
            serde_json::json!([{"id": "a", "aliases": ["a"]}]),
            "aliases",
        ),
        (
            serde_json::json!([{"id": "a", "efforts": ["turbo"]}]),
            "effort",
        ),
        (
            serde_json::json!([{"id": "a", "default": true, "hidden": true}]),
            "hidden",
        ),
        (
            serde_json::json!([{"id": "a", "default": true}, {"id": "b", "default": true}]),
            "default",
        ),
        (serde_json::json!([{"id": "a", "order": "first"}]), "JSON"),
        (serde_json::json!([{"id": "a", "order": 1e12}]), "JSON"),
    ] {
        let error = reject(list_of(models.clone()));
        assert!(error.contains(why), "{models}: {error}");
    }
    let many: Vec<_> = (0..101)
        .map(|i| serde_json::json!({"id": format!("m{i}")}))
        .collect();
    assert!(reject(list_of(many.into())).contains("too many"));
}

#[test]
fn oversized_lists_are_rejected_before_parsing() {
    let mut big = serde_json::to_vec(&list_of(serde_json::json!([]))).unwrap();
    big.extend(std::iter::repeat_n(b' ', MAX_BYTES));
    assert!(validate(&big).unwrap_err().contains("limit"));
}

#[test]
fn unknown_fields_and_harnesses_are_allowed_for_newer_lists() {
    let list = validate(
        br#"{"schema":1,"publishedAt":"2026-11-10","harnesses":{
            "claude-code":{"models":[{"id":"claude-opus-5-5","badge":"new"}],"note":"x"},
            "some-future-harness":{"models":[]}}}"#,
    )
    .unwrap();
    assert_eq!(list.harnesses.len(), 2);
}

/// The checked-in staging list must pass this engine's own checks.
#[test]
fn the_staging_list_on_the_edge_is_valid() {
    let list = validate(include_bytes!("../../../edge/src/models/staging-v1.json")).unwrap();
    let claude = &list.harnesses["claude-code"];
    assert!(!claude.models.is_empty());
    // And applied to the real built-in catalog it keeps every built-in row
    // for a current CLI and marks one default.
    let builtin = zeron_harness::claude::catalog::static_models();
    let models = apply(builtin.clone(), claude, Some("2.1.322"), Source::BuiltIn);
    for model in &builtin {
        assert!(models.iter().any(|m| m.id == model.id), "{}", model.id);
    }
    assert_eq!(models.iter().filter(|m| m.default).count(), 1);
}

#[test]
fn only_https_on_zeron_sh_may_serve_the_list() {
    for (edge, url) in [
        (
            "https://edge.zeron.sh",
            Some("https://edge.zeron.sh/models/v1.json"),
        ),
        (
            "https://edge.zeron.sh/",
            Some("https://edge.zeron.sh/models/v1.json"),
        ),
        (
            "https://staging.zeron.sh",
            Some("https://staging.zeron.sh/models/v1.json"),
        ),
        ("https://zeron.sh", Some("https://zeron.sh/models/v1.json")),
        ("http://edge.zeron.sh", None),
        ("https://edge.zeron.sh:8443", None),
        ("https://zeron.sh.evil.example", None),
        ("https://evilzeron.sh", None),
        ("https://user@edge.zeron.sh", None),
        (
            "https://zeron-edge-staging.sylasincorporated.workers.dev",
            None,
        ),
        ("http://127.0.0.1:27640", None),
        ("not a url", None),
    ] {
        assert_eq!(list_url(edge).as_deref(), url, "{edge}");
    }
}

// ── cache, offline, fetch ───────────────────────────────────────────────

/// A one-route HTTP server on loopback: each request gets the next reply
/// (`status`, `etag`, `body`); the request heads are recorded.
async fn server(
    replies: Vec<(u16, Option<&'static str>, Vec<u8>)>,
) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}{PATH}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        for (status, etag, body) in replies {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut head = vec![0; 4096];
            let n = socket.read(&mut head).await.unwrap();
            log.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&head[..n]).into_owned());
            let etag = etag.map(|e| format!("etag: {e}\r\n")).unwrap_or_default();
            let reply = format!(
                "HTTP/1.1 {status} X\r\ncontent-length: {}\r\n{etag}connection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
        }
    });
    (url, seen)
}

fn good() -> Vec<u8> {
    serde_json::to_vec(&list_of(serde_json::json!([
        {"id": "claude-haiku-4-5", "label": "Haiku 4.5 (hosted)"}
    ])))
    .unwrap()
}

fn label_of(hosted: &Arc<HostedModels>) -> String {
    let models = hosted.apply(HarnessId::ClaudeCode, catalog(), None, Source::Cli);
    models
        .iter()
        .find(|m| m.id == "claude-haiku-4-5")
        .unwrap()
        .label
        .clone()
}

#[tokio::test]
async fn a_fetched_list_is_cached_on_disk_and_used_offline() {
    let dir = tempfile::tempdir().unwrap();
    let (url, seen) = server(vec![(200, Some("\"v1\""), good())]).await;
    let hosted = HostedModels::new(dir.path(), Some(url.clone()), true);
    assert_eq!(label_of(&hosted), "Haiku 4.5");
    hosted.fetch().await.unwrap();
    assert_eq!(label_of(&hosted), "Haiku 4.5 (hosted)");
    assert_eq!(seen.lock().unwrap().len(), 1);
    // A new engine with the server gone: the disk copy, no network needed.
    let offline = HostedModels::new(dir.path(), Some(url.clone()), true);
    assert_eq!(label_of(&offline), "Haiku 4.5 (hosted)");
    assert!(offline.fetch().await.is_err());
    assert_eq!(label_of(&offline), "Haiku 4.5 (hosted)");
    // A copy saved for another URL (another edge) is not used.
    let other = HostedModels::new(dir.path(), Some(format!("{url}?other")), true);
    assert_eq!(label_of(&other), "Haiku 4.5");
}

#[tokio::test]
async fn the_etag_is_sent_and_a_304_keeps_the_list() {
    let dir = tempfile::tempdir().unwrap();
    let (url, seen) = server(vec![(200, Some("\"v1\""), good()), (304, None, vec![])]).await;
    let hosted = HostedModels::new(dir.path(), Some(url), true);
    hosted.fetch().await.unwrap();
    hosted.fetch().await.unwrap();
    let seen = seen.lock().unwrap();
    assert!(!seen[0].to_ascii_lowercase().contains("if-none-match"));
    assert!(
        seen[1]
            .to_ascii_lowercase()
            .contains("if-none-match: \"v1\"")
    );
    drop(seen);
    assert_eq!(label_of(&hosted), "Haiku 4.5 (hosted)");
}

#[tokio::test]
async fn bad_oversized_or_failed_replies_keep_the_last_good_list() {
    let dir = tempfile::tempdir().unwrap();
    let huge = vec![b' '; MAX_BYTES + 1];
    let (url, _) = server(vec![
        (200, Some("\"v1\""), good()),
        (
            200,
            Some("\"v2\""),
            b"{\"schema\":1,\"harnesses\":{\"claude-code\":{\"models\":[{\"id\":\"\"}]}}}".to_vec(),
        ),
        (200, None, huge),
        (200, None, b"<html>captive portal</html>".to_vec()),
        (500, None, b"oops".to_vec()),
        (302, None, vec![]),
    ])
    .await;
    let hosted = HostedModels::new(dir.path(), Some(url.clone()), true);
    hosted.fetch().await.unwrap();
    for _ in 0..5 {
        assert!(hosted.fetch().await.is_err());
        assert_eq!(label_of(&hosted), "Haiku 4.5 (hosted)");
    }
    // Nothing bad reached the disk either.
    let reloaded = HostedModels::new(dir.path(), Some(url), true);
    assert_eq!(label_of(&reloaded), "Haiku 4.5 (hosted)");
}

#[tokio::test]
async fn a_404_retires_the_list() {
    let dir = tempfile::tempdir().unwrap();
    let (url, _) = server(vec![(200, Some("\"v1\""), good()), (404, None, vec![])]).await;
    let hosted = HostedModels::new(dir.path(), Some(url.clone()), true);
    hosted.fetch().await.unwrap();
    hosted.fetch().await.unwrap();
    assert_eq!(label_of(&hosted), "Haiku 4.5");
    assert_eq!(
        label_of(&HostedModels::new(dir.path(), Some(url), true)),
        "Haiku 4.5"
    );
}

#[test]
fn a_corrupt_disk_copy_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let url = "https://edge.zeron.sh/models/v1.json".to_owned();
    let path = dir.path().join("model-catalogs").join("hosted-v1.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    for body in [
        "not json".to_owned(),
        serde_json::json!({"schema_version":1,"fetched_at":0,"url":url,"etag":null,
            "body":"{\"schema\":1,\"harnesses\":{\"claude-code\":{\"models\":[{\"id\":\"a b\"}]}}}"})
        .to_string(),
    ] {
        std::fs::write(&path, body).unwrap();
        let hosted = HostedModels::new(dir.path(), Some(url.clone()), false);
        assert!(hosted.for_harness(HarnessId::ClaudeCode).is_none());
    }
}

#[tokio::test]
async fn a_stale_list_refreshes_in_the_background_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let (url, seen) = server(vec![(200, Some("\"v1\""), good())]).await;
    let hosted = HostedModels::new(dir.path(), Some(url), true);
    // The first listing answers from what it has and starts the fetch…
    assert_eq!(label_of(&hosted), "Haiku 4.5");
    for _ in 0..200 {
        if label_of(&hosted) == "Haiku 4.5 (hosted)" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // …which the next listings use; a fresh list is not fetched again.
    assert_eq!(label_of(&hosted), "Haiku 4.5 (hosted)");
    assert_eq!(seen.lock().unwrap().len(), 1);
}
