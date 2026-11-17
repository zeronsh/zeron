//! Option B of the usage-limit plan: a thread whose account hit its usage
//! limit continues meanwhile on another connected account slot of the same
//! harness that has quota, when its orchestrator allows it
//! (`CrewPolicy::on_limit`, off by default), and switches back after the
//! original account's reset, at a turn boundary.
//!
//! (Interface only so far; filled in by the account-switch work.)

use zeron_proto::usage_limit::{SessionLimit, SwitchedAccount};

use crate::rpc::EngineRpc;

impl EngineRpc {
    /// Continue `chat_id`, stopped by `limit`, on another account when its
    /// orchestrator's policy allows it and one has quota: the account it now
    /// runs on (the caller records it on the limit and sends the switch
    /// message). `None`: wait for the reset.
    pub(crate) async fn switch_account_for_limit(&self, chat_id: &str, limit: &SessionLimit) -> Option<SwitchedAccount> {
        let _ = (chat_id, limit);
        None
    }

    /// At a turn boundary of `chat_id` running on another account after its
    /// own account's limit reset: back to its own account. `true` when it
    /// switched back.
    pub(crate) async fn switch_back_after_reset(&self, chat_id: &str, limit: &SessionLimit) -> bool {
        let _ = (chat_id, limit);
        false
    }
}
