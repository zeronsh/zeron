import { spawn } from "node:child_process";

const child = spawn(process.execPath, ["node_modules/vitest/vitest.mjs", "run", "-c", "vitest.workerd.config.ts", ...process.argv.slice(2)], {
  cwd: new URL("..", import.meta.url),
  stdio: ["inherit", "pipe", "pipe"]
});


let forwardedSignal;
const forwardSignal = (signal) => {
  if (forwardedSignal !== undefined) return;
  forwardedSignal = signal;
  child.kill(signal);
};
const signalHandlers = new Map(
  ["SIGINT", "SIGTERM"].map((signal) => [signal, () => forwardSignal(signal)])
);
for (const [signal, handler] of signalHandlers) process.on(signal, handler);

let output = "";
for (const stream of [child.stdout, child.stderr]) {
  stream.on("data", (chunk) => {
    output += chunk;
    (stream === child.stdout ? process.stdout : process.stderr).write(chunk);
  });
}

const code = await new Promise((resolve, reject) => {
  child.on("error", reject);
  child.on("close", resolve);
});


if (forwardedSignal !== undefined) {
  for (const [signal, handler] of signalHandlers) process.removeListener(signal, handler);
  process.kill(process.pid, forwardedSignal);
}

if (/uncaught exception|unhandled rejection|Expected global Vitest state/i.test(output)) {
  console.error("Workerd emitted an uncaught runner error despite Vitest's exit status.");
  process.exitCode = 1;
} else {
  process.exitCode = code ?? 1;
}
