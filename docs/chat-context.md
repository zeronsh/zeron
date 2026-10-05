# Add another chat as context

On desktop, drag a chat from the left sidebar onto the conversation or composer of the chat you want to use it in. The pane shows **Drop to add chat context**; releasing adds a `#title` chip to that pane's draft. This also works for saved and unsent side chats in the right pane, regardless of keyboard focus.

The drop does not send a message. Add your question and send when ready. A repeated drop reuses the existing reference. Dropping a chat onto itself does nothing. Chips participate in normal selection, deletion, undo, draft switching, and queue editing. Sidebar pinning and section moves still work inside the sidebar.

A reference stores the chat ID and a short title snapshot, not a transcript copy. On send, every provider gets the identity and instructions to read that chat through Zeron MCP `read_chat`, paging older messages with `offset` and `limit`. History is retrieved when the agent reads it, so later messages remain available. The agent is told to treat the source as reference material and to leave it alone unless asked. Access follows the existing MCP workspace scope and source-device availability; unavailable or deleted sources cannot be read.

The device hosting the destination chat must advertise `chat-context-v1`. If it runs an older Zeron, sending is blocked with an update notice and the draft is retained.

This follows T3 Code's sidebar-to-composer interaction and identity-only thread context design. Zeron uses its native GPUI drop targets, durable Markdown chip transport, and existing `read_chat` tool.
