import { describe, expect, it } from "vitest";
import {
  fileIconAssetPath,
  hasSpecificFileIcon,
  resolveDirectoryIcon,
  resolveFileIcon,
  wellBg,
} from "../src/lib/file-icons";

/**
 * Ports of `crates/ui/src/file_icons.rs`'s tests — the resolution order,
 * the compound-extension longest-match rule, the two manifest aliases, the
 * dark-mode path split, and `has_specific_file_icon`'s stricter check.
 */

describe("fileIconAssetPath (root-absolute URLs)", () => {
  it("resolved paths are root-absolute — no document-relative re-reading", () => {
    const names = ["main.ts", "lib.rs", "component.test.tsx", "Makefile", "unknown.unrecognized"];
    for (const name of names) {
      for (const appearance of ["light", "dark"] as const) {
        const path = resolveFileIcon(name, appearance);
        expect(path.startsWith("/file-icons/")).toBe(true);
        // The engine's static bundle only answers exact root paths; a
        // document-relative reference would re-base onto `/chat/` and 404.
        expect(new URL(path, "https://host/chat/abc").href).toBe(new URL(path, "https://host/").href);
      }
    }
    const folder = resolveDirectoryIcon("src", "light");
    expect(folder.startsWith("/file-icons/")).toBe(true);
    expect(new URL(folder, "https://host/chat/abc").href).toBe(new URL(folder, "https://host/").href);
  });
});

describe("resolveDirectoryIcon (file_icons.rs)", () => {
  it("folders_resolve_name_and_expansion_state", () => {
    // The expansion state is threaded for the caller's chevron glyph; the
    // folder image is identical either way.
    expect(resolveDirectoryIcon("src", "light", false)).toBe("/file-icons/folders/folder-orange-code.svg");
    expect(resolveDirectoryIcon("src", "light", true)).toBe("/file-icons/folders/folder-orange-code.svg");
    expect(resolveDirectoryIcon("src", "light")).toBe("/file-icons/folders/folder-orange-code.svg");
    expect(resolveDirectoryIcon("unknown", "light", true)).toBe("/file-icons/folders/folder.svg");
    expect(resolveDirectoryIcon("assets", "light")).toBe("/file-icons/folders/folder-assets.svg");
  });

  it("directory names are matched case-insensitively", () => {
    expect(resolveDirectoryIcon("SRC", "light")).toBe("/file-icons/folders/folder-orange-code.svg");
  });
});

describe("resolveFileIcon (file_icons.rs)", () => {
  it("exact_names_are_case_insensitive_and_ignore_parent_dots", () => {
    expect(resolveFileIcon("C:\\project\\PACKAGE.JSON", "light")).toBe("/file-icons/files/node.svg");
    expect(resolveFileIcon("folder.css/unknown", "light")).toBe("/file-icons/files/document.svg");
    expect(resolveFileIcon("config/.env.local", "light")).toBe("/file-icons/files/gear.svg");
    expect(resolveFileIcon("docs/claude.md", "light")).toBe("/file-icons/files/claude.svg");
  });

  it("compound_extensions_win_longest_first", () => {
    expect(resolveFileIcon("component.test.tsx", "light")).toBe("/file-icons/files/react-test.svg");
    expect(resolveFileIcon("model.schema.json", "light")).toBe("/file-icons/files/brackets-yellow.svg");
    expect(resolveFileIcon("model.freezed.dart", "light")).toBe("/file-icons/files/dart.svg");
  });

  it("resolves the manifest's two missing-definition aliases", () => {
    expect(resolveFileIcon("styles.less", "light")).toBe("/file-icons/files/brackets-sky.svg");
    expect(resolveFileIcon("stack.yml", "light")).toBe("/file-icons/files/yaml.svg");
  });

  it("appearance_independence_and_hints_are_supported", () => {
    expect(resolveFileIcon("bun.lock", "light")).toBe("/file-icons/files/bun.svg");
    expect(resolveFileIcon("untitled", "light", { language: "Rust" })).toBe("/file-icons/files/rust.svg");
    expect(resolveFileIcon("untitled", "light", { language: "Toml" })).toBe("/file-icons/files/gear.svg");
    expect(resolveFileIcon("download", "light", { mimeType: "image/png; charset=binary" })).toBe(
      "/file-icons/files/image.svg",
    );
    expect(resolveFileIcon("download", "light", { mimeType: "application/zip" })).toBe(
      "/file-icons/files/compressed.svg",
    );
    expect(resolveFileIcon("data", "light", { mimeType: "text/markdown" })).toBe("/file-icons/files/markdown.svg");
  });

  it("symlinks_and_unknown_files_fall_back_cleanly", () => {
    expect(resolveFileIcon("README.md", "light")).toBe("/file-icons/files/markdown.svg");
    expect(resolveFileIcon("unknown.unrecognized", "light")).toBe("/file-icons/files/document.svg");
    expect(resolveFileIcon("", "light")).toBe("/file-icons/files/document.svg");
  });

  it("dark mode is a distinct asset path (the theme-switch cache buster)", () => {
    for (const name of ["main.ts", "main.rs", "config.toml", "notes.txt"]) {
      const light = resolveFileIcon(name, "light");
      const dark = resolveFileIcon(name, "dark");
      expect(dark).not.toBe(light);
      expect(dark.startsWith("/file-icons/dark/")).toBe(true);
      expect(light.startsWith("/file-icons/dark/")).toBe(false);
    }
    expect(resolveDirectoryIcon("src", "dark")).toBe("/file-icons/dark/folders/folder-orange-code.svg");
    expect(fileIconAssetPath("files/rust.svg", "dark")).toBe("/file-icons/dark/files/rust.svg");
    expect(fileIconAssetPath("files/rust.svg", "light")).toBe("/file-icons/files/rust.svg");
  });

  it("has_specific_file_icon skips the generic fallback without hints", () => {
    expect(hasSpecificFileIcon("src/component.test.tsx")).toBe(true);
    expect(hasSpecificFileIcon("config/.env.local")).toBe(true);
    expect(hasSpecificFileIcon("version.unknown")).toBe(false);
    expect(hasSpecificFileIcon("plainname")).toBe(false);
  });

  it("well_bg is the near-black/near-white counter-shade", () => {
    expect(wellBg("dark", false)).toBe("rgb(0 0 0 / 0.16)");
    expect(wellBg("dark", true)).toBe("rgb(0 0 0 / 0.32)");
    expect(wellBg("light", false)).toBe("rgb(255 255 255 / 0.16)");
    expect(wellBg("light", true)).toBe("rgb(255 255 255 / 0.32)");
  });
});
