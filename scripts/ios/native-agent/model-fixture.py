#!/usr/bin/env python3
"""Deterministic Responses endpoint. No model, credentials or external network.

The first request asks actual Codex to invoke the mobile shell. The follow-up
only succeeds when Codex sends the shell's real output back to the model.
"""
import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(b'{"data":[]}')

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        request = json.loads(body)
        if "VERIFY_NATIVE_SETTINGS" in json.dumps(request.get("input", [])):
            assert request.get("reasoning", {}).get("effort") == "high", request.get("reasoning")
            assert request.get("service_tier") == "flex", request.get("service_tier")
        prompt = json.dumps(request.get("input", [])) + str(request.get("instructions", ""))
        assert "`sandbox_mode` is `workspace-write`" in prompt, "Missing writable workspace policy"
        assert "`sandbox_mode` is `read-only`" not in prompt, "Contradictory read-only policy"
        assert "writable, persistent /workspace" in prompt, "Missing mobile workspace instructions"
        tools = list(request.get("tools", []))
        # Responses Lite carries schemas in developer additional_tools input items.
        for item in request.get("input", []):
            if item.get("type") == "additional_tools":
                tools.extend(item.get("tools", []))
        flat_tools = []
        for tool in tools:
            flat_tools.extend(tool.get("tools", []) if tool.get("type") == "namespace" else [tool])
        names = {tool.get("name") for tool in flat_tools}
        assert {"mobile_shell", "mobile_read_file", "mobile_write_file"} <= names, f"Mobile tools missing: {names}"
        assert not names.intersection({"exec_command", "shell", "shell_command", "apply_patch", "read_file", "exec", "wait", "code_mode"}), names
        results = [item for item in request.get("input", []) if item.get("type") == "function_call_output"]
        events = [{"type": "response.created", "response": {"id": "resp_mobile", "status": "in_progress"}}]
        if not results:
            item = {"type": "function_call", "id": "fc_mobile", "call_id": "call_mobile", "name": "mobile_shell",
                    "arguments": json.dumps({"command": "printf 'Hello from native Codex\\n' > hello.txt; cat hello.txt"})}
        else:
            assert "Hello from native Codex" in json.dumps(results), "Tool output did not return to Codex"
            text = "Edited hello.txt on this iPhone."
            item = {"type": "message", "id": "msg_mobile", "role": "assistant", "status": "completed",
                    "content": [{"type": "output_text", "text": text, "annotations": []}]}
            events.append({"type": "response.output_item.added", "output_index": 0, "item": {**item, "status": "in_progress", "content": []}})
            events.append({"type": "response.output_text.delta", "item_id": "msg_mobile", "output_index": 0, "content_index": 0, "delta": text})
        events.append({"type": "response.output_item.done", "output_index": 0, "item": item})
        events.append({"type": "response.completed", "response": {"id": "resp_mobile", "status": "completed", "output": [item],
            "usage": {"input_tokens": 20, "output_tokens": 10, "total_tokens": 30}}})
        data = "".join("event: " + e["type"] + "\ndata: " + json.dumps(e) + "\n\n" for e in events).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=28761)
    args = parser.parse_args()
    print(f"Mobile Codex fixture: http://127.0.0.1:{args.port}/v1", flush=True)
    ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()
