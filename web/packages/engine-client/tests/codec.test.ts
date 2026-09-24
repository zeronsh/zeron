import { describe, expect, test } from "vitest";
import type { Chat, EngineInfo } from "@zeron/proto";
import {
  decodeServerMessage,
  encodeAuthEnvelope,
  encodeClientFrame,
  isStreamAck,
} from "../src/codec";
import { recorded } from "./fixtures/recorded";

describe("client frame encoding", () => {
  test("serializes invoke frames in the Rust serde key order", () => {
    expect(encodeClientFrame({ id: 1, method: "EngineInfo", params: {} })).toBe(
      '{"id":1,"method":"EngineInfo","params":{}}',
    );
    expect(encodeClientFrame({ id: 2, method: "WatchChats", params: null })).toBe(
      '{"id":2,"method":"WatchChats"}',
    );
  });

  test("serializes cancel frames", () => {
    expect(encodeClientFrame({ id: 7, cancel: true })).toBe('{"id":7,"cancel":true}');
  });

  test("the auth envelope travels alone", () => {
    expect(encodeAuthEnvelope("a-secret")).toBe('{"auth":"a-secret"}');
  });

  test("encoded frames parse back to the input frame", () => {
    const frame = { id: 9, method: "WatchQueue", params: { chatId: "chat-1" } };
    expect(JSON.parse(encodeClientFrame(frame))).toEqual(frame);
  });

  test("reproduces the client frames recorded against a real engine byte for byte", () => {
    expect(encodeAuthEnvelope(recorded.authCredential)).toBe(recorded.clientFrames.auth);
    expect(encodeClientFrame({ id: 1, method: "EngineInfo", params: {} })).toBe(
      recorded.clientFrames.engineInfoInvoke,
    );
    expect(encodeClientFrame({ id: 2, method: "WatchChats", params: {} })).toBe(
      recorded.clientFrames.watchChatsInvoke,
    );
    expect(encodeClientFrame({ id: 2, cancel: true })).toBe(recorded.clientFrames.watchChatsCancel);
  });
});

describe("server frame decoding", () => {
  test("decodes each frame shape", () => {
    expect(decodeServerMessage('{"id":1,"ok":{"a":1}}').frames).toEqual([
      { id: 1, ok: { a: 1 } },
    ]);
    expect(decodeServerMessage('{"id":2,"err":"boom"}').frames).toEqual([{ id: 2, err: "boom" }]);
    expect(decodeServerMessage('{"id":3,"item":[1,2]}').frames).toEqual([{ id: 3, item: [1, 2] }]);
    expect(decodeServerMessage('{"id":4,"done":true}').frames).toEqual([{ id: 4, done: true }]);
    expect(decodeServerMessage('{"id":5}').frames).toEqual([{ id: 5 }]);
  });

  test("splits multi-line batches and skips blank lines", () => {
    const message = '{"id":1,"ok":1}\n\n{"id":2,"item":[]}\n{"id":3,"done":true}\n';
    expect(decodeServerMessage(message).frames).toEqual([
      { id: 1, ok: 1 },
      { id: 2, item: [] },
      { id: 3, done: true },
    ]);
    expect(decodeServerMessage(message).malformed).toBe(0);
  });

  test("drops garbage and wrongly-shaped lines, keeps the rest of the batch", () => {
    const message = [
      "not json",
      '{"id":1,"ok":true}',
      "[]",
      "123",
      '"a string"',
      '{"ok":true}',
      '{"id":"x","ok":true}',
      '{"id":1.5,"ok":true}',
      '{"id":-1,"ok":true}',
      '{"id":2,"err":5}',
      '{"id":3,"done":"yes"}',
      '{"id":4,"item":null}',
    ].join("\n");
    const decoded = decodeServerMessage(message);
    expect(decoded.malformed).toBe(10);
    expect(decoded.frames).toEqual([{ id: 1, ok: true }, { id: 4, item: null }]);
  });

  test("a whole garbage message is malformed but not fatal", () => {
    expect(decodeServerMessage("}not json{").frames).toEqual([]);
    expect(decodeServerMessage("}not json{").malformed).toBe(1);
  });
});

describe("the stream special-case", () => {
  test("recognizes the readiness ack", () => {
    expect(isStreamAck({ id: 1, ok: { stream: true } })).toBe(true);
    expect(isStreamAck({ id: 1, ok: { value: 2 } })).toBe(false);
    expect(isStreamAck({ id: 1, ok: {} })).toBe(false);
    expect(isStreamAck({ id: 1, ok: "stream" })).toBe(false);
    expect(isStreamAck({ id: 1, ok: null })).toBe(false);
    expect(isStreamAck({ id: 1 })).toBe(false);
    expect(isStreamAck({ id: 1, item: { stream: true } })).toBe(false);
  });
});

describe("recorded engine frames", () => {
  test("the EngineInfo reply satisfies the generated type", () => {
    const frames = decodeServerMessage(recorded.serverMessages.engineInfo).frames;
    expect(frames).toHaveLength(1);
    const info = frames[0]!.ok as EngineInfo;
    expect(info.deviceId).toBe(recorded.deviceId);
    expect(["local", "synced", "development"]).toContain(info.workspaceScope);
    expect(info.capabilities).toContain("web-client");
  });

  test("the WatchChats item is a Chat array with the generated fields", () => {
    const frames = decodeServerMessage(recorded.serverMessages.watchChatsItem).frames;
    expect(frames).toHaveLength(1);
    const chats = frames[0]!.item as Chat[];
    for (const chat of chats) {
      expect(typeof chat.id).toBe("string");
      expect(typeof chat.deviceId).toBe("string");
      expect(typeof chat.archived).toBe("boolean");
      expect(typeof chat.createdAt).toBe("string");
      expect(chat.title === null || typeof chat.title === "string").toBe(true);
      expect(chat.cwd === null || typeof chat.cwd === "string").toBe(true);
    }
  });
});
