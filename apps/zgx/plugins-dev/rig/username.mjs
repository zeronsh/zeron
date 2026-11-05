#!/usr/bin/env node
// Claims a username for a rig engine's account on the staging edge (the
// consent sheet names a plugin's author by it), unless it has one.
//
//   node username.mjs <port> <username>
//
// The engine's bearer (IPC AccessToken) is used once and never printed.

import { Ipc, engineByPort } from "./lib.mjs";

const [port, wanted] = process.argv.slice(2);
const engine = engineByPort(port);
if (!engine || !wanted) {
  console.error("usage: node username.mjs <rig port> <username>");
  process.exit(2);
}
const ipc = await Ipc.open(Number(port), engine.data);
const { token } = await ipc.call("AccessToken", {});
ipc.close();
const edge = "https://staging.zeron.sh";
const headers = { authorization: `Bearer ${token}`, "content-type": "application/json" };
const now = await (await fetch(`${edge}/username`, { headers })).json();
if (now.username) {
  console.log(`already @${now.username}`);
  process.exit(0);
}
const res = await fetch(`${edge}/username`, { method: "POST", headers, body: JSON.stringify({ username: wanted }) });
console.log(res.status, JSON.stringify(await res.json()));
