use clap::{Subcommand, ValueEnum};
use serde_json::json;
use zeron_engine::{Engine, EngineConfig, InstanceLock, WorkspaceScope};
use zeron_rpc::{RpcReply, methods};

#[derive(Clone, Copy, ValueEnum)]
pub enum Role {
    Server,
    Client,
}
impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Client => "client",
        }
    }
}

#[derive(Subcommand)]
pub enum Command {
    /// Create a sync hub and expose it privately with Tailscale Serve.
    Create {
        name: String,
        #[arg(long, value_enum, default_value = "server")]
        role: Role,
        #[arg(long, default_value_t = 27655)]
        listen_port: u16,
        #[arg(long, default_value_t = 8443)]
        serve_port: u16,
    },
    /// Pair this device with a private workspace.
    Join {
        hub_url: String,
        #[arg(long)]
        code: String,
        #[arg(long)]
        name: String,
        #[arg(long, value_enum, default_value = "server")]
        role: Role,
    },
    /// Show private workspace state and paired nodes without credentials.
    Status,
    /// Issue a single-use invitation on the hub (valid for five minutes).
    Pair {
        #[arg(long, value_enum, default_value = "client")]
        role: Role,
    },
    /// Revoke a paired node and disconnect it immediately.
    Revoke { device_id: String },
    /// Enable this device's private connection (or the hub's remote access).
    Enable,
    /// Disable private access while preserving local work.
    Disable,
    /// Leave the private workspace and select Local on next startup.
    Leave,
}

pub async fn run(config: EngineConfig, command: Command) -> anyhow::Result<()> {
    let (method, params) = match command {
        Command::Create {
            name,
            role,
            listen_port,
            serve_port,
        } => (
            methods::CREATE_PRIVATE_WORKSPACE,
            json!({"name":name,"role":role.as_str(),"listenPort":listen_port,"servePort":serve_port}),
        ),
        Command::Join {
            hub_url,
            code,
            name,
            role,
        } => (
            methods::JOIN_PRIVATE_WORKSPACE,
            json!({"hubUrl":hub_url,"code":code,"name":name,"role":role.as_str()}),
        ),
        Command::Status => (methods::PRIVATE_STATUS, json!({})),
        Command::Pair { role } => (
            methods::CREATE_PRIVATE_INVITATION,
            json!({"role":role.as_str()}),
        ),
        Command::Revoke { device_id } => {
            (methods::REVOKE_PRIVATE_NODE, json!({"deviceId":device_id}))
        }
        Command::Enable => (methods::SET_PRIVATE_ACCESS_ENABLED, json!({"enabled":true})),
        Command::Disable => (
            methods::SET_PRIVATE_ACCESS_ENABLED,
            json!({"enabled":false}),
        ),
        Command::Leave => (methods::LEAVE_PRIVATE_WORKSPACE, json!({})),
    };
    let value = if InstanceLock::holder(&config.data_dir).is_some() {
        let client = zeron_rpc::connect_ws(&format!("ws://127.0.0.1:{}", config.ipc_port)).await?;
        let expected = Engine::engine_info(&config, WorkspaceScope::Local)?.device_id;
        let actual: zeron_engine::EngineInfo =
            client.call_as(methods::ENGINE_INFO, json!({})).await?;
        anyhow::ensure!(
            expected == actual.device_id,
            "IPC port belongs to another Zeron data directory"
        );
        client.call(method, params).await?
    } else {
        std::fs::create_dir_all(&config.data_dir)?;
        let _lock = InstanceLock::acquire(&config.data_dir)?;
        let device = Engine::engine_info(&config, WorkspaceScope::Local)?.device_id;
        let access = zeron_engine::private_access::PrivateAccess::open(&config.data_dir)?;
        let RpcReply::Value(value) = access.handle(method, params, &device).await? else {
            anyhow::bail!("Expected a private workspace reply")
        };
        if zeron_engine::private_access::PrivateAccess::changes_workspace(method) {
            Engine::build_auth(&config).await.sign_out();
        }
        value
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    if value.get("restartRequired").and_then(|v| v.as_bool()) == Some(true) {
        println!(
            "Restart Zeron to apply the workspace change. Use `zeron daemon install` to run in the background."
        );
    }
    Ok(())
}
