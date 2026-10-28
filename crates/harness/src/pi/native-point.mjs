// Pi 0.85.1 persists message_end before dispatching turn_end. Object identity
// binds the official entry ID to that exact response, even with repeated text.
// No transcript contents or paths travel through this internal notification.
export default function (pi) {
  pi.on('turn_end', (event, ctx) => {
    const entry = ctx.sessionManager.getLeafEntry();
    if (event.message.role !== 'assistant' || event.message.stopReason !== 'stop'
        || entry?.type !== 'message' || entry.message !== event.message) return;
    ctx.ui.notify('zeron-native-fork-v1:' + JSON.stringify({
      sessionId: ctx.sessionManager.getSessionId(), entryId: entry.id,
    }), 'info');
  });
}
