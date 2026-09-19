//! Tailscale Serve management scoped to one HTTPS listener. Never reset the
//! node's Serve configuration: other applications may own its other ports.

use crate::PrivateConfig;
use anyhow::{Context, Result, ensure};
use serde_json::Value;

async fn command(args: &[&str]) -> Result<Vec<u8>> {
    let mut command = tokio::process::Command::new("tailscale");
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .context(
            "Tailscale did not respond; check that the service is running and HTTPS is enabled",
        )?
        .context("Cannot run tailscale; install it and connect this machine to your tailnet")?;
    ensure!(
        output.status.success(),
        "Tailscale command failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(output.stdout)
}

pub async fn discover_url(serve_port: u16) -> Result<String> {
    ensure!(serve_port > 0, "The HTTPS port must be nonzero");
    let status: Value = serde_json::from_slice(&command(&["status", "--json"]).await?)?;
    ensure!(
        status.get("BackendState").and_then(Value::as_str) == Some("Running"),
        "Tailscale is not connected; run tailscale up first"
    );
    let dns = status
        .pointer("/Self/DNSName")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim_end_matches('.');
    ensure!(
        !dns.is_empty()
            && dns
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'.'),
        "Tailscale has no valid MagicDNS hostname; enable MagicDNS and HTTPS in your tailnet"
    );
    Ok(if serve_port == 443 {
        format!("https://{dns}")
    } else {
        format!("https://{dns}:{serve_port}")
    })
}

async fn status() -> Result<Value> {
    Ok(serde_json::from_slice(
        &command(&["serve", "status", "--json"]).await?,
    )?)
}

pub async fn setup(config: &PrivateConfig) -> Result<String> {
    ensure!(
        config.host_hub && config.listen_port > 0,
        "Serve requires a hub with a fixed local port"
    );
    let hub_url = discover_url(config.serve_port).await?;
    ensure!(
        hub_url == config.hub_url,
        "Hub URL does not match this machine's Tailscale HTTPS address"
    );
    let before = status().await?;
    if inspect(&before, config)? {
        return Ok(hub_url);
    }
    let https = format!("--https={}", config.serve_port);
    let target = format!("http://127.0.0.1:{}", config.listen_port);
    command(&["serve", "--bg", &https, &target]).await?;
    ensure!(
        inspect(&status().await?, config)?,
        "Tailscale did not retain the private hub route"
    );
    Ok(hub_url)
}

pub async fn disable(config: &PrivateConfig) -> Result<()> {
    if !inspect(&status().await?, config)? {
        return Ok(());
    }
    let https = format!("--https={}", config.serve_port);
    command(&["serve", &https, "off"]).await?;
    ensure!(
        !inspect(&status().await?, config)?,
        "Tailscale still serves the private hub route"
    );
    Ok(())
}

pub async fn verify(config: &PrivateConfig) -> Result<()> {
    ensure!(
        inspect(&status().await?, config)?,
        "The private hub Tailscale Serve route is missing"
    );
    Ok(())
}

fn inspect(value: &Value, config: &PrivateConfig) -> Result<bool> {
    let origin = url::Url::parse(&config.hub_url)?;
    let host_port = format!(
        "{}:{}",
        origin.host_str().context("Hub hostname is missing")?,
        config.serve_port
    );
    let target = format!("http://127.0.0.1:{}", config.listen_port);
    let mut owned = false;
    inspect_scope(value, &host_port, &target, config.serve_port, &mut owned)?;
    Ok(owned)
}

