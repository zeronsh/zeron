#!/usr/bin/env node
// One IPC call to a rig engine, its reply printed as JSON.
//
//   node ipc.mjs <port> <method> [jsonParams] [--stream N] [--data <dir>] [--timeout ms]
//
//   node ipc.mjs 27891 EngineInfo
//   node ipc.mjs 27891 WatchDevices --stream 1
//   node ipc.mjs 27892 PluginSend '{"plugin":"ping","to":"*","data":{}}'
//
// The port picks the rig engine (A1 27891, A2 27892, B 27893) and so its
// data dir's ipc-token; --data names another engine's data dir. Stream
// methods print one JSON line per item and exit after N (or when the stream
// ends). Never the production engine (27654).

import { Ipc, engineByPort } from "./lib.mjs";

const args = process.argv.slice(2);
const flag = (name) => {
  const i = args.indexOf(name);
  if (i < 0) return undefined;
  const v = args[i + 1];
  args.splice(i, 2);
  return v;
};
const stream = flag("--stream");
const dataFlag = flag("--data");
const timeout = Number(flag("--timeout") ?? 30000);
const [port, method, raw] = args;

if (!port || !method) {
  console.error("usage: node ipc.mjs <port> <method> [jsonParams] [--stream N] [--data <dir>] [--timeout ms]");
  process.exit(2);
}
const data = dataFlag ?? engineByPort(port)?.data;
if (!data) {
  console.error(`port ${port} isn't a rig engine's; pass --data <its data dir>`);
  process.exit(2);
}
let params = {};
if (raw) {
  try {
    params = JSON.parse(raw);
  } catch (e) {
    console.error(`params aren't JSON: ${e.message}`);
    process.exit(2);
  }
}

let ipc;
try {
  ipc = await Ipc.open(port, data);
  if (stream !== undefined) {
    const n = Number(stream);
    let seen = 0;
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`${method}: ${seen}/${n} items in ${timeout} ms`)), timeout);
      const cancel = ipc.stream(
        method,
        params,
        (item) => {
          seen++;
          console.log(JSON.stringify(item));
          if (seen >= n) {
            clearTimeout(timer);
            cancel();
            resolve();
          }
        },
        (err) => {
          clearTimeout(timer);
          err ? reject(err) : resolve();
        },
      );
    });
  } else {
    const ok = await ipc.call(method, params, timeout);
    console.log(JSON.stringify(ok, null, 2));
  }
  ipc.close();
  process.exit(0);
} catch (e) {
  ipc?.close();
  console.error(`error: ${e.message}`);
  process.exit(1);
}
