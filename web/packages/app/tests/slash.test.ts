import { describe, expect, it } from "vitest";
import type { SlashCommand } from "@zeron/proto";
import { parseSlashCommands, refilterSlash, slashDescription, slashErrorMessage, slashToken } from "../src/lib/slash";

/**
 * The slash-command unit tests — `slash_token_only_opens_the_prompt`
 * mirrors composer.rs:8517; the rest pin the local filter (the desktop's
 * `refilter_slash` + `popover::filter_indices` composition), the error
 * copy (`slash_error_message`, :3980), and the row description (:5580).
 */

describe("slash_token_only_opens_the_prompt", () => {
  it("opens on a whole-prompt prefix", () => {
    expect(slashToken("/comp", 5)).toEqual({ start: 0, end: 5, query: "comp" });
  });

  it("the token range spans the whole command word even mid-cursor", () => {
    expect(slashToken("/compact now", 3)).toEqual({ start: 0, end: 8, query: "co" });
  });

  it("not at offset 0, cursor in the argument, or a typed path", () => {
    expect(slashToken("run /compact", 12)).toBeNull();
    expect(slashToken("/goal ship it", 10)).toBeNull();
    expect(slashToken("/usr/bin", 8)).toBeNull();
  });

  it("bare '/' at cursor 0 stays closed; after it opens all", () => {
    expect(slashToken("/", 0)).toBeNull();
    expect(slashToken("/", 1)?.query).toBe("");
  });
});

const commands: SlashCommand[] = [
  { name: "compact", description: "Summarize the conversation", inputHint: null },
  { name: "goal", description: "", inputHint: "text..." },
  { name: "pr-comments", description: "Review pending comments", inputHint: null },
];

describe("refilterSlash", () => {
  it("prefix matches rank before substring matches, ties by input order", () => {
    // "c" is a prefix of "compact" (rank 0) and a substring of
    // "pr-comments" (rank 1).
    const { filtered, active } = refilterSlash("c", commands);
    expect(filtered).toEqual([0, 2]);
    expect(active).toBe(0);
  });

  it("an empty query matches everything at rank 1, input order preserved", () => {
    const { filtered } = refilterSlash("   ", commands);
    expect(filtered).toEqual([0, 1, 2]);
  });

  it("no match leaves the cursor null", () => {
    expect(refilterSlash("zzz", commands)).toEqual({ filtered: [], active: null });
  });
});

describe("slashErrorMessage", () => {
  it("each failure kind has its own verbatim string", () => {
    expect(slashErrorMessage("unknown-method")).toBe(
      "The session's device runs an older zeron — update it to list commands",
    );
    expect(slashErrorMessage("transport")).toBe("The session's device is unreachable");
    expect(slashErrorMessage("closed")).toBe("The session's device is unreachable");
    expect(slashErrorMessage("failed")).toBe("Couldn't load this agent's commands");
    expect(slashErrorMessage("bad-reply")).toBe("Couldn't load this agent's commands");
  });
});

describe("slashDescription", () => {
  it("a hint alone becomes <hint>; with a description it joins with ·", () => {
    expect(slashDescription(commands[0]!)).toBe("Summarize the conversation");
    expect(slashDescription(commands[1]!)).toBe("<text...>");
    expect(
      slashDescription({ name: "x", description: "Do the thing", inputHint: "arg" }),
    ).toBe("Do the thing · <arg>");
  });
});

describe("parseSlashCommands", () => {
  it("decodes the wire list and tolerates malformed payloads", () => {
    expect(parseSlashCommands(commands)).toEqual(commands);
    expect(parseSlashCommands(null)).toBeNull();
    expect(parseSlashCommands(["nope"])).toBeNull();
    expect(parseSlashCommands([{ name: 1, description: "" }])).toBeNull();
  });
});
