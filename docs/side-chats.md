# Side chats

Hover a completed Codex, Claude Code, OpenCode, or Pi reply and click the fork icon immediately to the left of Copy. Its menu offers **Fork in side chat** and **Fork as main conversation**. A main conversation appears in the left sidebar and opens in the main chat area. The same controls are reachable by keyboard. Zeron opens a saved conversation with an empty composer and the original conversation through that reply. Creating it does not send a prompt or run the model.

The new conversation resumes an independent native provider session. Later turns in the original conversation are excluded. The original chat keeps running. Both chats use the same execution device, provider, and current checkout; this action does not restore files or create a worktree. The provider picker stays locked in either saved destination.

Choosing **Fork in side chat** inside a side chat creates a sibling under that chat's existing root. Choosing **Fork as main conversation** creates a root conversation regardless of where it started. Replies inherited from an earlier fork retain their original native provenance. Forking one of those replies uses the native session that actually contains its boundary; new replies belong to the child's session.

The fork icon is hidden when a reply has no recorded native boundary or the execution host does not support native message forks. This includes older replies without a mapping and hosts that need updating. The icon appears automatically when both become available. Hosts with native side-chat forks but no main-conversation support show the main-conversation menu option disabled until the host is updated. Other availability failures, such as an unverified provider contract, keep the icon disabled with an explanation. Codex boundaries correspond to completed provider turns, so a segment closed during steering may have no native boundary. Zeron never substitutes a textual context copy for this action.

If the child's native session has disappeared or cannot be resumed, sending fails explicitly. If a fork request loses its provider response, Zeron reports an indeterminate outcome and does not repeat creation automatically. A confirmed result can be retried safely using the same operation identity. Closing or changing panels does not cancel persistence or redirect the result to a different conversation.

Existing empty-side-chat and context-copy actions keep their existing behavior. See [native fork regression evidence](regressions/native-message-forks.md) for supported contracts, tests, and limitations.
