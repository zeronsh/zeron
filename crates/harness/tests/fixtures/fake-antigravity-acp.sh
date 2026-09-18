#!/bin/sh
# fake antigravity acp server for zeron-harness tests.
#
# mirrors the wire shapes of agy_acp_server 1.1.1: effort baked into model
# ids, a default/auto_edit/yolo mode select, auth_required (-32000) on
# session/new for a cwd containing "needs-login", and an `authenticate` that
# prints its sign-in url to stderr like the real browser flow. the prompt
# reply echoes every config option zeron set.

emit() { printf '%s\n' "$1"; }
rid() { printf '%s' "$1" | sed 's/.*"id":\([0-9]*\).*/\1/'; }
has() { case "$1" in *"$2"*) return 0 ;; *) return 1 ;; esac; }

OPTIONS='[{"id":"model","name":"Model","category":"model","type":"select","currentValue":"gemini-3.7-flash-high","options":[{"value":"gemini-3.7-flash-high","name":"Gemini 3.7 Flash (High)","description":"Gemini 3.7 Flash model with high thinking level"},{"value":"gemini-3.7-flash-medium","name":"Gemini 3.7 Flash (Medium)"},{"value":"gemini-3.7-flash-low","name":"Gemini 3.7 Flash (Low)"},{"value":"gemini-pro-agent","name":"Gemini 3.1 Pro (High)"},{"value":"gemini-3.1-pro-low","name":"Gemini 3.1 Pro (Low)"}]},{"id":"mode","name":"Session Mode","category":"mode","type":"select","currentValue":"default","options":[{"value":"default","name":"Default"},{"value":"auto_edit","name":"Auto Edit"},{"value":"yolo","name":"YOLO"}]}]'

AUTHED=0
REJECT_MODEL=0
SETS=""
while read -r line; do
  has "$line" '"id":' || continue
  id=$(rid "$line")
  if has "$line" '"method":"initialize"'; then
    emit "{\"id\":$id,\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{\"loadSession\":true},\"authMethods\":[{\"id\":\"oauth-personal\",\"name\":\"Log in with Google\"}]}}"
  elif has "$line" '"method":"session/new"'; then
    if has "$line" 'reject-model'; then
      REJECT_MODEL=1
    fi
    if has "$line" 'needs-login' && [ "$AUTHED" -eq 0 ]; then
      emit "{\"id\":$id,\"error\":{\"code\":-32000,\"message\":\"Authentication required\"}}"
    else
      emit "{\"id\":$id,\"result\":{\"sessionId\":\"agy-1\",\"configOptions\":$OPTIONS}}"
      emit "{\"method\":\"session/update\",\"params\":{\"sessionId\":\"agy-1\",\"update\":{\"sessionUpdate\":\"available_commands_update\",\"availableCommands\":[{\"name\":\"plan\",\"description\":\"Plan carefully\"},{\"name\":\"logout\",\"description\":\"Log out and clear stored credentials.\"}]}}}"
    fi
  elif has "$line" '"method":"authenticate"'; then
    has "$line" '"methodId":"oauth-personal"' || exit 1
    printf 'Sign in here: https://accounts.google.com/o/oauth2/auth?client_id=fake\n' >&2
    sleep 0.2
    AUTHED=1
    emit "{\"id\":$id,\"result\":{}}"
  elif has "$line" '"method":"logout"'; then
    AUTHED=0
    emit "{\"id\":$id,\"result\":{}}"
  elif has "$line" '"method":"session/set_config_option"'; then
    if [ "$REJECT_MODEL" -eq 1 ] && has "$line" '"configId":"model"'; then
      emit "{\"id\":$id,\"error\":{\"code\":-32602,\"message\":\"model unavailable\"}}"
    else
      set=$(printf '%s' "$line" | sed 's/.*"configId":"\([^"]*\)".*"value":"\([^"]*\)".*/\1=\2/')
      SETS="$SETS$set;"
      emit "{\"id\":$id,\"result\":{\"configOptions\":$OPTIONS}}"
    fi
  elif has "$line" '"method":"session/prompt"'; then
    emit "{\"method\":\"session/update\",\"params\":{\"sessionId\":\"agy-1\",\"update\":{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"sets:$SETS\"}}}}"
    emit "{\"id\":$id,\"result\":{\"stopReason\":\"end_turn\"}}"
    exit 0
  else
    emit "{\"id\":$id,\"result\":{}}"
  fi
done
