import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { AUTH_USER_HEADER } from "../../src/env";

const user = "review-owner";
const room = (name: string) => env.CHAT_ROOMS.get(env.CHAT_ROOMS.idFromName(`chat2/${name}`));
import { runInDurableObject } from "cloudflare:test";
const enableRelay = (name: string) =>
  runInDurableObject(room(name), instance => {
    const target = instance as unknown as { env: Record<string, unknown> };
    target.env = { ...target.env, CHAT_RELAY_LAYER: "1" };
  });

// postcard VersionVector: count, then (peer varint, zigzag counter varint)
const VV_PEER1_AT_10 = "AQEU"; // {1: 10}
const VV_PEER1_AT_3 = "AQEG"; // {1: 3}

const post = (name: string, query: string, frontier: string, fill: number) =>
  room(name).fetch(`https://chat/checkpoint?${query}`, {
    method: "POST",
    headers: { [AUTH_USER_HEADER]: user, "x-chat2-frontier": frontier },
    body: new Uint8Array(64).fill(fill)
  });

describe("relay checkpoints", () => {
  it("a non-relay checkpoint can't replace a relay-era one with less", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    // A Cloud host's relay-era checkpoint: all the room holds of its relayed ops.
    const cloud = await post(name, "seqCovered=0&atFloor=1", VV_PEER1_AT_10, 9);
    await cloud.arrayBuffer();
    expect(cloud.status).toBe(200);
    // A viewer behind it posts an ordinary checkpoint (durable-rejection / threshold).
    const viewer = await post(name, "seqCovered=0", VV_PEER1_AT_3, 3);
    await viewer.arrayBuffer();
    const held = await room(name).fetch("https://chat/checkpoint", { headers: { [AUTH_USER_HEADER]: user } });
    const body = new Uint8Array(await held.arrayBuffer());
    expect(body[0]).toBe(9);
  });

  it("a stored chat's checkpoints aren't held back with the layer off", async () => {
    const name = crypto.randomUUID();
    const first = await post(name, "seqCovered=0", VV_PEER1_AT_10, 9);
    await first.arrayBuffer();
    const second = await post(name, "seqCovered=0", VV_PEER1_AT_3, 3);
    await second.arrayBuffer();
    expect(second.status).toBe(200);
  });
});
