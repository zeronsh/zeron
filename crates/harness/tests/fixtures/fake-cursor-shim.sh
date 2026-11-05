#!/bin/sh
# Fake zeron cursor shim for zeron-harness tests: speaks the shim's JSONL
# protocol (see crates/harness/src/cursor/shim.mjs) without node or the SDK.
# Driven by crates/harness/tests/cursor.rs.

emit() { printf '%s\n' "$1"; }

# Models mode (argv, no stdin protocol): one catalog frame, real 1.0.28
# shapes — parameterized Auto + its bare `default` alias twin (skipped by the
# harness) + a plain model + one whose default variant is Max Mode.
if [ "$1" = "models" ]; then
  emit '{"ev":"models","items":[{"id":"auto-smart","displayName":"Auto","parameters":[{"id":"optimize_for","displayName":"Optimize For","values":[{"value":"intelligence","displayName":"Intelligence"},{"value":"balanced","displayName":"Balance"},{"value":"cost","displayName":"Cost"}]}],"variants":[{"params":[{"id":"optimize_for","value":"balanced"}],"displayName":"Auto","isDefault":true}]},{"id":"default","displayName":"Auto","aliases":["auto"]},{"id":"claude-fable-5","displayName":"Claude Fable 5","description":"Anthropic frontier","parameters":[{"id":"thinking","values":[{"value":"enabled"},{"value":"disabled"}]}]},{"id":"grok-4.7","displayName":"Grok 4.7","parameters":[{"id":"context","values":[{"value":"256k"},{"value":"500k"}]}],"variants":[{"params":[{"id":"context","value":"500k"}],"displayName":"Grok 4.7","isDefault":true,"isMaxMode":true}]}]}'
  exit 0
fi

read -r first || exit 1
case "$first" in
*'"op":"run"'*) ;;
*) emit '{"ev":"fatal","message":"expected op run first"}'; exit 1 ;;
esac

case "$first" in

*scenario:model-default*)
  case "$first" in
    *'"modelOptions":{"optimize_for":"balanced"}'*) ;;
    *) emit '{"ev":"fatal","message":"missing catalog default"}'; exit 1 ;;
  esac
  emit '{"ev":"ready","agentId":"agent-options","model":"auto-smart"}'
  emit '{"ev":"turn","status":"finished"}'
  read -r next || exit 0
  ;;

*scenario:model-legacy*)
  case "$first" in
    *'"modelOptions":{"optimizeFor":"speed"}'*) ;;
    *) emit '{"ev":"fatal","message":"legacy option conflicts with catalog default"}'; exit 1 ;;
  esac
  emit '{"ev":"ready","agentId":"agent-options","model":"auto-smart"}'
  emit '{"ev":"turn","status":"finished"}'
  read -r next || exit 0
  ;;

*scenario:model-switch*)
  emit '{"ev":"ready","agentId":"agent-options","model":"auto-smart"}'
  emit '{"ev":"turn","status":"finished"}'
  read -r next || exit 0
  case "$next" in
    *'"modelOptions":{"thinking":"enabled"}'*) ;;
    *) emit '{"ev":"turn","status":"error","error":"switch missing catalog default"}'; exit 0 ;;
  esac
  emit '{"ev":"steered"}'
  emit '{"ev":"turn","status":"finished"}'
  read -r next || exit 0
  ;;

*scenario:model-maxmode*)
  case "$first" in
    *'"modelOptions":{}'*) ;;
    *) emit '{"ev":"fatal","message":"max mode default was sent"}'; exit 1 ;;
  esac
  emit '{"ev":"ready","agentId":"agent-options","model":"grok-4.7"}'
  emit '{"ev":"turn","status":"finished"}'
  read -r next || exit 0
  ;;

*scenario:model-override*)
  case "$first" in
    *'"modelOptions":{"optimize_for":"intelligence"}'*) ;;
    *) emit '{"ev":"fatal","message":"explicit option was overwritten"}'; exit 1 ;;
  esac
  emit '{"ev":"ready","agentId":"agent-options","model":"auto-smart"}'
  emit '{"ev":"turn","status":"finished"}'
  read -r next || exit 0
  ;;

*scenario:mcp*)
  case "$first" in
    *'"mcp":{"args":["mcp"],"command":"/path with spaces/zeron","env":{"ZERON_CHAT_ID":"origin-chat"},"name":"zeron"}'*) ;;
    *) exit 1 ;;
  esac
  emit '{"ev":"ready","agentId":"agent-1","model":"auto"}'
  emit '{"ev":"text","text":"mcp configured"}'
  emit '{"ev":"turn","status":"finished"}'
  read -r next || exit 0
  ;;

*scenario:burst*)
  exec node "$(dirname "$0")/cursor-steering-peer.mjs" "$first"
  ;;

*scenario:followup-crash*)
  emit '{"ev":"ready","agentId":"agent-crash-followup","model":"composer-2.5"}'
  emit '{"ev":"turn","status":"finished"}'
  read -r next || exit 0
  echo "followup exploded" >&2
  exit 3
  ;;

