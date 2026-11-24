// What the built pages must hold whatever a redesign does to them: every
// download reachable without script, and the ids telemetry.js and
// downloads.ts look for.
import { readFileSync } from "node:fs";
import { describe, expect, test } from "vitest";
import { ANDROID_APK, APP_STORE, DESKTOP_BUILDS, FALLBACK_VERSION, RELEASES_URL } from "../src/data/releases.ts";

const page = (name: string) => readFileSync(new URL(`../dist/client/${name}`, import.meta.url), "utf8");
const home = page("index.html");

describe("the landing page, built", () => {
  test("links every desktop build at the fallback version without script", () => {
    for (const [id, build] of Object.entries(DESKTOP_BUILDS)) {
      const href = `${RELEASES_URL}zeron-${FALLBACK_VERSION}-${build.file}`;
      expect(home, id).toContain(`href="${href}"`);
      expect(home, id).toContain(`data-build="${id}"`);
    }
  });

  test("links the mobile apps and the downloads table", () => {
    expect(home).toContain(`href="${ANDROID_APK}"`);
    expect(home).toContain(`href="${APP_STORE}"`);
    expect(home).toContain('id="downloads"');
    expect(home).toMatch(/href="\/?#downloads"/);
  });

  test("has the three download buttons telemetry.js counts, pointing at the table", () => {
    for (const id of ["nav-download", "hero-download", "closing-download"]) {
      expect(home, id).toMatch(new RegExp(`id="${id}" href="#downloads"[^>]*data-download=`));
    }
  });

  test("loads the tracker once, deferred, and only on the landing page", () => {
    expect(home.match(/<script defer src="\/telemetry\.js"><\/script>/g)).toHaveLength(1);
    expect(page("privacy.html")).not.toContain("telemetry.js");
  });
});

describe("the other pages, built", () => {
  test("the privacy policy keeps its address and its anchor", () => {
    const privacy = page("privacy.html");
    expect(privacy).toContain("Privacy Policy");
    expect(privacy).toContain('id="delete-account"');
  });

  test("there is a page for addresses that don't exist", () => {
    expect(page("404.html")).toContain("There is no page here");
  });
});
