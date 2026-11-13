#!/usr/bin/env python3
"""Writes the fake-omp scenarios (*.ndjson beside this file), trimmed from
omp 18.20.0 RPC recordings. Regenerate after editing:

    python3 crates/harness/tests/fixtures/omp/scenarios.py crates/harness/tests/fixtures/omp
"""
import json, os, sys
out = sys.argv[1]
U = lambda text: {"role":"user","content":[{"type":"text","text":text}],"attribution":"user","timestamp":1794568177194}
def A(content, stop="stop", usage=True, err=None):
    m = {"role":"assistant","content":content,"api":"openai-codex-responses","provider":"openai-codex","model":"gpt-5.6-luna","stopReason":stop,"timestamp":1794568177214}
    if usage: m["usage"] = {"input":6196,"output":5,"cacheRead":0,"cacheWrite":0,"totalTokens":6201,"cost":{"total":0.0012}}
    if err: m["errorMessage"] = err
    return m
def upd(kind, index=0, **kw):
    e = {"type":kind,"contentIndex":index}; e.update(kw)
    return {"type":"message_update","message":{"role":"assistant"},"assistantMessageEvent":e,"messageId":"msg-2"}
def emit(f): return {"emit":f}
def prompt(var="$P1", **equals):
    s = {"expect":"prompt","bind":{var:["id"]}}
    if equals: s["equals"] = equals
    return s
def ack(var="$P1", data=None):
    r = {"id":var,"type":"response","command":"prompt","success":True}
    if data is not None: r["data"] = data
    return emit(r)
def result(var="$P1", status="completed", settled=True, error=None):
    r = {"type":"prompt_result","id":var,"agentInvoked":True,"status":status,"sessionSettled":settled}
    if error: r["error"] = error
    return emit(r)
def text_turn(user, reply, msg="msg-2"):
    return [emit({"type":"agent_start"}), emit({"type":"turn_start"}),
        emit({"type":"message_start","message":U(user),"messageId":"msg-1"}),
        emit({"type":"message_end","message":U(user),"messageId":"msg-1"}),
        emit({"type":"message_start","message":A([{"type":"text","text":""}], usage=False),"messageId":msg}),
        emit(upd("text_start", block={"type":"text","text":""})),
        emit(upd("text_delta", delta=reply)),
        emit(upd("text_end", content=reply, block={"type":"text"})),
        emit({"type":"message_end","message":A([{"type":"text","text":reply}]),"messageId":msg}),
        emit({"type":"turn_end","message":A([{"type":"text","text":reply}]),"toolResults":[]})]
END = lambda: emit({"type":"agent_end","messages":[],"isTerminal":True,"yielded":True})
BASH = "call_7fecf74cc52d4ad286ff3930acd4f2aa|fc_0f909dd243173c65016af6f19aa15c87d091e94d66647853e5"
def tool_start(cmd, id=BASH, name="bash", args=None):
    return emit({"type":"tool_execution_start","toolCallId":id,"toolName":name,"args":args or {"command":cmd,"timeout":10,"pty":False,"async":False},"intent":"Running requested command"})
def tool_end(text, id=BASH, name="bash", error=False):
    f = {"type":"tool_execution_end","toolCallId":id,"toolName":name,"result":{"content":[{"type":"text","text":text}],"details":{}}}
    if error: f["isError"] = True
    return emit(f)

S = {}
S["simple"] = [prompt(message="Reply with only the word ok"), ack()] + text_turn("Reply with only the word ok", "ok") + [
    emit({"type":"extension_ui_request","id":"w1","method":"setWidget","widgetKey":"autoresearch"}),
    emit({"type":"some_future_frame","detail":1}), END(), result(), emit({"type":"session_settled"})]
