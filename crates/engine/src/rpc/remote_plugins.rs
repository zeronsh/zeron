//! Settings → Plugins on another of your devices: what that device's
//! engine says when it's from before it took approvals and files from your
//! other devices, said as what to do about it.

use zeron_rpc::{RpcError, methods};

/// The first Zeron whose engine takes reviews, allows and plugin files from
/// your other devices: the 0.3.0 stable, as the 0.2.139 betas before it.
/// An older engine is known by its refusal's words, not by a version
/// compared, so no beta suffix can read as new enough; this only names the
/// release to update to.
pub const REMOTE_APPROVALS_SINCE: &str = "0.3.0";

/// The words for a device whose Zeron is too old for this.
pub fn too_old() -> String {
    format!(
        "That device's Zeron is too old to approve or install plugins from your other devices. \
         Update Zeron there to {REMOTE_APPROVALS_SINCE} or later, or allow it on that device."
    )
}

/// `error` from `target` for `method`, as the person can act on it: an
/// older engine's refusal (it kept allowing to its own window and had no
/// way to take a file) said as which version it needs; anything else as it
/// was.
pub fn older_engine(target: &str, method: &str, error: RpcError) -> RpcError {
    let said = error.to_string();
    let older = match method {
        // Older engines answered reviews over their own IPC only, and
        // refused an allow that came over the relay.
        methods::REVIEW_PLUGIN => said.contains("only answered over this device's local IPC"),
        methods::DECIDE_PLUGIN => said.contains("only this device's own window can do that"),
        // Older engines didn't know the method, or the install's `upload`.
        methods::STAGE_PLUGIN_PACKAGE => {
            matches!(error, RpcError::UnknownMethod(_)) || said.to_ascii_lowercase().contains("unknown method")
        }
        methods::INSTALL_PLUGIN => said.contains("Give a package file (`path`) or a link (`url`)"),
        _ => false,
    };
    if older {
        tracing::info!(device = target, method, "plugins: device's engine predates remote approvals: {said}");
        RpcError::Failed(too_old())
    } else {
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a device on an older Zeron said to each, word for word.
    #[test]
    fn an_older_device_s_refusal_says_which_version_it_needs() {
        for (method, error) in [
            (methods::REVIEW_PLUGIN, RpcError::Failed("ReviewPlugin is only answered over this device's local IPC".into())),
            (methods::DECIDE_PLUGIN, RpcError::Failed("DecidePlugin: only this device's own window can do that".into())),
            (methods::STAGE_PLUGIN_PACKAGE, RpcError::UnknownMethod("StagePluginPackage".into())),
            (methods::STAGE_PLUGIN_PACKAGE, RpcError::Failed("unknown method: StagePluginPackage".into())),
            (methods::INSTALL_PLUGIN, RpcError::Failed("Give a package file (`path`) or a link (`url`)".into())),
        ] {
            let said = older_engine("laptop", method, error).to_string();
            assert!(said.contains(REMOTE_APPROVALS_SINCE) && said.contains("Update Zeron there"), "{method}: {said}");
        }
        // Anything else stays as it was: the device away, a stale review.
        let away = older_engine("laptop", methods::DECIDE_PLUGIN, RpcError::Failed("transport: peer laptop: unreachable".into()));
        assert_eq!(away.to_string(), "transport: peer laptop: unreachable");
        let stale = older_engine("laptop", methods::DECIDE_PLUGIN, crate::plugins::reviews::needed("Allowing a plugin"));
        assert!(stale.to_string().starts_with("review:"), "{stale}");
    }
}
