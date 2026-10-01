import assert from "node:assert/strict";
import test from "node:test";
import { createRuntime } from "./runtime.mjs";

test("native file tools and shell share edits, pipes and persisted binary/directory state", async () => {
  const runtime = await createRuntime();
  await runtime.dispatch({ method: "write", path: "/workspace/hello.txt", content: "hello\nworld\n" });
  assert.deepEqual(await runtime.dispatch({ method: "exec", command: "cat hello.txt | grep hello; sed -i 's/world/mobile/' hello.txt; mkdir empty; printf '\\000\\377' > binary" }),
    { stdout: "hello\n", stderr: "", exitCode: 0 });
  assert.equal(await runtime.dispatch({ method: "read", path: "/workspace/hello.txt" }), "hello\nmobile\n");
  const checkpoint = await runtime.dispatch({ method: "snapshot" });
  const restored = await createRuntime(checkpoint);
  assert.deepEqual(await restored.dispatch({ method: "snapshot" }), checkpoint);
  assert.equal(await restored.dispatch({ method: "read", path: "/workspace/hello.txt" }), "hello\nmobile\n");
});

test("reports unavailable tools and bounds runaway shell loops", async () => {
  const runtime = await createRuntime();
  const missing = await runtime.dispatch({ method: "exec", command: "node --version" });
  assert.equal(missing.exitCode, 127);
  const loop = await runtime.dispatch({ method: "exec", command: "while true; do :; done" });
  assert.notEqual(loop.exitCode, 0);
});