*scenario:happy*)
  emit '{"ev":"ready","agentId":"agent-1","model":"composer-2.5"}'
  emit '{"ev":"thinking","text":"planning"}'
  emit '{"ev":"text","text":"Hello from cursor"}'
  emit '{"ev":"tool","phase":"start","id":"c1","name":"shell","args":{"command":"ls -la"}}'
  emit '{"ev":"tool","phase":"end","id":"c1","name":"shell","args":{"command":"ls -la"},"error":false}'
  # A spawned subagent: the task chip on the parent feed, its interior tagged.
  emit '{"ev":"tool","phase":"start","id":"task1","name":"task","args":{"description":"scan repo"}}'
  emit '{"ev":"text","text":"sub scanning","parent":"task1"}'
  emit '{"ev":"tool","phase":"start","id":"s1","name":"grep","args":{"pattern":"todo"},"parent":"task1"}'
  emit '{"ev":"tool","phase":"end","id":"s1","name":"grep","args":{"pattern":"todo"},"error":false,"parent":"task1"}'
  emit '{"ev":"tool","phase":"end","id":"task1","name":"task","args":{"description":"scan repo"},"error":false}'
  # Unknown frame kinds must be tolerated.
  emit '{"ev":"someNewThing","x":1}'
  emit '{"ev":"usage","input":11,"output":5}'
  emit '{"ev":"turn","status":"finished"}'
  # Parked: wait for a follow-up or stdin EOF.
  read -r next || exit 0
  case "$next" in
  *'"op":"steer"'*)
    emit '{"ev":"steered"}'
    emit '{"ev":"text","text":"second turn"}'
    emit '{"ev":"turn","status":"finished"}'
    ;;
  esac
  exit 0
  ;;

*scenario:stop-wedge*)
  # The SDK's cancel wedges: a stop is never answered.
  emit '{"ev":"ready","agentId":"agent-stop","model":"auto"}'
  emit '{"ev":"text","text":"working"}'
  read -r msg || exit 0
  exec sleep 30
  ;;

*scenario:images*)
  case "$first" in
    *'"images":[{"data":"'*'"mimeType":"image/png","type":"image"}]'*) ;;
    *) emit '{"ev":"fatal","message":"images missing"}'; exit 1 ;;
  esac
  emit '{"ev":"ready","agentId":"agent-img","model":"auto"}'
  emit '{"ev":"turn","status":"finished"}'
  read -r next || exit 0
  case "$next" in
    *'"images":[{"data":"'*) emit '{"ev":"steered"}'; emit '{"ev":"text","text":"steer image"}' ;;
    *) emit '{"ev":"steered"}'; emit '{"ev":"text","text":"steer image missing"}' ;;
  esac
  emit '{"ev":"turn","status":"finished"}'
  read -r next || exit 0
  ;;

*scenario:image-cap*)
  # Only the small attachment is inlined; the large one rides as a path ref.
  case "$first" in
    *'"images":[{"data":"iVBORw0KGgpzbWFsbA==","mimeType":"image/png","type":"image"}]'*) ;;
    *) emit '{"ev":"fatal","message":"image caps not applied"}'; exit 1 ;;
  esac
  emit '{"ev":"ready","agentId":"agent-img","model":"auto"}'
  emit '{"ev":"turn","status":"finished"}'
  read -r next || exit 0
  ;;

*scenario:send-bounds*)
  case "$first" in
    *'"imageBudget":8388608,'*'"silenceTimeoutMs":1234'*) ;;
    *) emit '{"ev":"fatal","message":"send bounds missing"}'; exit 1 ;;
  esac
  emit '{"ev":"ready","agentId":"agent-bounds","model":"auto"}'
  emit '{"ev":"turn","status":"finished"}'
  read -r next || exit 0
  ;;

*scenario:interrupt-fatal*)
  emit '{"ev":"ready","agentId":"agent-int-fatal","model":"auto"}'
  emit '{"ev":"text","text":"working"}'
  read -r msg || exit 0
  emit '{"ev":"fatal","message":"transport closed during cancellation"}'
  exit 1
  ;;

*scenario:interrupt*)
  emit '{"ev":"ready","agentId":"agent-int","model":"auto"}'
  emit '{"ev":"text","text":"working"}'
  read -r msg || exit 0
  case "$msg" in
  *'"op":"interrupt"'*)
    emit '{"ev":"turn","status":"cancelled"}'
    ;;
  esac
  exit 0
  ;;

*scenario:fatal*)
  emit '{"ev":"fatal","message":"Cursor SDK is not authenticated (its login is separate from `cursor-agent login`): set CURSOR_API_KEY from cursor.com/settings, then retry."}'
  exit 1
  ;;

*scenario:crash*)
  emit '{"ev":"ready","agentId":"agent-c","model":"auto"}'
  echo "shim exploded" >&2
  exit 3
  ;;

*)
  emit '{"ev":"fatal","message":"unknown scenario"}'
  exit 1
  ;;
esac
