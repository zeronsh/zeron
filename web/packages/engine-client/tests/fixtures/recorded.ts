/**
 * Frames recorded from a real engine session (crates/engine/examples/
 * web_conformance, mock profile): the client frames are the exact strings
 * the capture sent — hand-written to the Rust serde shape — and the server
 * messages are verbatim engine replies. Values are instance-specific (the
 * credential and device id change per run); the tests assert structure,
 * never the values. The WatchQueue `{items}` wrapper shape lives in
 * `@zeron/proto`'s hand shims; a fresh mock profile has no chats to
 * record, so only the invoke-side special case is exercised elsewhere.
 */
export const recorded = {
  authCredential: "c4G9pwQ_FFiTgzOcnMRiHVylB-wlzGlHpyagG7TgDuU",
  deviceId: "66d356a1-499e-40a2-97e4-89bd1729487a",
  clientFrames: {
    auth: '{"auth":"c4G9pwQ_FFiTgzOcnMRiHVylB-wlzGlHpyagG7TgDuU"}',
    engineInfoInvoke: '{"id":1,"method":"EngineInfo","params":{}}',
    watchChatsInvoke: '{"id":2,"method":"WatchChats","params":{}}',
    watchChatsCancel: '{"id":2,"cancel":true}',
  },
  serverMessages: {
    engineInfo:
      '{"id":1,"ok":{"capabilities":["message-queue-v1","message-queue-actions-v1","message-queue-attachments-v1","message-queue-clean-attachment-text-v1","message-queue-edit-lease-v1","web-client"],"deviceId":"66d356a1-499e-40a2-97e4-89bd1729487a","workspaceScope":"local"}}',
    watchChatsItem: '{"id":2,"item":[]}',
  },
};
