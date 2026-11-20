// Where Zeron is downloaded from. One list feeds the downloads table, the
// download buttons and the script that points them at the latest release.

export const RELEASES_URL = "https://zeron.sh/releases/";
export const RELEASE_NOTES_URL = "https://github.com/zeronsh/zeron/releases/latest";
export const INSTALL_COMMAND = "curl -fsSL https://zeron.sh/install.sh | sh";

/** Links use this version until `latest.txt` answers, and when it can't.
 *  0.2.97 is the first release with the Windows installer. */
export const FALLBACK_VERSION = "0.2.97";

export type Platform = "macos" | "windows" | "linux" | "ios" | "android";

export interface DesktopBuild {
  platform: Platform;
  name: string;
  /** What the downloads table says beside the name. */
  detail: string;
  /** The file name after `zeron-<version>-`. */
  file: string;
  /** The main button's words when this build is the visitor's. */
  label: string;
  /** Its tooltip and accessible detail. */
  hint: string;
}

export const DESKTOP_BUILDS = {
  macos: { platform: "macos", name: "macOS", detail: "Apple silicon · dmg", file: "macos-arm64.dmg", label: "Download for macOS", hint: "Apple silicon" },
  windows: { platform: "windows", name: "Windows", detail: "x64 · installer", file: "windows-x86_64-setup.exe", label: "Download for Windows", hint: "Windows x64 · Installer" },
  "windows-portable": { platform: "windows", name: "Windows", detail: "x64 · portable zip", file: "windows-x86_64.zip", label: "Download for Windows", hint: "Windows x64 · Portable ZIP" },
  linux: { platform: "linux", name: "Linux", detail: "x64 · tar.gz", file: "linux-x86_64.tar.gz", label: "Download for Linux", hint: "Linux x64" },
  "linux-arm": { platform: "linux", name: "Linux", detail: "arm64 · tar.gz", file: "linux-aarch64.tar.gz", label: "Download for Linux", hint: "Linux ARM64" },
} as const satisfies Record<string, DesktopBuild>;

export type BuildId = keyof typeof DESKTOP_BUILDS;

/** Android builds publish on every change under one stable name. */
export const ANDROID_APK = `${RELEASES_URL}zeron-android-arm64.apk`;
export const APP_STORE = "https://apps.apple.com/us/app/zeron-coding-agents/id6803187897";
