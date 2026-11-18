// The store's words on the web are the app's (apps/zgx/ui/locales/en.json,
// `plugins.store.*`), so an author meets one name for one thing.
// APP_EN_JSON points at another copy of en.json (the app lane's branch,
// before the merge brings its `plugins.store` words here).
import { readFileSync } from "node:fs";
import { describe, expect, test } from "vitest";
import { ON_THE_WEB } from "./permissions.ts";
import { AUTHOR_ACTIONS, revokeAsk } from "./actions.ts";
import { versionState } from "./states.ts";
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
  test("every state of a version, by the app's name", () => {
    const names = (["pending", "approved", "rejected", "withdrawn", "yanked", "removed"] as VersionStatus[]).map((status) => versionState({ status }).label);
    expect(names).toEqual(["In review", "Approved", "Not approved", "Withdrawn", "No new installs", "Taken down"]);
  });

  test("an author's actions and their questions", () => {
    expect(AUTHOR_ACTIONS.yank.ask("Focus timer", "1.2.0")).toMatchObject({ title: "Stop new installs of Focus timer 1.2.0?", confirm: "Stop new installs" });
    expect(AUTHOR_ACTIONS.unyank.ask("Focus timer", "1.2.0")).toMatchObject({ title: "Allow installs of Focus timer 1.2.0?", confirm: "Allow installs again" });
    expect(AUTHOR_ACTIONS.withdraw.ask("Focus timer", "1.2.0")).toMatchObject({ title: "Withdraw Focus timer 1.2.0?", confirm: "Withdraw" });
    expect(revokeAsk("GitHub Actions")).toMatchObject({ title: "Revoke GitHub Actions?", confirm: "Revoke" });
  });
});

describe("the app's plural, as its en.json writes it", () => {
  test("one, and other with its count", () => {
    expect(store("plugins.store.key.ends_in", { n: 1 })).toBe("Ends in 1 day");
    expect(store("plugins.store.key.ends_in", { n: 11 })).toBe("Ends in 11 days");
  });
});

