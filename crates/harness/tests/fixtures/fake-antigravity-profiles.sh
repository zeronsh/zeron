#!/bin/sh
# Only profile-local state is allowed, including during sign-in.
[ -n "$GEMINI_HOME" ] && [ "$AGY_ACP_FORCE_FILE_STORAGE" = 1 ] || exit 1
mkdir -p "$GEMINI_HOME/antigravity-acp"
while read -r line; do
  id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"id":%s,"result":{"protocolVersion":1,"authMethods":[{"id":"oauth-personal"}]}}\n' "$id"
      ;;
    *'"method":"authenticate"'*)
      [ ! -f "$GEMINI_HOME/antigravity-acp/acp_token.json" ] || exit 2
      printf 'Sign in here: https://accounts.google.com/o/oauth2/auth?client_id=fake\n' >&2
      printf '{"auth":{"type":"oauth-personal"}}' > "$GEMINI_HOME/antigravity-acp/settings.json"
      printf 'fake-profile-token' > "$GEMINI_HOME/antigravity-acp/acp_token.json"
      printf '{"id":%s,"result":{}}\n' "$id"
      ;;
    *'"method":"session/new"'*)
      [ -f "$GEMINI_HOME/antigravity-acp/acp_token.json" ] || exit 3
      printf '{"id":%s,"result":{"sessionId":"profile-session"}}\n' "$id"
      ;;
    *'"method":"session/prompt"'*)
      printf '{"method":"session/update","params":{"sessionId":"profile-session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"%s"}}}}\n' "$GEMINI_HOME"
      printf '{"id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
      ;;
  esac
done
