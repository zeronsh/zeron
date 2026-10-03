#!/bin/sh
# Fake Claude Code CLI for zeron-harness tests.
#
# Reads the first stream-json user line from stdin, picks a scenario from the
# prompt text, and plays a scripted stream-json transcript on stdout —
# including control-channel round-trips read back from stdin. Frame shapes
# mirror live captures from CLI 2.1.228. Driven by
# crates/harness/tests/claude.rs.

read -r first || exit 1
# A run opens with an `initialize` control request (the SDK handshake); the
# real CLI answers it before the first user line. (Command discovery sends
# its own initialize as the only line — matched by its scenario below.)
initialized=false
while :; do
  case "$first" in
    *'"request_id":"zeron_initialize"'*)
      case "$first" in *'"perTaskStopAffordance":true'*) initialized=true ;; esac
      printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"zeron_initialize","response":{}}}'
      read -r first || exit 1
      ;;
    *) break ;;
  esac
done

emit() { printf '%s\n' "$1"; }
uuid_of() { printf '%s\n' "$1" | sed 's/.*"uuid":"\([^"]*\)".*/\1/'; }

case "$first" in

*scenario:command-echo*)
  content=$(printf '%s\n' "$first" | sed 's/.*"content":"\([^"]*\)".*/\1/')
  emit "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"$content\"}}}"
  emit '{"type":"result","subtype":"success","result":"echoed","usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-command-echo"}'
  ;;

*scenario:title*)
  tools_off=false
  system_set=false
  mcp_off=false
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --tools) shift; [ "$1" = "" ] && tools_off=true ;;
      --system-prompt) shift; case "$1" in "You generate session titles."*) system_set=true ;; esac ;;
      --strict-mcp-config) mcp_off=true ;;
      --dangerously-skip-permissions) exit 1 ;;
    esac
    shift
  done
  [ "$tools_off" = true ] && [ "$system_set" = true ] && [ "$mcp_off" = true ] || exit 1
  emit '{"type":"control_request","request_id":"title-tool","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"touch should-not-exist"}}}'
  read -r response || exit 1
  case "$response" in *'"behavior":"deny"'*) ;; *) exit 1 ;; esac
  emit '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Fix Login Flow"}}}'
  emit '{"type":"result","subtype":"success","result":"Fix Login Flow","usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-title"}'
  ;;


*scenario:happy*)
  emit '{"type":"system","subtype":"init","model":"claude-fable-5","tools":["Bash","Read"],"cwd":"/tmp","session_id":"sess-1"}'
  # Re-emitted init mid-run (background-task wakeup): must be deduped.
  emit '{"type":"system","subtype":"init","model":"claude-fable-5","tools":["Bash","Read"],"cwd":"/tmp","session_id":"sess-1"}'
  # Post-init system subtypes the 2.1.x CLI emits (thinking accounting,
  # session state): must be tolerated and dropped.
  emit '{"type":"system","subtype":"thinking_tokens","tokens":12}'
  emit '{"type":"system","subtype":"session_state_changed","state":"running"}'
  emit '{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"pondering"}}}'
  emit '{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hello"}}}'
  # Subagent frames (parent_tool_use_id set): tagged, never in the parent feed.
  emit '{"type":"stream_event","parent_tool_use_id":"sub-1","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"SUBAGENT"}}}'
  emit '{"type":"assistant","parent_tool_use_id":"sub-1","message":{"content":[{"type":"tool_use","id":"sub-tool","name":"Bash","input":{"command":"echo sub"}}]}}'
  emit '{"type":"user","parent_tool_use_id":"sub-1","message":{"content":[{"type":"tool_result","tool_use_id":"sub-tool","is_error":false}]}}'
  emit '{"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"text","text":"Hello"},{"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"ls -la"}},{"type":"tool_use","id":"tool-2","name":"mcp__linear__search","input":{"q":"bug"}}]}}'
  emit '{"type":"user","parent_tool_use_id":null,"message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","is_error":false},{"type":"tool_result","tool_use_id":"tool-2","is_error":true}]}}'
  # Informational rate-limit status: stays quiet.
  emit '{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}'
  emit '{"type":"result","subtype":"success","result":"done!","errors":[],"usage":{"input_tokens":10,"output_tokens":20},"session_id":"sess-1","total_cost_usd":0.01}'
  ;;

