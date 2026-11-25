// An author's running Zeron, called over its own IPC as its app does.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { whyNotItsSession } from "./where.mjs";

/** `method` of author `a`'s Zeron (an accounts file's entry). */
export const call = (a, method, params = {}) =>
  new Promise((res, rej) => {
    const token = readFileSync(join(a.data_dir, "ipc-token"), "utf8").trim();
    const ws = new WebSocket(`ws://127.0.0.1:${a.ipc_port}`, { headers: { authorization: `Bearer ${token}` } });
    ws.onerror = () => rej(new Error(`nothing answers on its IPC port ${a.ipc_port}`));
    ws.onclose = () => rej(new Error("it closed the call"));
    ws.onopen = () => ws.send(JSON.stringify({ id: 1, method, params }));
    ws.onmessage = ({ data }) => {
      const frame = JSON.parse(String(data));
      if (frame.id !== 1) return;
      frame.err !== undefined ? rej(new Error(typeof frame.err === "string" ? frame.err : JSON.stringify(frame.err))) : res(frame.ok);
      ws.close();
    };
  });

/** Why author `a`'s Zeron may not publish for an accounts file naming
 * `clientId`: what it publishes goes to its own edge, whatever the seed was
 * told, so it must be signed in to that edge's WorkOS. */
export const whyNotTheirs = async (a, clientId) => {
  try {
    return whyNotItsSession((await call(a, "AccessToken"))?.token, clientId);
  } catch (error) {
    return error.message;
  }
};