fn inspect_scope(
    value: &Value,
    host_port: &str,
    target: &str,
    port: u16,
    owned: &mut bool,
) -> Result<()> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    if let Some(funnel) = object.get("AllowFunnel").and_then(Value::as_object) {
        ensure!(
            !funnel
                .iter()
                .any(|(host, enabled)| enabled == true && host.ends_with(&format!(":{port}"))),
            "The requested HTTPS port is exposed through Funnel; disable public exposure before enabling private access"
        );
    }
    let tcp = object
        .get("TCP")
        .and_then(Value::as_object)
        .and_then(|tcp| tcp.get(&port.to_string()));
    let web = object.get("Web").and_then(Value::as_object);
    if let Some(tcp) = tcp {
        ensure!(
            tcp.get("HTTPS") == Some(&Value::Bool(true))
                && tcp.get("TCPForward").is_none()
                && tcp.get("TerminateTLS").is_none(),
            "The requested HTTPS port belongs to another Tailscale service"
        );
        let routes = web
            .and_then(|web| web.get(host_port))
            .and_then(|route| route.get("Handlers"))
            .and_then(Value::as_object)
            .context("The requested port has an unrelated Serve configuration")?;
        ensure!(
            routes.len() == 1
                && routes
                    .get("/")
                    .and_then(|route| route.get("Proxy"))
                    .and_then(Value::as_str)
                    == Some(target),
            "The requested HTTPS port belongs to another application; choose a different port"
        );
        ensure!(
            !*owned,
            "Multiple Tailscale scopes claim the private hub port"
        );
        *owned = true;
    }
    if let Some(web) = web {
        for (host, routes) in web {
            if host.ends_with(&format!(":{port}")) {
                ensure!(
                    host == host_port && tcp.is_some(),
                    "Conflicting Serve hostname on the requested port"
                );
            }
            if let Some(handlers) = routes.get("Handlers").and_then(Value::as_object) {
                for handler in handlers.values() {
                    if handler.get("Proxy").and_then(Value::as_str) == Some(target) {
                        ensure!(
                            host == host_port && tcp.is_some(),
                            "The local hub port is already exposed on a different Serve listener"
                        );
                    }
                }
            }
        }
    }
    for scope in ["Foreground", "Services"] {
        if let Some(scopes) = object.get(scope).and_then(Value::as_object) {
            for nested in scopes.values() {
                inspect_scope(nested, host_port, target, port, owned)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn config() -> PrivateConfig {
        PrivateConfig {
            workspace_id: "workspace".into(),
            user_id: "user".into(),
            device_id: "server".into(),
            name: "Test".into(),
            hub_url: "https://hub.example.ts.net:8443".into(),
            role: crate::NodeRole::Server,
            host_hub: true,
            enabled: true,
            token: "x".repeat(64),
            listen_port: 27655,
            serve_port: 8443,
        }
    }
    fn owned() -> Value {
        json!({"TCP":{"8443":{"HTTPS":true}},"Web":{"hub.example.ts.net:8443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:27655"}}}}})
    }
    #[test]
    fn accepts_empty_or_owned_and_preserves_other_ports() {
        assert!(!inspect(&json!({}), &config()).unwrap());
        let mut value = owned();
        value["TCP"]["443"] = json!({"HTTPS":true});
        value["Web"]["hub.example.ts.net:443"] =
            json!({"Handlers":{"/":{"Proxy":"http://127.0.0.1:3000"}}});
        assert!(inspect(&value, &config()).unwrap());
    }
    #[test]
    fn rejects_funnel_conflicting_paths_and_foreground_claims() {
        let mut value = owned();
        value["AllowFunnel"] = json!({"hub.example.ts.net:8443":true});
        assert!(inspect(&value, &config()).is_err());
        let mut value = owned();
        value["Web"]["hub.example.ts.net:8443"]["Handlers"]["/other"] =
            json!({"Proxy":"http://127.0.0.1:3000"});
        assert!(inspect(&value, &config()).is_err());
        let mut value = owned();
        value["Foreground"] = json!({"session":owned()});
        assert!(inspect(&value, &config()).is_err());
        let value = json!({"TCP":{"8443":{"TCPForward":"localhost:3000"}}});
        assert!(inspect(&value, &config()).is_err());
    }
}