*scenario:wake*)
  # Eager-done + wake, the live-verified 2.1.228 background-subagent shape:
  # the parent turn settles with result #1 while the subagent still runs;
  # tagged subagent traffic continues; the CLI then wakes with a second init
  # (SAME session id) and settles the wake turn with result #2.
  emit '{"type":"system","subtype":"init","model":"claude-fable-5","tools":["Bash"],"cwd":"/tmp","session_id":"sess-wake"}'
  emit '{"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"toolu_agent","name":"Agent","input":{"description":"background task","run_in_background":true}}]}}'
  emit '{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"LAUNCHED"}}}'
  emit '{"type":"result","subtype":"success","result":"LAUNCHED","errors":[],"usage":{"input_tokens":5,"output_tokens":5},"session_id":"sess-wake"}'
  # Background subagent interior, after the eager done.
  emit '{"type":"stream_event","parent_tool_use_id":"toolu_agent","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"sub working"}}}'
  emit '{"type":"assistant","parent_tool_use_id":"toolu_agent","message":{"content":[{"type":"tool_use","id":"sub-t1","name":"Bash","input":{"command":"sleep 1"}}]}}'
  emit '{"type":"user","parent_tool_use_id":"toolu_agent","message":{"content":[{"type":"tool_result","tool_use_id":"sub-t1","is_error":false}]}}'
  # The wake turn: re-init (deduped), untagged output, second result.
  emit '{"type":"system","subtype":"init","model":"claude-fable-5","tools":["Bash"],"cwd":"/tmp","session_id":"sess-wake"}'
  emit '{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"subagent finished"}}}'
  emit '{"type":"result","subtype":"success","result":"wrapped up","errors":[],"usage":{"input_tokens":3,"output_tokens":3},"session_id":"sess-wake"}'
  ;;

*scenario:askuser*)
  emit '{"type":"system","subtype":"init","model":"claude-fable-5","tools":["Bash"],"cwd":"/tmp","session_id":"sess-ask"}'
  # A plain tool permission request: must be auto-allowed.
  emit '{"type":"control_request","request_id":"cr-0","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"}}}'
  read -r resp0 || exit 1
  case "$resp0" in
  *'"request_id":"cr-0"'*'"behavior":"allow"'*) ;;
  *)
    emit '{"type":"result","subtype":"error_during_execution","errors":["bash tool was not allowed"],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-ask"}'
    exit 0
    ;;
  esac
  # AskUserQuestion: must be intercepted and answered via updatedInput.answers.
  emit '{"type":"control_request","request_id":"cr-1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"header":"Choice","question":"Pick one","options":["A","B"],"multiSelect":false}]}}}'
  read -r resp1 || exit 1
  case "$resp1" in
  *'"behavior":"allow"'*)
    case "$resp1" in
    *'"Pick one":"B"'*)
      emit '{"type":"result","subtype":"success","result":"answered","errors":[],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-ask"}'
      ;;
    *)
      emit '{"type":"result","subtype":"error_during_execution","errors":["answers missing from updatedInput"],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-ask"}'
      ;;
    esac
    ;;
  *)
    emit '{"type":"result","subtype":"error_during_execution","errors":["AskUserQuestion was denied"],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-ask"}'
    ;;
  esac
  ;;

