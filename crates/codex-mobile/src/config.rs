use std::{path::PathBuf, sync::Arc};

use codex_app_server_client::{EnvironmentManager, InProcessClientStartArgs};
use codex_config::{CloudConfigBundleLoader, LoaderOverrides};
use codex_core::config::{ConfigBuilder, ConfigOverrides};
use codex_feedback::CodexFeedback;
use codex_protocol::protocol::SessionSource;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    pub home: PathBuf,
    pub fixture_base_url: Option<String>,
}

pub async fn start_args(options: &Options) -> std::io::Result<InProcessClientStartArgs> {
    if !options.home.is_absolute() {
        return Err(std::io::Error::other("Codex home must be absolute"));
    }
    std::fs::create_dir_all(options.home.join("context"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&options.home, std::fs::Permissions::from_mode(0o700))?;
    }
    // No executor is registered. All workspace access is through the three
    // dynamic mobile tools; Codex's metadata lives separately in its app home.
    let mut overrides: Vec<(String, toml::Value)> = vec![
        ("cli_auth_credentials_store".into(), "file".into()),
        ("approval_policy".into(), "never".into()),
        // Codex must describe the actual mobile tool contract, not its default read-only policy.
        ("sandbox_mode".into(), "workspace-write".into()),
        ("sandbox_workspace_write.writable_roots".into(), toml::Value::Array(vec!["/workspace".into()])),
        ("sandbox_workspace_write.exclude_tmpdir_env_var".into(), true.into()),
        ("sandbox_workspace_write.exclude_slash_tmp".into(), true.into()),
        ("web_search".into(), "disabled".into()),
        ("analytics.enabled".into(), false.into()),
        ("feedback.enabled".into(), false.into()),
        ("features.skip_host_skill_discovery".into(), true.into()),
    ];
    for feature in [
        "shell_tool",
        "apply_patch_freeform",
        "code_mode",
        "code_mode_host",
        "code_mode_prewarm",
        "code_mode_only",
        "multi_agent",
        "multi_agent_v2",
        "hooks",
        "plugin_hooks",
        "plugins",
        "remote_plugin",
        "recommended_plugins",
        "skill_search",
        "skill_mcp_dependency_install",
        "memories",
    ] {
        overrides.push((format!("features.{feature}"), false.into()));
    }
    if let Some(base_url) = &options.fixture_base_url {
        // The fixture endpoint is only for deterministic integration tests.
        if !base_url.starts_with("http://127.0.0.1:") {
            return Err(std::io::Error::other(
                "Fixture endpoint must use IPv4 loopback",
            ));
        }
        // Exercise model-driven code-mode-only routing, not just the feature defaults.
        let mut catalog: serde_json::Value = serde_json::from_str(include_str!(
            "../../../target/native-agent-spike/codex/codex-rs/models-manager/models.json"
        )).map_err(std::io::Error::other)?;
        let models = catalog["models"].as_array_mut().ok_or_else(|| std::io::Error::other("Invalid fixture catalog"))?;
        let mut model = models.first().cloned().ok_or_else(|| std::io::Error::other("Empty fixture catalog"))?;
        model["slug"] = "gpt-5.1-codex".into();
        model["tool_mode"] = "code_mode_only".into();
        model["supported_in_api"] = true.into();
        *models = vec![model];
        let catalog_path = options.home.join("fixture-models.json");
        std::fs::write(&catalog_path, serde_json::to_vec(&catalog).map_err(std::io::Error::other)?)?;
        overrides.push(("model_catalog_json".into(), catalog_path.to_string_lossy().into_owned().into()));
        overrides.extend([
            ("model_provider".into(), "mobile_fixture".into()),
            ("model".into(), "gpt-5.1-codex".into()),
            (
                "model_providers.mobile_fixture.name".into(),
                "Mobile test fixture".into(),
            ),
            (
                "model_providers.mobile_fixture.base_url".into(),
                base_url.clone().into(),
            ),
            (
                "model_providers.mobile_fixture.wire_api".into(),
                "responses".into(),
            ),
            (
                "model_providers.mobile_fixture.requires_openai_auth".into(),
                false.into(),
            ),
        ]);
    }
    let loader = LoaderOverrides {
        ignore_user_config: true,
        ignore_project_config: true,
        ignore_user_and_project_exec_policy_rules: true,
        ..Default::default()
    };
    let config = ConfigBuilder::default()
        .codex_home(options.home.clone())
        .cli_overrides(overrides.clone())
        .loader_overrides(loader.clone())
        .harness_overrides(ConfigOverrides {
            cwd: Some(options.home.join("context")),
            ..Default::default()
        })
        .build()
        .await?;
    let environments = EnvironmentManager::without_environments(config.http_client_factory());
    let state_db = codex_core::init_state_db(&config).await;
    Ok(InProcessClientStartArgs {
        arg0_paths: Default::default(),
        config: Arc::new(config),
        cli_overrides: overrides,
        loader_overrides: loader,
        strict_config: false,
        cloud_config_bundle: CloudConfigBundleLoader::default(),
        embedded_network_policy: Default::default(),
        feedback: CodexFeedback::new(),
        log_db: None,
        state_db,
        environment_manager: Arc::new(environments),
        config_warnings: vec![],
        session_source: SessionSource::Exec,
        enable_codex_api_key_env: false,
        client_name: "zeron_ios".into(),
        client_version: env!("CARGO_PKG_VERSION").into(),
        experimental_api: true,
        mcp_server_openai_form_elicitation: false,
        opt_out_notification_methods: vec![],
        channel_capacity: 128,
    })
}