S["tool"] = [prompt(), ack(), emit({"type":"agent_start"}), emit({"type":"turn_start"}),
    emit({"type":"message_start","message":U("list files"),"messageId":"msg-1"}),
    emit({"type":"message_start","message":A([{"type":"toolCall","id":BASH,"name":"bash","arguments":{}}],usage=False),"messageId":"msg-2"}),
    emit(upd("toolcall_start", block={"type":"toolCall","id":BASH,"name":"bash","arguments":{}})),
    emit(upd("toolcall_delta", delta="{\"command\":\"ls\"}")),
    emit({"type":"message_end","message":A([{"type":"toolCall","id":BASH,"name":"bash","arguments":{"command":"ls"}}],stop="toolUse"),"messageId":"msg-2"}),
    tool_start("ls"),
    emit({"type":"tool_execution_update","toolCallId":BASH,"toolName":"bash","args":{"command":"ls"},"partialResult":{"content":[{"type":"text","text":"a.txt\n"}]}}),
    tool_end("a.txt\nb.txt"),
    emit({"type":"message_start","message":{"role":"toolResult","toolCallId":BASH,"toolName":"bash","content":[{"type":"text","text":"a.txt\nb.txt"}],"isError":False},"messageId":"msg-3"}),
    emit({"type":"turn_end","message":A([],stop="toolUse"),"toolResults":[]}), emit({"type":"turn_start"}),
    emit({"type":"message_start","message":A([{"type":"text","text":""}],usage=False),"messageId":"msg-4"}),
    emit(upd("text_delta", delta="Two files.")),
    emit({"type":"message_end","message":A([{"type":"text","text":"Two files."}]),"messageId":"msg-4"}),
    END(), result()]
S["chunk"] = [prompt(), ack(), emit({"type":"agent_start"}),
    emit({"type":"message_start","message":A([{"type":"text","text":""}],usage=False),"messageId":"msg-2"}),
    {"chunked":{"type":"message_end","message":A([{"type":"text","text":"é" * 4000 + "END"}]),"messageId":"msg-2"},"pieces":4},
    END(), result()]
S["local"] = [prompt(message="/session info"),
    emit({"type":"command_output","text":"Session: 01a1d49f-be6e-72f8-b068-12a6d1bc8460\nTitle: undefined"}),
    ack(data={"agentInvoked":False})]
S["error"] = [prompt(), ack(), emit({"type":"agent_start"}),
    emit({"type":"message_start","message":U("hi"),"messageId":"msg-1"}),
    emit({"type":"auto_retry_start","attempt":1,"maxAttempts":3,"delayMs":10,"errorMessage":"429"}),
    emit({"type":"message_end","message":A([],stop="error",usage=False,err="429 Too Many Requests: rate limited"),"messageId":"msg-2"}),
    END(), result(status="error", error={"message":"429 Too Many Requests: rate limited","retryable":True,"httpStatus":429})]
S["stop"] = [prompt(), ack(), emit({"type":"agent_start"}), emit({"type":"turn_start"}),
    emit({"type":"message_start","message":U("sleep"),"messageId":"msg-1"}),
    tool_start("sleep 30"),
    {"expect":"abort_and_restore_queue","bind":{"$A":["id"]}},
    tool_end("Command aborted", error=True),
    emit({"type":"message_start","message":A([],stop="aborted",usage=False,err="Interrupted by user"),"messageId":"msg-5"}),
    emit({"type":"message_end","message":A([],stop="aborted",usage=False,err="Interrupted by user"),"messageId":"msg-5"}),
    END(),
    emit({"id":"$A","type":"response","command":"abort_and_restore_queue","success":True,"data":{"steering":[],"followUp":[]}}),
    result(status="aborted"), emit({"type":"session_settled"}),
    prompt("$P2", message="still there?", streamingBehavior="steer"), ack("$P2")] + text_turn("still there?", "yes") + [END(), result("$P2")]
S["steer"] = [prompt(), ack(), emit({"type":"agent_start"}), emit({"type":"turn_start"}),
    emit({"type":"message_start","message":U("Use bash to run exactly: sleep 4; echo done."),"messageId":"msg-1"}),
    tool_start("sleep 4; echo done"),
    prompt("$P2", message="Also include the word banana in your final reply.", streamingBehavior="steer"),
    emit({"type":"queue_update","steering":["Also include the word banana in your final reply."],"followUp":[],"liveSteered":0}),
    ack("$P2"),
    tool_end("done\n\n\nWall time: 4.04 seconds"),
    emit({"type":"turn_end","message":A([],stop="toolUse"),"toolResults":[]}), emit({"type":"turn_start"}),
    emit({"type":"message_start","message":A([{"type":"text","text":""}],usage=False),"messageId":"msg-4"}),
    emit(upd("text_delta", delta="Command completed: `done`.")),
    emit({"type":"message_end","message":A([{"type":"text","text":"Command completed: `done`."}]),"messageId":"msg-4"}),
    emit({"type":"queue_update","steering":[],"followUp":[],"liveSteered":0}), emit({"type":"turn_start"}),
    emit({"type":"message_start","message":U("Also include the word banana in your final reply."),"messageId":"msg-5"}),
    emit({"type":"message_start","message":A([{"type":"text","text":""}],usage=False),"messageId":"msg-6"}),
    emit(upd("text_delta", delta="banana")),
    emit({"type":"message_end","message":A([{"type":"text","text":"banana"}]),"messageId":"msg-6"}),
    END(), result("$P2"), result(), emit({"type":"session_settled"})]
