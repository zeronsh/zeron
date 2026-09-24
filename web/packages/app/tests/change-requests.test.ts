import { describe, expect, it } from "vitest";
import type { ChangeRequestState, ChangeRequestSummary } from "@zeron/proto";
import {
  PROVIDERS,
  badgeModel,
  normalizeProvider,
  toneFor,
} from "../src/lib/change-requests";

function summary(state: ChangeRequestState): ChangeRequestSummary {
  return {
    provider: "github",
    number: 90,
    title: "First line\nSecond line",
    url: "https://github.com/acme/zeron/pull/90",
    state,
    baseRef: "main",
    headRef: "feature/pr",
  };
}

describe("badgeModel", () => {
  it("renders the Open / Merged / Closed labels and tones", () => {
    const cases: Array<[ChangeRequestState, string, "open" | "merged" | "closed"]> = [
      ["open", "Open", "open"],
      ["merged", "Merged", "merged"],
      ["closed", "Closed", "closed"],
    ];
    for (const [state, label, tone] of cases) {
      const model = badgeModel(summary(state));
      expect(model.number).toBe("#90");
      expect(model.stateLabel).toBe(label);
      expect(model.tone).toBe(tone);
      expect(model.title).toBe("First line Second line");
    }
  });

  it("maps state to tone", () => {
    expect(toneFor("open")).toBe("open");
    expect(toneFor("merged")).toBe("merged");
    expect(toneFor("closed")).toBe("closed");
  });
});

// The create-PR compare-URL builder and its tests are gone (ticket 04): the
// desktop has no create flow at all, so guessing a provider's compare URL was
// web-only invention.

describe("normalizeProvider", () => {
  it("knows the provider keys it can normalize", () => {
    expect(PROVIDERS).toEqual(["github", "gitlab", "bitbucket", "azuredevops", "codeberg"]);
  });

  it("lowercases known provider names", () => {
    expect(normalizeProvider("GitHub")).toBe("github");
    expect(normalizeProvider("GITLAB")).toBe("gitlab");
    expect(normalizeProvider("bitbucket")).toBe("bitbucket");
  });

  it("returns null for missing or whitespace-only input", () => {
    expect(normalizeProvider(null)).toBeNull();
    expect(normalizeProvider(undefined)).toBeNull();
    expect(normalizeProvider("")).toBeNull();
    expect(normalizeProvider("   ")).toBeNull();
  });

  it("passes an unknown host through lower-cased rather than guessing a key", () => {
    expect(normalizeProvider("gitlab.example.com")).toBe("gitlab.example.com");
  });
});
