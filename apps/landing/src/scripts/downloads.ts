// Points the page's download links at the latest release, and its main
// buttons at the visitor's own build. The HTML already links every build at
// the fallback version, so nothing here is needed for a download to work.
//
//   [data-build="<id>"]        a row of the downloads table
//   [data-download]            a main button; "long" takes the build's label
//   [data-release-version]     shows the latest version once it is known

import { DESKTOP_BUILDS, FALLBACK_VERSION, RELEASES_URL, type BuildId } from "../data/releases.ts";
import { acceptVersion, buildUrl, detectTarget, targetUrl } from "../lib/downloads.ts";

const target = detectTarget(navigator);

function apply(version: string): void {
  for (const link of document.querySelectorAll<HTMLAnchorElement>("[data-build]")) {
    const id = link.dataset.build as BuildId;
    if (id in DESKTOP_BUILDS) link.href = buildUrl(version, id);
  }
  const url = targetUrl(target, version);
  if (!url) return;
  const words =
    target.kind === "desktop"
      ? { os: target.build, label: DESKTOP_BUILDS[target.build].label, hint: DESKTOP_BUILDS[target.build].hint }
      : { os: "android", label: "Download for Android", hint: "arm64 APK" };
  for (const link of document.querySelectorAll<HTMLAnchorElement>("[data-download]")) {
    link.href = url;
    link.dataset.os = words.os;
    link.title = words.hint;
    link.setAttribute("aria-label", `${words.label} (${words.hint})`);
    const text = link.querySelector("[data-download-label]");
    if (text && link.dataset.download === "long") text.textContent = words.label;
  }
}

apply(FALLBACK_VERSION);
fetch(`${RELEASES_URL}latest.txt`, { credentials: "omit" })
  .then((response) => (response.ok ? response.text() : Promise.reject()))
  .then((text) => {
    const version = acceptVersion(text);
    if (!version) return;
    apply(version);
    for (const el of document.querySelectorAll<HTMLElement>("[data-release-version]")) {
      el.textContent = version;
      el.hidden = false;
    }
  })
  .catch(() => {});
