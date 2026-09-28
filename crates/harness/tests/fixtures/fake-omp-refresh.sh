#!/bin/sh
# Fake Omp ACP agent: session/new advertises model-a (thinking through high),
# and switching to model-b rewrites the thinking ladder to include xhigh/max
# PLUS resets current — the Omp 18.2.5 live behavior. The set_config_option
# RESPONSE carries the refreshed configOptions (like the real server).
emit() { printf '%s\n' "$1"; }
rid() { printf '%s' "$1" | sed 's/.*"id":\([0-9]*\).*/\1/'; }
has() { case "$1" in *"$2"*) return 0 ;; *) return 1 ;; esac; }
update() { emit "{\"method\":\"session/update\",\"params\":{\"sessionId\":\"$SID\",\"update\":$1}}"; }
MODEL="model-a"
THINKING='[{"value":"off","name":"Off"},{"value":"auto","name":"Auto"},{"value":"minimal","name":"minimal"},{"value":"low","name":"low"},{"value":"medium","name":"medium"},{"value":"high","name":"high"}]'
THINK_CUR="high"
config_options() {
  printf '[{"id":"thinking","name":"Thinking","category":"thought_level","type":"select","currentValue":"%s","options":%s},{"id":"model","name":"Model","category":"model","type":"select","currentValue":"%s","options":[{"value":"model-a","name":"Model A"},{"value":"model-b","name":"Model B"}]}]' "$THINK_CUR" "$THINKING" "$MODEL"
}
read -r line || exit 1 # initialize
emit "{\"id\":$(rid "$line"),\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{\"loadSession\":true}}}"
read -r line || exit 1 # session/new
SID="s-omp"
emit "{\"id\":$(rid "$line"),\"result\":{\"sessionId\":\"s-omp\",\"configOptions\":$(config_options)}}"
CONFIG_SETS=""
read -r promptline || exit 1
while has "$promptline" '"method":"session/set_config_option"'; do
  if has "$promptline" '"configId":"model"'; then
    MODEL="model-b"
    THINKING='[{"value":"off","name":"Off"},{"value":"auto","name":"Auto"},{"value":"minimal","name":"minimal"},{"value":"low","name":"low"},{"value":"medium","name":"medium"},{"value":"high","name":"high"},{"value":"xhigh","name":"xhigh"},{"value":"max","name":"max"}]'
    THINK_CUR="high"
  elif has "$promptline" '"configId":"thinking"'; then
    if has "$promptline" '"value":"xhigh"'; then THINK_CUR="xhigh"; else THINK_CUR="other"; fi
  fi
  CONFIG_SETS="$CONFIG_SETS $promptline"
  emit "{\"id\":$(rid "$promptline"),\"result\":{\"configOptions\":$(config_options)}}"
  read -r promptline || exit 1
done
has "$promptline" '"method":"session/prompt"' || exit 1
pid=$(rid "$promptline")
if has "$CONFIG_SETS" '"configId":"model"' && has "$CONFIG_SETS" '"value":"model-b"' \
  && has "$CONFIG_SETS" '"configId":"thinking"' && has "$CONFIG_SETS" '"value":"xhigh"'; then
  update '{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"refreshed"}}'
  emit "{\"id\":$pid,\"result\":{\"stopReason\":\"end_turn\"}}"
else
  emit "{\"id\":$pid,\"result\":{\"stopReason\":\"refusal\"}}"
fi
