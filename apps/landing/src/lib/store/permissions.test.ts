// The web says what a plugin asks for in the app's words: every string here
// is held to the app's English (apps/zgx/ui/locales/en.json), and the
// wording follows the consent sheet's rules (words.luau).
import { readFileSync } from "node:fs";
import { describe, expect, test } from "vitest";
import { details, grew, lines, ON_THE_WEB, permission, place, places, WORDS } from "./permissions.ts";
import type { Permissions } from "./types.ts";

const en = JSON.parse(readFileSync(new URL("../../../../zgx/ui/locales/en.json", import.meta.url), "utf8")) as Record<string, unknown>;
const lookup = (key: string): unknown => key.split(".").reduce<unknown>((node, part) => (node && typeof node === "object" ? (node as Record<string, unknown>)[part] : undefined), en);

describe("the words are the app's", () => {
  test.each(Object.keys(WORDS))("%s", (key) => {
    const app = lookup(key);
    expect(typeof app, `${key} is in en.json`).toBe("string");
    const onTheWeb = ON_THE_WEB.reduce((text, [from, to]) => text.replaceAll(from, to), app as string);
    expect(WORDS[key as keyof typeof WORDS]).toBe(onTheWeb);
  });
});

const p = (more: Partial<Permissions> = {}): Permissions => ({ asks: [], ...more });

describe("each ask's line", () => {
  test("calm asks first, risky ones last, in the order asked", () => {
    const out = lines(p({ asks: ["commands", "ui.add", "native", "sessions.read"], commands: ["gh"] }));
    expect(out.map((l) => l.risky)).toEqual([false, false, true, true]);
    expect(out[0]!.title).toBe("Add things to Zeron's interface");
    expect(out[2]!.icon).toBe("terminal");
  });

  test("programs: one that is full control says so, one that can do anything says so, a contained one by name", () => {
    const shell = permission(p({ asks: ["commands"], commands: ["bash", "git", "jq"], commands_anything: ["bash", "git"], commands_full_control: ["bash"] }), "commands");
    expect(shell.title).toBe("Run bash, git, jq on your computer as you");
    expect(shell.risky).toBe(true);
    expect(shell.detail).toBe("They run as you, with your sign-ins.");
    expect(shell.literal).toEqual(["It can run bash, which gives it full control of your computer.", "git: can do anything you can", "jq"]);

    const contained = permission(p({ asks: ["commands"], commands: ["jq"], commands_anything: [], commands_full_control: [] }), "commands");
    expect(contained.detail).toBe("It can do whatever jq can do with your sign-ins.");
    expect(contained.literal).toBeUndefined();

    const anything = permission(p({ asks: ["commands"], commands: ["gh"], commands_anything: ["gh"], commands_full_control: [] }), "commands");
    expect(anything.detail).toBe("These can do anything you can, with your sign-ins.");
  });

  test("programs without the host's rating are each said to do anything you can", () => {
    const line = permission(p({ asks: ["commands"], commands: ["make"] }), "commands");
    expect(line.literal).toEqual(["make: can do anything you can"]);
  });

  test("hosts: any site, named ones, and ones on your network (risky then)", () => {
    const any = permission(p({ asks: ["network"], network_hosts: ["*"] }), "network");
    expect(any.title).toBe("Connect to any site on the internet");
    expect(any.literal).toEqual(["Any site on the internet"]);
    expect(any.risky).toBe(false);

    const near = permission(p({ asks: ["network"], network_hosts: ["api.example.com", "localhost:8080"], network_local: ["localhost:8080"], network_named: ["api.example.com"] }), "network");
    expect(near.title).toBe("Connect to these places");
    expect(near.risky).toBe(true);
    expect(near.detail).toBe("A named host can be on your computer or your network.");
    expect(near.literal).toEqual(["api.example.com", "localhost:8080: your computer or your network"]);

    expect(permission(p({ asks: ["network"] }), "network").title).toBe("Connect to the internet");
  });

  test("tools are named with what they tell agents", () => {
    const line = permission(p({ asks: ["tools"], tools: [{ name: "pg_query", description: "Runs a query." }, { name: "bare" }] }), "tools");
    expect(line.items).toEqual([{ name: "pg_query", description: "Runs a query." }, { name: "bare" }]);
    expect(permission(p({ asks: ["tools"] }), "tools").detail).toBe("none listed");
  });

  test("changing and replacing parts of Zeron name the places", () => {
    expect(permission(p({ asks: ["ui.change"], ui: { change: [".session-row >> .session-title", "#titlebar"] } }), "ui.change").title).toBe("Change how a sidebar row and the title bar shows");
    const replace = permission(p({ asks: ["ui.replace"], ui: { replace: ["@app/screens/chat/composer"] } }), "ui.replace");
    expect(replace.title).toBe("Replace the message box with its own");
    expect(replace.risky).toBe(true);
  });

  test("a permission this site doesn't know is named as written, and risky", () => {
    expect(permission(p(), "time.travel")).toEqual({ title: "time.travel", risky: true, icon: "danger-triangle" });
  });

  test("details list what it changes and replaces, as written, at most forty", () => {
    const change = Array.from({ length: 45 }, (_, i) => `.thing-${i}`);
    const out = details(p({ ui: { change, replace: ["@app/screens/files"] } }));
    expect(out[0]!.title).toBe("Parts of Zeron it changes");
    expect(out[0]!.list).toHaveLength(41);
    expect(out[0]!.list.at(-1)).toBe("and 5 more");
    expect(out[1]).toEqual({ title: "Screens it replaces", list: ["@app/screens/files"] });
  });
});

describe("places", () => {
  test.each([
    ["#sidebar", "the sidebar"],
    [".message.message-user", "a message"],
    ["#a >> .file-row", "a file's row"],
    ["@app/screens/chat/home", "the new chat page"],
    ["@app/screens/settings/whatever", "Settings"],
    ["constructor", "part of Zeron"],
    ["__proto__", "part of Zeron"],
    [".unknown", "part of Zeron"],
  ])("%s is %s", (target, said) => expect(place(target)).toBe(said));

  test("each place once, joined as a person would", () => {
    expect(places([])).toBe("parts of Zeron");
    expect(places(["#sidebar", "#sidebar-actions", "#titlebar", "#panel"])).toBe("the sidebar, the title bar and the side panel");
  });
});

describe("what grew since the last approved version", () => {
  test("new asks, then what grew inside the ones it had, risky last", () => {
    const now = p({ asks: ["ui.add", "network", "commands"], commands: ["gh", "bash"], commands_anything: ["gh", "bash"], commands_full_control: ["bash"], network_hosts: ["a.example", "b.example"] });
    const out = grew(now, { asks_added: ["ui.add"], added_commands: ["bash"], network_added: ["b.example"] });
    expect(out.map((l) => l.title)).toEqual(["Add things to Zeron's interface", "Connect to more places", "Run more programs as you"]);
    expect(out[2]!.literal).toEqual(["It can run bash, which gives it full control of your computer."]);
  });

  test("nothing grew, nothing said", () => {
    expect(grew(p(), null)).toEqual([]);
    expect(grew(p(), {})).toEqual([]);
  });
});
