// The store's words on the web are the app's (apps/zgx/ui/locales/en.json,
// `plugins.store.*`), so an author meets one name for one thing.
// APP_EN_JSON points at another copy of en.json (the app lane's branch,
// before the merge brings its `plugins.store` words here).
import { readFileSync } from "node:fs";
import { describe, expect, test } from "vitest";
import { ON_THE_WEB } from "./permissions.ts";
import { AUTHOR_ACTIONS, revokeAsk } from "./actions.ts";
import { verificationState, versionState } from "./states.ts";
import { store, STORE_WORDS } from "./words.ts";
import type { VersionStatus } from "./types.ts";

const source = process.env.APP_EN_JSON ?? new URL("../../../../zgx/ui/locales/en.json", import.meta.url);
const en = JSON.parse(readFileSync(source, "utf8")) as Record<string, unknown>;
const lookup = (key: string): unknown => key.split(".").reduce<unknown>((node, part) => (node && typeof node === "object" ? (node as Record<string, unknown>)[part] : undefined), en);
const appHasThem = typeof lookup("plugins.store") === "object";

describe.skipIf(!appHasThem)("the store's words are the app's", () => {
  test.each(Object.keys(STORE_WORDS))("%s", (key) => {
    const app = lookup(key);
    expect(typeof app, `${key} is in en.json`).toBe("string");
    const onTheWeb = ON_THE_WEB.reduce((text, [from, to]) => text.replaceAll(from, to), app as string);
    expect(STORE_WORDS[key as keyof typeof STORE_WORDS]).toBe(onTheWeb);
  });
});

describe("the site says them where they belong", () => {
  test("every state of a version (amendment 45), by the app's name where it has one", () => {
    const names = (["listed", "yanked", "taken_down", "removed"] as VersionStatus[]).map((status) => versionState({ status }).label);
    expect(names).toEqual(["Listed", "No new installs", "Taken down", "Removed"]);
  });

  test("whether it's verified: the public's two words, and a decline the author's and staff's alone", () => {
    // (The contract's three states: a decline reads "Not verified" to its author only; the public reads "Not verified yet".)
    expect(verificationState({ verified: true }).label).toBe("Verified");
    expect(verificationState({ verified: false }).label).toBe("Not verified yet");
    expect(verificationState({ verified: false, review: { decided: false, decision: null } }).label).toBe("Not verified yet");
    expect(verificationState({ verified: false, review: { decided: true, decision: "declined", reason: "broken" } })).toMatchObject({ label: "Not verified", tone: "warning" });
  });

  test("an author's actions and their questions", () => {
    expect(AUTHOR_ACTIONS.yank.ask("Focus timer", "1.2.0")).toMatchObject({ title: "Stop new installs of Focus timer 1.2.0?", confirm: "Stop new installs" });
    expect(AUTHOR_ACTIONS.unyank.ask("Focus timer", "1.2.0")).toMatchObject({ title: "Allow installs of Focus timer 1.2.0?", confirm: "Allow installs again" });
    expect(Object.keys(AUTHOR_ACTIONS)).toEqual(["yank", "unyank"]);
    expect(revokeAsk("GitHub Actions")).toMatchObject({ title: "Revoke GitHub Actions?", confirm: "Revoke" });
  });
});

describe("the app's plural, as its en.json writes it", () => {
  test("one, and other with its count", () => {
    expect(store("plugins.store.key.ends_in", { n: 1 })).toBe("Ends in 1 day");
    expect(store("plugins.store.key.ends_in", { n: 11 })).toBe("Ends in 11 days");
  });
});