*scenario:steer*)
  emit '{"type":"system","subtype":"init","model":"claude-fable-5","tools":[],"cwd":"/tmp","session_id":"sess-steer"}'
  emit '{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"first"}}}'
  # The queued steering user line, applied at "the step boundary" (here: now).
  read -r steer || exit 1
  case "$steer" in *'"priority":"now"'*) ;; *) exit 9 ;; esac
  # Old output continues until Claude actually consumes the input.
  emit '{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"-still-first"}}}'
  emit "$steer"
  content=$(printf '%s\n' "$steer" | sed 's/.*"content":"\([^"]*\)".*/\1/')
  emit "{\"type\":\"stream_event\",\"parent_tool_use_id\":null,\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"steered:$content\"}}}"
  emit '{"type":"result","subtype":"success","result":"steered","errors":[],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-steer"}'
  ;;

*scenario:lifecycle-now-steer*)
  # CLI 2.1.286, captured live: every uuid-tagged user line gets
  # `command_lifecycle` frames. A `now` steer aborts the streaming turn
  # (result, terminal_reason aborted_streaming), the first command is
  # cancelled and the steer's own turn starts. That turn then runs a tool
  # for longer than any quiet-time heuristic: no Done may land before the
  # steered turn's own result.
  fid=$(uuid_of "$first")
  emit "{\"type\":\"command_lifecycle\",\"command_uuid\":\"$fid\",\"state\":\"queued\"}"
  emit "{\"type\":\"command_lifecycle\",\"command_uuid\":\"$fid\",\"state\":\"started\"}"
  emit '{"type":"system","subtype":"init","model":"claude-sonnet-5-5","tools":["Bash"],"cwd":"/tmp","session_id":"sess-lc"}'
  emit "$first"
  emit '{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"essay"}}}'
  read -r steer || exit 1
  case "$steer" in *'"priority":"now"'*) ;; *) exit 9 ;; esac
  sid=$(uuid_of "$steer")
  emit "{\"type\":\"command_lifecycle\",\"command_uuid\":\"$sid\",\"state\":\"queued\"}"
  emit '{"type":"result","subtype":"success","is_error":false,"terminal_reason":"aborted_streaming","result":"essay","errors":[],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-lc"}'
  emit "{\"type\":\"command_lifecycle\",\"command_uuid\":\"$fid\",\"state\":\"cancelled\"}"
  emit "{\"type\":\"command_lifecycle\",\"command_uuid\":\"$sid\",\"state\":\"started\"}"
  emit '{"type":"system","subtype":"init","model":"claude-sonnet-5-5","tools":["Bash"],"cwd":"/tmp","session_id":"sess-lc"}'
  emit "$steer"
  emit '{"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"tool-sleep","name":"Bash","input":{"command":"sleep 8"}}]}}'
  sleep "${FAKE_CLAUDE_TOOL_SECS:-6}"
  emit '{"type":"user","parent_tool_use_id":null,"message":{"content":[{"type":"tool_result","tool_use_id":"tool-sleep","is_error":false}]}}'
  emit '{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"DONE"}}}'
  emit '{"type":"result","subtype":"success","is_error":false,"terminal_reason":"completed","result":"DONE","errors":[],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-lc"}'
  emit "{\"type\":\"command_lifecycle\",\"command_uuid\":\"$sid\",\"state\":\"completed\"}"
  ;;

*scenario:lifecycle-queued-behind-end*)
  # A message that reaches the CLI as its turn is finishing waits for that
  # turn's result, then starts its own turn: the first result is a
  # boundary, and the run's only Done is the queued message's.
  fid=$(uuid_of "$first")
  emit "{\"type\":\"command_lifecycle\",\"command_uuid\":\"$fid\",\"state\":\"started\"}"
  emit '{"type":"system","subtype":"init","model":"claude-sonnet-5-5","tools":[],"cwd":"/tmp","session_id":"sess-q"}'
  emit '{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"first answer"}}}'
  read -r steer || exit 1
  sid=$(uuid_of "$steer")
  emit "{\"type\":\"command_lifecycle\",\"command_uuid\":\"$sid\",\"state\":\"queued\"}"
  emit '{"type":"result","subtype":"success","is_error":false,"terminal_reason":"completed","result":"first answer","errors":[],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-q"}'
  emit "{\"type\":\"command_lifecycle\",\"command_uuid\":\"$fid\",\"state\":\"completed\"}"
  sleep 1
  emit "{\"type\":\"command_lifecycle\",\"command_uuid\":\"$sid\",\"state\":\"started\"}"
  emit "$steer"
  emit '{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"second answer"}}}'
  emit '{"type":"result","subtype":"success","is_error":false,"terminal_reason":"completed","result":"second answer","errors":[],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-q"}'
  emit "{\"type\":\"command_lifecycle\",\"command_uuid\":\"$sid\",\"state\":\"completed\"}"
  ;;

