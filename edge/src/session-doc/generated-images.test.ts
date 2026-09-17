import { describe, expect, it } from "vitest";
import { type DocMessagePart, fromDocParts, toDocParts, splitMessageEntry, joinContinuations } from "./messages";
import { toRenderParts } from "./render-parts";

describe("generated image parts", () => {
  const image = { kind: "image" as const, id: "i:image", path: "/uploads/generated.png", name: "generated.png", mimeType: "image/png" };
  it("preserves only the structured reference through render/doc conversion", () => {
    expect(fromDocParts(toDocParts(toRenderParts([image])))).toEqual([image]);
  });
  it("keeps images atomic through split/join", () => {
    const entry = { id: "a", role: "assistant" as const, deviceId: "owner", createdAt: 1, parts: [image, { ...image, id: "j:image" }] };
    const split = splitMessageEntry(entry, 160);
    expect(split).toHaveLength(2);
    expect(joinContinuations(split)).toEqual([entry]);
  });
  it("degrades malformed references without rendering paths as text", () => {
    for (const part of [{ kind: "image" as const, id: "i" }, { ...image, mimeType: "image/svg+xml" }, { ...image, path: 42 } as unknown as DocMessagePart]) {
      expect(fromDocParts([part])).toEqual([{ kind: "error", id: part.id, message: "Generated image unavailable" }]);
    }
  });
});
