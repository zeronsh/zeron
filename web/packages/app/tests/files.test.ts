import { describe, expect, it } from "vitest";
import type { WorkspaceEntry, WorkspaceFileText } from "@zeron/proto";
import {
  compareEntries,
  fileReadOnlyReason,
  formatBytes,
  isDirectChild,
  isImagePath,
  isMarkdownPath,
  parentPath,
  readOnlyMessage,
  writableEncoding,
  writableLineEnding,
} from "../src/lib/files";

function entry(path: string, kind: WorkspaceEntry["kind"]): WorkspaceEntry {
  return { path, name: path.split("/").pop() ?? path, kind, ignored: false, readOnly: kind === "symlink" };
}

describe("compareEntries (desktop compare_paths)", () => {
  it("ranks directories, then files, then symlinks", () => {
    const rows = [entry("b.txt", "file"), entry("link", "symlink"), entry("src", "directory"), entry("A.txt", "file")];
    rows.sort(compareEntries);
    expect(rows.map((row) => row.path)).toEqual(["src", "A.txt", "b.txt", "link"]);
  });

  it("orders names case-insensitively with a path tiebreak", () => {
    const rows = [entry("Beta", "file"), entry("alpha", "file"), entry("Beta2", "file")];
    rows.sort(compareEntries);
    expect(rows.map((row) => row.path)).toEqual(["alpha", "Beta", "Beta2"]);
  });
});

describe("workspace path math", () => {
  it("computes parent paths like the desktop", () => {
    expect(parentPath("src/lib.rs")).toBe("src");
    expect(parentPath("src")).toBe("");
    expect(parentPath("")).toBeNull();
  });

  it("recognizes direct children", () => {
    expect(isDirectChild("src/lib.rs", "src")).toBe(true);
    expect(isDirectChild("src/deep/lib.rs", "src")).toBe(false);
    expect(isDirectChild("README.md", "")).toBe(true);
  });
});

describe("preview classification", () => {
  it("detects images by extension, case-insensitively", () => {
    expect(isImagePath("assets/logo.PNG")).toBe(true);
    expect(isImagePath("docs/photo.jpeg")).toBe(true);
    expect(isImagePath("src/main.rs")).toBe(false);
    expect(isImagePath("no-extension")).toBe(false);
  });

  it("detects markdown like the desktop (md, markdown)", () => {
    expect(isMarkdownPath("README.md")).toBe(true);
    expect(isMarkdownPath("notes.MARKDOWN")).toBe(true);
    expect(isMarkdownPath("src/md.rs")).toBe(false);
  });
});

describe("read-only copy (desktop read_only_message)", () => {
  it("carries the desktop's exact messages", () => {
    expect(readOnlyMessage("binary")).toBe("Binary files cannot be previewed.");
    expect(readOnlyMessage("unsupportedEncoding")).toBe("This file encoding is not supported.");
    expect(readOnlyMessage("symlink")).toBe("Symlink targets are read-only.");
    expect(readOnlyMessage("permissionDenied")).toBe("Permission denied.");
    expect(readOnlyMessage("tooLarge")).toBe("This file is too large to preview.");
    expect(readOnlyMessage("mixedLineEndings")).toBe("Files with mixed line endings are read-only.");
    expect(readOnlyMessage("notRegularFile")).toBe("This file cannot be previewed.");
    expect(readOnlyMessage(null)).toBe("This file cannot be previewed.");
  });
});

describe("writable shape derivation (desktop document.rs)", () => {
  it("maps encodings", () => {
    expect(writableEncoding("utf8")).toBe("utf8");
    expect(writableEncoding("utf8Bom")).toBe("utf8Bom");
    expect(writableEncoding("binary")).toBeNull();
    expect(writableEncoding("unsupported")).toBeNull();
  });

  it("maps line endings", () => {
    expect(writableLineEnding("lf")).toBe("lf");
    expect(writableLineEnding("none")).toBe("lf");
    expect(writableLineEnding("crlf")).toBe("crlf");
    expect(writableLineEnding("mixed")).toBeNull();
    expect(writableLineEnding(null)).toBeNull();
    expect(writableLineEnding(undefined)).toBeNull();
  });
});

function textFile(fields: Partial<WorkspaceFileText>): WorkspaceFileText {
  return {
    checkoutId: "checkout-1",
    path: "src/lib.rs",
    text: "fn main() {}",
    contentHash: "hash-1",
    size: 12,
    encoding: "utf8",
    lineEnding: "lf",
    truncated: false,
    ...fields,
  };
}

describe("fileReadOnlyReason", () => {
  it("prefers the engine's explicit reason", () => {
    expect(fileReadOnlyReason(textFile({ readOnlyReason: "symlink" }))).toBe("symlink");
  });

  it("treats truncated, textless, hashless, and mixed-ending files as notRegularFile", () => {
    expect(fileReadOnlyReason(textFile({ truncated: true }))).toBe("notRegularFile");
    expect(fileReadOnlyReason(textFile({ text: null }))).toBe("notRegularFile");
    expect(fileReadOnlyReason(textFile({ contentHash: null }))).toBe("notRegularFile");
    expect(fileReadOnlyReason(textFile({ lineEnding: "mixed" }))).toBe("notRegularFile");
    expect(fileReadOnlyReason(textFile({ encoding: "binary" }))).toBe("notRegularFile");
  });

  it("accepts an ordinary editable snapshot", () => {
    expect(fileReadOnlyReason(textFile({}))).toBeNull();
    expect(fileReadOnlyReason(textFile({ encoding: "utf8Bom", lineEnding: "crlf" }))).toBeNull();
  });
});

describe("formatBytes", () => {
  it("formats compact sizes", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(2048)).toBe("2.0 KB");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MB");
  });
});