*scenario:lifecycle-queued-cancelled*)
  # A queued message cancelled before any turn takes it up: the result held
  # for it was the turn's real end, released at once — no quiet timer.
  fid=$(uuid_of "$first")
  emit "{\"type\":\"command_lifecycle\",\"command_uuid\":\"$fid\",\"state\":\"started\"}"
  emit '{"type":"system","subtype":"init","model":"claude-sonnet-5-5","tools":[],"cwd":"/tmp","session_id":"sess-qc"}'
  read -r steer || exit 1
  sid=$(uuid_of "$steer")
  emit "{\"type\":\"command_lifecycle\",\"command_uuid\":\"$sid\",\"state\":\"queued\"}"
  emit '{"type":"result","subtype":"success","is_error":false,"result":"only","errors":[],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-qc"}'
  emit "{\"type\":\"command_lifecycle\",\"command_uuid\":\"$fid\",\"state\":\"completed\"}"
  emit "{\"type\":\"command_lifecycle\",\"command_uuid\":\"$sid\",\"state\":\"cancelled\"}"
  exec sleep 30
  ;;

*scenario:reconfigure*)
  # A follow-up sent with another model and effort: the live process
  # adopts them (`set_model`, then `apply_flag_settings`) before the prompt
  # — the CLI reads stdin in order. Live-verified on 2.1.286.
  emit '{"type":"system","subtype":"init","model":"claude-sonnet-5-5","tools":[],"cwd":"/tmp","session_id":"sess-rc"}'
  emit '{"type":"result","subtype":"success","result":"first","errors":[],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-rc"}'
  read -r model || exit 1
  case "$model" in *'"subtype":"set_model"'*) ;; *) exit 11 ;; esac
  case "$model" in *'"model":"claude-haiku-4-5"'*) ;; *) exit 11 ;; esac
  read -r flags || exit 1
  case "$flags" in *'"subtype":"apply_flag_settings"'*) ;; *) exit 12 ;; esac
  case "$flags" in *'"effortLevel":"high"'*) ;; *) exit 13 ;; esac
  case "$flags" in *'"fastMode":null'*) ;; *) exit 14 ;; esac
  read -r prompt || exit 1
  case "$prompt" in *'"type":"user"'*) ;; *) exit 15 ;; esac
  case "$prompt" in *'switched'*) ;; *) exit 15 ;; esac
  emit '{"type":"system","subtype":"init","model":"claude-haiku-4-5","tools":[],"cwd":"/tmp","session_id":"sess-rc"}'
  emit "$prompt"
  emit '{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"on haiku"}}}'
  emit '{"type":"result","subtype":"success","result":"on haiku","errors":[],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-rc"}'
  # An unchanged configuration sends no control request at all.
  read -r again || exit 1
  case "$again" in *'"type":"user"'*) ;; *) exit 16 ;; esac
  case "$again" in *'same again'*) ;; *) exit 16 ;; esac
  emit "$again"
  emit '{"type":"result","subtype":"success","result":"same","errors":[],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-rc"}'
  ;;

