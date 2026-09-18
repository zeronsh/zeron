(() => {
  const base = "https://zeron.sh/releases/";
  const releases = {
    macos: ["macos-arm64.dmg", "Download for macOS", "Apple silicon"],
    windows: ["windows-x86_64.zip", "Download for Windows", "Windows x64 · Portable ZIP"],
    linux: ["linux-x86_64.tar.gz", "Download for Linux", "Linux x64"],
    "linux-arm": ["linux-aarch64.tar.gz", "Download for Linux", "Linux ARM64"],
  };
  const ua = navigator.userAgent || "";
  const platform = navigator.userAgentData?.platform || navigator.platform || "";
  const mobile = /Android|iPhone|iPad|iPod/i.test(ua) || navigator.userAgentData?.mobile
    || (/Mac/i.test(platform) && navigator.maxTouchPoints > 1);
  const os = mobile ? null : /Win/i.test(platform) ? "windows"
    : /Mac/i.test(platform) ? "macos" : /Linux/i.test(platform)
    ? (/aarch64|arm64/i.test(`${platform} ${ua}`) ? "linux-arm" : "linux") : null;
  const apply = (version) => {
    for (const link of document.querySelectorAll("[data-platform-download]")) {
      const release = releases[link.dataset.platformDownload];
      if (release) link.href = `${base}zeron-${version}-${release[0]}`;
    }
    for (const id of ["nav-download", "hero-download", "closing-download"]) {
      const link = document.getElementById(id);
      if (!link || !os) continue;
      const [file, label, detail] = releases[os];
      link.href = `${base}zeron-${version}-${file}`;
      link.textContent = id === "nav-download" ? "Download" : label;
      link.setAttribute("aria-label", `${label} (${detail})`);
      link.title = detail;
    }
    document.getElementById("ver").textContent = `v${version}`;
  };
  // A published fallback keeps downloads usable without the version endpoint.
  const fallback = "0.2.66";
  apply(fallback);
  fetch(`${base}latest.txt`, { credentials: "omit" })
    .then((r) => r.ok ? r.text() : Promise.reject())
    .then((text) => {
      const version = text.trim();
      if (!/^\d+\.\d+\.\d+$/.test(version)) return;
      const a = version.split(".").map(Number), b = fallback.split(".").map(Number);
      const first = a.findIndex((value, i) => value !== b[i]);
      if (first !== -1 && a[first] < b[first]) return;
      apply(version);
    }).catch(() => {});
})();