APPROVAL = {"type":"extension_ui_request","id":"ap1","method":"select","title":"Allow tool: bash\nCommand: echo zeron-probe","options":["Approve","Approve bash for this session","Deny"]}
def approval(value, reply):
    return [prompt(), ack(), emit({"type":"agent_start"}), tool_start("echo zeron-probe"), emit(APPROVAL),
        {"expect":"extension_ui_response","equals":{"id":"ap1","value":value}},
        tool_end(reply, error=value == "Deny"),
        emit({"type":"message_end","message":A([{"type":"text","text":"done"}]),"messageId":"msg-4"}), END(), result()]
S["approval"] = approval("Approve", "zeron-probe")
S["approval-deny"] = approval("Deny", "Tool call denied by user: bash")
S["wake"] = [prompt(), ack()] + text_turn("start a background job", "started") + [END(), result(settled=False),
    {"sleep":100}, emit({"type":"agent_start"}),
    emit({"type":"message_start","message":A([{"type":"text","text":""}],usage=False),"messageId":"msg-9"}),
    emit(upd("text_delta", delta="job finished")),
    emit({"type":"message_end","message":A([{"type":"text","text":"job finished"}]),"messageId":"msg-9"}),
    END(), emit({"type":"session_settled"})]
S["no-model"] = [{"no_ready":True}, {"stderr":"No default model selected. Use /login, set an API key environment variable, or select a local model with /model or --model."}, {"exit":1}]
S["crash"] = [prompt(), ack(), emit({"type":"agent_start"}), {"stderr":"fatal: omp lost its session store"}, {"exit":3}]
S["mcp"] = [{"expect":"set_host_tools","bind":{"$H":["id"]},"equals":{"type":"set_host_tools"}},
    emit({"id":"$H","type":"response","command":"set_host_tools","success":True,"data":{"toolNames":["zeron_echo"]}}),
    prompt(), ack(), emit({"type":"agent_start"}),
    tool_start(None, id="t1", name="zeron_echo", args={"text":"hi"}),
    emit({"type":"host_tool_call","id":"hc1","toolCallId":"t1","toolName":"zeron_echo","arguments":{"text":"hi"}}),
    {"expect":"host_tool_result","equals":{"id":"hc1"}},
    tool_end("echo:hi", id="t1", name="zeron_echo"), END(), result()]
S["session-switch"] = [prompt(message="/drop"), ack(data={"agentInvoked":False}),
    {"expect":"get_state","bind":{"$S":["id"]}},
    emit({"id":"$S","type":"response","command":"get_state","success":True,"data":{"sessionId":"01a1d49f-0000-7000-8000-000000000002","model":{"id":"gpt-5.6-luna","provider":"openai-codex"},"isSettled":True}})]
S["local-steer"] = [prompt(), ack()] + text_turn("hi", "hello") + [END(), result(),
    prompt("$P2", message="/session info", streamingBehavior="steer"),
    emit({"type":"command_output","text":"Session: 01a1d49f-be6e-72f8-b068-12a6d1bc8460"}),
    ack("$P2", data={"agentInvoked":False})]
S["resume"] = [prompt(), ack()] + text_turn("again", "welcome back") + [END(), result()]
os.makedirs(out, exist_ok=True)
for name, steps in S.items():
    with open(os.path.join(out, name + ".ndjson"), "w") as f:
        for step in steps:
            f.write(json.dumps(step, ensure_ascii=False) + "\n")
print(len(S), "scenarios")