*scenario:resume-killed-tasks*)
  # Captured live on 2.1.286: `--resume` of a session whose background tasks
  # died with its previous process. The CLI settles them with two empty
  # `result` frames (num_turns 0) while the new prompt is still queued —
  # they are not that prompt's end.
  fid=$(uuid_of "$first")
  emit '{"type":"system","subtype":"task_notification","task_id":"a-1","status":"stopped"}'
  emit '{"type":"system","subtype":"task_notification","task_id":"b-1","status":"stopped"}'
  emit "{\"type\":\"command_lifecycle\",\"command_uuid\":\"$fid\",\"state\":\"queued\"}"
  emit '{"type":"system","subtype":"init","model":"claude-sonnet-5-5","tools":[],"cwd":"/tmp","session_id":"sess-rk"}'
  emit '{"type":"result","subtype":"success","is_error":false,"num_turns":0,"result":"","errors":[],"usage":{"input_tokens":0,"output_tokens":0},"session_id":"sess-rk"}'
  emit '{"type":"system","subtype":"init","model":"claude-sonnet-5-5","tools":[],"cwd":"/tmp","session_id":"sess-rk"}'
  emit '{"type":"result","subtype":"success","is_error":false,"num_turns":0,"result":"","errors":[],"usage":{"input_tokens":0,"output_tokens":0},"session_id":"sess-rk"}'
  emit "{\"type\":\"command_lifecycle\",\"command_uuid\":\"$fid\",\"state\":\"started\"}"
  emit "$first"
  emit '{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"PONG"}}}'
  emit '{"type":"result","subtype":"success","is_error":false,"num_turns":1,"terminal_reason":"completed","result":"PONG","errors":[],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-rk"}'
  emit "{\"type\":\"command_lifecycle\",\"command_uuid\":\"$fid\",\"state\":\"completed\"}"
  ;;

*scenario:superseded-steers*)
  # CLI 2.1.280 with rapid `now` steers: the second interrupts the turn the
  # first started before it is replayed; only the last steer is replayed.
  emit '{"type":"system","subtype":"init","model":"claude-fable-5","tools":[],"cwd":"/tmp","session_id":"sess-sup"}'
  emit '{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"story"}}}'
  read -r steer1 || exit 1
  read -r steer2 || exit 1
  emit '{"type":"result","subtype":"success","result":"story","errors":[],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-sup"}'
  emit '{"type":"system","subtype":"init","model":"claude-fable-5","tools":[],"cwd":"/tmp","session_id":"sess-sup"}'
  emit '{"type":"result","subtype":"error_during_execution","errors":[],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-sup"}'
  emit '{"type":"system","subtype":"init","model":"claude-fable-5","tools":[],"cwd":"/tmp","session_id":"sess-sup"}'
  emit "$steer2"
  emit '{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"answered-both"}}}'
  emit '{"type":"result","subtype":"success","result":"answered-both","errors":[],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-sup"}'
  ;;

*scenario:absorbed-steer*)
  # A steer whose replay never comes: the turn end must not be held forever.
  emit '{"type":"system","subtype":"init","model":"claude-fable-5","tools":[],"cwd":"/tmp","session_id":"sess-abs"}'
  read -r steer || exit 1
  emit '{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"absorbed"}}}'
  emit '{"type":"result","subtype":"success","result":"absorbed","errors":[],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-abs"}'
  exec sleep 30
  ;;

