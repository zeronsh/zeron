(() => {
  const base = "https://zeron.sh/releases/";
  // Android builds publish on every change under one stable name.
  const android = `${base}zeron-android-arm64.apk`;
  // Desktop downloads are the new app, published on the beta channel. A beta
  // install follows the beta channel, so it keeps itself up to date.
  const desktop = `${base}beta/`;
  const releases = {
    macos: ["macos-arm64.dmg", "Download for macOS", "Apple silicon"],
    windows: ["windows-x86_64-setup.exe", "Download for Windows", "Windows x64 · Installer"],
    "windows-portable": ["windows-x86_64.zip", "Download for Windows", "Windows x64 · Portable ZIP"],
    linux: ["linux-x86_64.tar.gz", "Download for Linux", "Linux x64"],
    "linux-arm": ["linux-aarch64.tar.gz", "Download for Linux", "Linux ARM64"],
  };
  const ua = navigator.userAgent || "";
  const platform = navigator.userAgentData?.platform || navigator.platform || "";
  const mobile = /Android|iPhone|iPad|iPod/i.test(ua) || navigator.userAgentData?.mobile
    || (/Mac/i.test(platform) && navigator.maxTouchPoints > 1);
  const onAndroid = /Android/i.test(ua) || /Android/i.test(platform);
  const os = mobile ? null : /Win/i.test(platform) ? "windows"
    : /Mac/i.test(platform) ? "macos" : /Linux/i.test(platform)
    ? (/aarch64|arm64/i.test(`${platform} ${ua}`) ? "linux-arm" : "linux") : null;
  // On an Android phone the buttons offer the APK (no version to wait for).
  if (onAndroid) {
    for (const id of ["nav-download", "hero-download", "closing-download"]) {
      const link = document.getElementById(id);
      if (!link) continue;
      link.href = android;
      link.textContent = id === "nav-download" ? "Download" : "Download for Android";
      link.setAttribute("data-download-os", "android");
      link.setAttribute("aria-label", "Download for Android (arm64 APK)");
      link.title = "arm64 APK";
    }
  }
  const apply = (version) => {
    for (const link of document.querySelectorAll("[data-platform-download]")) {
      const release = releases[link.dataset.platformDownload];
      if (release) link.href = `${desktop}zeron-${version}-${release[0]}`;
    }
    for (const id of ["nav-download", "hero-download", "closing-download"]) {
      const link = document.getElementById(id);
      if (!link || !os) continue;
      const [file, label, detail] = releases[os];
      link.href = `${desktop}zeron-${version}-${file}`;
      link.textContent = id === "nav-download" ? "Download" : label;
      link.setAttribute("data-download-os", os);
      link.setAttribute("aria-label", `${label} (${detail})`);
      link.title = detail;
    }
  };
  // A published fallback keeps downloads usable without the manifest.
  const fallback = "0.2.139-beta.6";
  const parts = (version) => version.match(/^(\d+)\.(\d+)\.(\d+)-beta\.(\d+)$/)?.slice(1).map(Number);
  apply(fallback);
  fetch(`${desktop}manifest.json`, { credentials: "omit" })
    .then((r) => r.ok ? r.json() : Promise.reject())
    .then((manifest) => {
      const version = typeof manifest?.version === "string" ? manifest.version.trim() : "";
      const a = parts(version), b = parts(fallback);
      if (!a) return;
      const first = a.findIndex((value, i) => value !== b[i]);
      if (first !== -1 && a[first] < b[first]) return;
      apply(version);
    }).catch(() => {});
})();
