import { createHash } from "node:crypto";
import { expect, test } from "vitest";
import { contentSecurityPolicy, hasInlineScript, inlineStyles } from "./security.ts";

test("a page's policy allows exactly its own inline styles", async () => {
  const html = `<html><head><style>a{color:red}</style><style data-x>b{}</style></head><body style="x"></body></html>`;
  expect(inlineStyles(html)).toEqual(["a{color:red}", "b{}"]);
  const csp = await contentSecurityPolicy(html);
  for (const style of inlineStyles(html)) expect(csp).toContain(`'sha256-${createHash("sha256").update(style).digest("base64")}'`);
  expect(csp).toContain("script-src 'self';");
  expect(csp).not.toContain("unsafe-inline");
  expect(csp).toContain("frame-ancestors 'none'");
  expect(csp).toContain("default-src 'none'");
});

test("author text in an attribute is never taken for a style or a script", () => {
  const html = `<meta name="description" content="Probe <style> summary <script>x</script>"><style>real{}</style><p title='<style>no{}</style>'>t</p><!-- <style>gone{}</style> -->`;
  expect(inlineStyles(html)).toEqual(["real{}"]);
  expect(hasInlineScript(html)).toBe(false);
});

test("inline scripts are found; script files and data blocks aren't scripts", () => {
  expect(hasInlineScript('<script type="module" src="/_astro/a.js"></script>')).toBe(false);
  expect(hasInlineScript('<script type="application/ld+json">{"a":1}</script>')).toBe(false);
  expect(hasInlineScript("<script>alert(1)</script>")).toBe(true);
  expect(hasInlineScript('<script type="module">import "x"</script>')).toBe(true);
});
