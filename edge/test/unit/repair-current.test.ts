import { execFileSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readlinkSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { REPAIR_ENGINE_SCRIPT } from "../../src/cloud/install-script";

// A machine restored from its sleep snapshot came back with `current` naming
// 0.3.14, which the update to 0.3.15 had removed (2026-11-23): every start of
// the unit failed (203/EXEC) and the wake gave up. The repair the unit runs
// before each start points it back at the newest version installed.
describe("REPAIR_ENGINE_SCRIPT and `current`", () => {
  let root = "";
  afterEach(() => {
    if (root) rmSync(root, { recursive: true, force: true });
  });

  const version = (app: string, v: string) => {
    mkdirSync(join(app, v), { recursive: true });
    for (const bin of ["zeron", "zeron-engine"]) {
      writeFileSync(join(app, v, bin), "#!/bin/sh\n");
      chmodSync(join(app, v, bin), 0o755);
    }
  };
  const repair = (home: string) =>
    execFileSync("sh", ["-c", REPAIR_ENGINE_SCRIPT], {
      env: { PATH: "/usr/bin:/bin", ZERON_CLOUD_HOME: home },
      encoding: "utf8"
    });

  it("points a `current` naming a removed version at the newest installed", () => {
    root = mkdtempSync(join(tmpdir(), "zeron-repair-current-"));
    const app = join(root, ".zeron", "app");
    version(app, "0.3.9");
    version(app, "0.3.15");
    symlinkSync(join(app, "0.3.14"), join(app, "current"));
    expect(repair(root)).toContain("now 0.3.15");
    expect(readlinkSync(join(app, "current"))).toBe(join(app, "0.3.15"));
  });

  it("leaves a `current` that runs alone, even when a newer version is there", () => {
    root = mkdtempSync(join(tmpdir(), "zeron-repair-current-"));
    const app = join(root, ".zeron", "app");
    version(app, "0.3.14");
    version(app, "0.3.15");
    symlinkSync(join(app, "0.3.14"), join(app, "current"));
    expect(repair(root)).toBe("");
    expect(readlinkSync(join(app, "current"))).toBe(join(app, "0.3.14"));
  });

  it("does nothing when no version is installed", () => {
    root = mkdtempSync(join(tmpdir(), "zeron-repair-current-"));
    const app = join(root, ".zeron", "app");
    mkdirSync(app, { recursive: true });
    symlinkSync(join(app, "0.3.14"), join(app, "current"));
    expect(repair(root)).toBe("");
    expect(readlinkSync(join(app, "current"))).toBe(join(app, "0.3.14"));
  });
});