pub const INSTRUCTIONS: &str = "You are running natively inside Zeron on iOS. Your project is in a writable, persistent /workspace filesystem, accessed ONLY through mobile_shell, mobile_read_file, and mobile_write_file. All three mobile tools are available independently of desktop execution environments and share the same files. Desktop shell or filesystem capability restrictions do not disable these mobile tools. Use mobile_write_file to create files and mobile_shell to list directories or edit files. Writes within /workspace are authorized; there is no additional enable-write-access step. Report a tool failure only after actually calling the tool. Completed writes persist even if a later command fails or is interrupted. Use these tools to inspect and edit the user's project. mobile_shell is a limited Bash interpreter, not an OS shell: it has no Node, npm, Cargo, Python, package installation, general networking, or native executable support. Each shell call begins in /workspace; use explicit paths. Shell work has a 7 second watchdog; native Git may take up to 45 seconds. Workspace limits: 128 MB, 20,000 entries, 16 MB per file. Native commands in mobile_shell: git help lists the supported embedded Git subset (init, status, diff [--cached], add, commit -m, log, branch, config user.name/user.email, public HTTPS clone URL DEST; git -C /workspace/project COMMAND works). Git has no private authentication, fetch/push, hooks, submodules or checkout/switch. Configure an identity supplied by the user before committing. pdf html /workspace/input.html /workspace/output.pdf [a4|letter] [margin_points] creates up to 100 pages with CSS page breaks; self-contained HTML only. pdf images /workspace/output.pdf /workspace/image.png [...images] creates one A4 page per image (up to 24 images, 16 million pixels total). serve [/workspace/index.html] starts a native static localhost server and returns its URL. Relative CSS/JS/images and local JSON fetch work. Tell the user to open Preview website in the chat menu; HTML files also have Open website. Serve snapshots stay consistent; run serve again or tap Refresh to publish edits. This is foreground-only static hosting, not a Node/Python backend. serve stop shuts it down. Website code has no native filesystem bridge or external network. Ordinary file tools cannot access .git; use git commands for metadata. Do not claim to run unavailable builds or tests. For computed charts, fractals, PNGs and PDFs: write a self-contained HTML file with inline JavaScript/canvas/SVG, then call mobile_shell with: render /workspace/input.html /workspace/output.png 800 600 (or output.pdf). Width and height are 1-2048, at most 4 million pixels. The renderer has no network, filesystem bridge, external assets, or Node. Inline scripts may assign window.zeronReady to a Promise for asynchronous drawing; rendering waits for it, fonts, and embedded images, within a 4 second deadline. Avoid using limited awk to construct binary formats. Use computation, not imagegen, for mathematically accurate graphs. imagegen results are NOT displayed inline in this UI. To ensure an imagegen output is saved, use mobile_shell: import_image GENERATED_FILENAME.png /workspace/output.png; use the filename from the imagegen result, even if its absolute container path is stale. This imports only images from this conversation. Do not use cp on private native paths. After a successful render/import, give the workspace file path and tell the user to open Workspace files to preview or Save to Files. Never claim an artifact is displayed above. If tools fail, report the actual failure without claiming a file exists. The host cwd is private session metadata, not the user's project.";

pub fn tools() -> serde_json::Value {
    use serde_json::json;
    let function = |name: &str, description: &str, properties, required| {
        json!({
            "type": "function", "name": name, "description": description,
            "inputSchema": {"type":"object", "properties":properties, "required":required, "additionalProperties":false}
        })
    };
    json!([
        function(
            "mobile_shell",
            "Run a limited Bash command in /workspace. Supports pipes, grep, rg, sed, limited awk, jq and file operations. Also: render INPUT.html OUTPUT.pdf|png WIDTH HEIGHT; import_image GENERATED_FILENAME.png [/workspace/OUTPUT.png]. No native executables or networking.",
            json!({"command":{"type":"string"}}),
            json!(["command"])
        ),
        function(
            "mobile_read_file",
            "Read a UTF-8 file from the shared virtual workspace. Use an absolute /workspace/ path.",
            json!({"path":{"type":"string"}}),
            json!(["path"])
        ),
        function(
            "mobile_write_file",
            "Write a UTF-8 file in the shared virtual workspace. Use an absolute /workspace/ path.",
            json!({"path":{"type":"string"},"content":{"type":"string"}}),
            json!(["path", "content"])
        )
    ])
}