*scenario:stop-in-place*)
  # A turn stop is the `interrupt` control request with `cancel_queued`: the
  # CLI ends the turn (an error_during_execution result carrying only its
  # diagnostic) and keeps reading stdin. Background tasks survive because the
  # run declared perTaskStopAffordance (live-verified 2.1.285).
  [ "$initialized" = true ] || exit 7
  emit '{"type":"system","subtype":"init","model":"claude-fable-5","tools":["Bash"],"cwd":"/tmp","session_id":"sess-stop"}'
  emit '{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"bg-1","task_type":"local_agent","description":"scout"}]}'
  emit '{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"working"}}}'
  read -r stop || exit 1
  case "$stop" in *'"subtype":"interrupt"'*) ;; *) exit 8 ;; esac
  case "$stop" in *'"cancel_queued":true'*) ;; *) exit 8 ;; esac
  rid=$(printf '%s\n' "$stop" | sed 's/.*"request_id":"\([^"]*\)".*/\1/')
  emit "{\"type\":\"control_response\",\"response\":{\"subtype\":\"success\",\"request_id\":\"$rid\",\"response\":{\"still_queued\":[]}}}"
  emit '{"type":"user","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]},"parent_tool_use_id":null}'
  emit '{"type":"result","subtype":"error_during_execution","errors":["[ede_diagnostic] result_type=user last_content_type=n/a stop_reason=tool_use"],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-stop"}'
  # Same process, next turn.
  read -r next || exit 1
  case "$next" in *'"type":"image"'*) img=yes ;; *) img=no ;; esac
  emit "$next"
  emit "{\"type\":\"stream_event\",\"parent_tool_use_id\":null,\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"resumed:image=$img\"}}}"
  emit '{"type":"system","subtype":"background_tasks_changed","tasks":[]}'
  emit '{"type":"result","subtype":"success","result":"resumed","errors":[],"usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-stop"}'
  # A stop between turns: answered, and no result follows.
  read -r idle_stop || exit 1
  case "$idle_stop" in *'"subtype":"interrupt"'*) ;; *) exit 9 ;; esac
  rid=$(printf '%s\n' "$idle_stop" | sed 's/.*"request_id":"\([^"]*\)".*/\1/')
  emit "{\"type\":\"control_response\",\"response\":{\"subtype\":\"success\",\"request_id\":\"$rid\",\"response\":{\"still_queued\":[]}}}"
  cat >/dev/null
  ;;

*scenario:interrupt*)
  emit '{"type":"system","subtype":"init","model":"claude-fable-5","tools":[],"cwd":"/tmp","session_id":"sess-int"}'
  # Wedge without reading stdin — forces the SIGTERM escalation path.
  exec sleep 30
  ;;

*'"subtype":"initialize"'*)
  if [ -f .command-fixture ]; then
    rid=$(printf '%s' "$first" | sed 's/.*"request_id":"\([^"]*\)".*/\1/')
    name=$(cat .command-fixture)
    emit "{\"type\":\"control_response\",\"response\":{\"subtype\":\"success\",\"request_id\":\"$rid\",\"response\":{\"commands\":[{\"name\":\"$name\"}]}}}"
    exec sleep 30
  fi
  # Command discovery: the initialize control request arrives as the FIRST
  # stdin line (no user message ever follows). Shape mirrors 2.1.228's
  # control_response: commands under response.response.
  rid=$(printf '%s\n' "$first" | sed 's/.*"request_id":"\([^"]*\)".*/\1/')
  emit "{\"type\":\"control_response\",\"response\":{\"subtype\":\"success\",\"request_id\":\"$rid\",\"response\":{\"commands\":[{\"name\":\"review\",\"description\":\"Review a pull request\",\"argumentHint\":\"[pr number]\"},{\"name\":\"compact\",\"description\":\"Compact the conversation\",\"argumentHint\":\"\"},{\"name\":\"\",\"description\":\"nameless: dropped\"}],\"output_style\":\"default\"}}}"
  # Stay alive until the driver tears us down, like the real CLI would.
  exec sleep 30
  ;;

*scenario:error*)
  emit '{"type":"system","subtype":"init","model":"claude-fable-5","tools":[],"cwd":"/tmp","session_id":"sess-err"}'
  # Terse assistant-level error code with no content.
  emit '{"type":"assistant","parent_tool_use_id":null,"message":{"content":[]},"error":"rate_limit"}'
  # Hard-rejected claude.ai usage window.
  emit '{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour"}}'
  # Result error with an EMPTY errors array: needs fallback wording.
  emit '{"type":"result","subtype":"error_max_turns","errors":[],"usage":{"input_tokens":1,"output_tokens":2},"session_id":"sess-err"}'
  ;;

*)
  emit '{"type":"result","subtype":"error_during_execution","errors":["unknown scenario"],"usage":{"input_tokens":0,"output_tokens":0},"session_id":"sess-x"}'
  ;;
esac
