/**
 * Runs the real Cloud install script (src/cloud/install-script.ts) under sh
 * in a temp dir, with `curl`/`sudo`/`systemctl`/`flock`/`uname` stubbed on
 * PATH, to prove what a workflow step retry relies on: re-running converges
 * without re-downloading or restarting, the env file is 0600, and a changed
 * enrollment value (re-provision) rewrites it and restarts the service.
 */
import { execFileSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readlinkSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir, userInfo } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { CLOUD_INSTALL_SCRIPT, installCommand, type SessionEnv } from "../../src/cloud/install-script";

/** One session's provisioning (the workflow's create env). */
const session = (enroll: string, edge = "https://edge.test", overrides: Partial<SessionEnv> = {}): SessionEnv => ({
  ZERON_RUNNER_ENROLL: enroll,
  ZERON_EDGE_URL: edge,
  ZERON_DEVICE_NAME: "Cloud session",
  ZERON_DEVICE_PLATFORM: "cloud",
  ZERON_CLOUD_ACCOUNT: "cloud-account-1",
  ZERON_CLOUD_CHAT: "chat-1",
  ZERON_CLOUD_REPO: "https://github.com/acme/app.git",
  ZERON_CLOUD_PATH: "/home/user/app",
  ZERON_CLOUD_BRANCH: "main",
  ...overrides
});

let root: string;

const stub = (bin: string, name: string, body: string) => {
  const path = join(bin, name);
  writeFileSync(path, `#!/bin/sh\n${body}\n`);
  chmodSync(path, 0o755);
};

const setup = () => {
  root = mkdtempSync(join(tmpdir(), "zeron-cloud-install-"));
  const bin = join(root, "bin");
  const home = join(root, "home");
  const units = join(root, "units");
  const release = join(root, "release");
  for (const dir of [bin, home, units, join(release, "zeron-1.2.3-linux-x86_64")]) mkdirSync(dir, { recursive: true });
  const fakeZeron = join(release, "zeron-1.2.3-linux-x86_64", "zeron");
  writeFileSync(fakeZeron, "#!/bin/sh\necho zeron 1.2.3\n");
  chmodSync(fakeZeron, 0o755);
  execFileSync("tar", ["-czf", join(release, "zeron.tar.gz"), "-C", release, "zeron-1.2.3-linux-x86_64"]);
  const log = join(root, "calls.log");
  writeFileSync(log, "");
  // curl: latest.txt prints the version; the tarball is copied to -o.
  stub(bin, "curl", `echo "curl $*" >> "${log}"
out=""; url=""
while [ $# -gt 0 ]; do case "$1" in -o) out="$2"; shift 2;; -*) shift;; *) url="$1"; shift;; esac; done
case "$url" in
  */releases/latest.txt) echo "1.2.3" ;;
  */releases/zeron-1.2.3-linux-x86_64.tar.gz) cp "${join(release, "zeron.tar.gz")}" "$out" ;;
  *) exit 22 ;;
esac`);
  stub(bin, "sudo", `"$@"`);
  stub(bin, "systemctl", `echo "systemctl $*" >> "${log}"`);
  stub(bin, "flock", "exit 0");
  stub(bin, "uname", `[ "$1" = "-m" ] && echo x86_64 || /usr/bin/uname "$@"`);
  const script = join(root, "install.sh");
  writeFileSync(script, CLOUD_INSTALL_SCRIPT);
  const run = (enroll: string) =>
    execFileSync("sh", [script], {
      env: {
        PATH: `${bin}:/usr/bin:/bin`,
        ...session(enroll),
        ZERON_CLOUD_HOME: home,
        ZERON_CLOUD_UNIT_DIR: units
      },
      encoding: "utf8"
    });
  const calls = () => readFileSync(log, "utf8").trim().split("\n").filter(Boolean);
  const clearCalls = () => writeFileSync(log, "");
  return { home, units, run, calls, clearCalls };
};

beforeEach(() => {
  root = "";
});
afterEach(() => {
  if (root) rmSync(root, { recursive: true, force: true });
});

describe("cloud install script", () => {
  it("is ASCII (it travels base64-encoded via btoa) and carries its idempotency guards", () => {
    expect(/^[\x00-\x7f]*$/.test(CLOUD_INSTALL_SCRIPT)).toBe(true);
    for (const marker of ["#!/bin/sh", "set -eu", "umask 077", "flock 9", 'if [ -x "$dest/zeron" ]', "ln -sfn", "chmod 600", "cmp -s", "systemctl enable", "EnvironmentFile="]) {
      expect(CLOUD_INSTALL_SCRIPT).toContain(marker);
    }
  });

  it("installs once, converges on re-run, and restarts only on change", () => {
    const { home, units, run, calls, clearCalls } = setup();
    run("org.user.cloud-1.codeA");

    const data = join(home, ".zeron");
    expect(readlinkSync(join(data, "app", "current"))).toBe(join(data, "app", "1.2.3"));
    expect(existsSync(join(data, "app", "1.2.3", "zeron"))).toBe(true);
    const envFile = join(data, "env");
    expect(statSync(envFile).mode & 0o777).toBe(0o600);
    expect(readFileSync(envFile, "utf8")).toBe(
      [
        "ZERON_RUNNER_ENROLL=org.user.cloud-1.codeA",
        "ZERON_EDGE_URL=https://edge.test",
        'ZERON_DEVICE_NAME="Cloud session"',
        "ZERON_DEVICE_PLATFORM=cloud",
        "ZERON_CLOUD_ACCOUNT=cloud-account-1",
        "ZERON_CLOUD_CHAT=chat-1",
        "ZERON_CLOUD_REPO=https://github.com/acme/app.git",
        "ZERON_CLOUD_PATH=/home/user/app",
        "ZERON_CLOUD_BRANCH=main",
        "ZERON_AUTO_UPDATE=1",
        `HOME=${home}`,
        ""
      ].join("\n")
    );
    const unit = readFileSync(join(units, "zeron-cloud.service"), "utf8");
    expect(statSync(join(units, "zeron-cloud.service")).mode & 0o777).toBe(0o644);
    expect(unit).toContain(`User=${userInfo().username}`);
    expect(unit).toContain(`WorkingDirectory=${home}`);
    expect(unit).toContain(`EnvironmentFile=${envFile}`);
    expect(unit).toContain(`ExecStart=${join(data, "app", "current", "zeron")} headless`);
    expect(unit).toContain("Restart=always");
    expect(calls().filter((c) => c.startsWith("curl"))).toHaveLength(2);
    expect(calls()).toContain("systemctl restart zeron-cloud.service");

    // A retried step: nothing to download, nothing changed, no restart.
    clearCalls();
    run("org.user.cloud-1.codeA");
    expect(calls().filter((c) => c.startsWith("curl"))).toHaveLength(1); // latest.txt only
    expect(calls()).toContain("systemctl start zeron-cloud.service");
    expect(calls()).not.toContain("systemctl restart zeron-cloud.service");
    expect(calls()).toContain("systemctl enable zeron-cloud.service");

    // A re-provision with a fresh code rewrites the env file and restarts.
    clearCalls();
    run("org.user.cloud-1.codeB");
    expect(readFileSync(envFile, "utf8")).toContain("ZERON_RUNNER_ENROLL=org.user.cloud-1.codeB\n");
    expect(statSync(envFile).mode & 0o777).toBe(0o600);
    expect(calls()).toContain("systemctl restart zeron-cloud.service");
  });

  it("builds a quote-safe command and refuses values that could break out", () => {
    const command = installCommand(session("org_1.user_1.cloud-abc.Zm9v-_x", "https://edge.zeron.sh"));
    expect(command).toContain("ZERON_RUNNER_ENROLL='org_1.user_1.cloud-abc.Zm9v-_x'");
    expect(command).toContain("ZERON_EDGE_URL='https://edge.zeron.sh'");
    expect(command).toContain("ZERON_CLOUD_REPO='https://github.com/acme/app.git'");
    expect(() => installCommand(session("org.user.dev.co'de", "https://edge.zeron.sh"))).toThrow();
    expect(() => installCommand(session("org.user.dev.code", "https://edge.zeron.sh/x'; rm -rf /"))).toThrow();
    expect(() => installCommand(session("org.user.dev.code", "https://edge.test", { ZERON_CLOUD_REPO: "https://github.com/a/b'; id" }))).toThrow();
    expect(() => installCommand(session("org.user.dev.code", "https://edge.test", { ZERON_CLOUD_PATH: "/home/user/../../etc" }))).toThrow();
    expect(() => installCommand(session("org.user.dev.code", "https://edge.test", { ZERON_CLOUD_BRANCH: "main; reboot" }))).toThrow();
  });

  it("runs end to end through the command wrapper", () => {
    const { home, units } = setup();
    const command = installCommand(session("org.user.cloud-1.codeC"));
    execFileSync("sh", ["-c", command], {
      env: {
        PATH: `${join(root, "bin")}:/usr/bin:/bin`,
        ZERON_CLOUD_HOME: home,
        ZERON_CLOUD_UNIT_DIR: units
      },
      encoding: "utf8"
    });
    expect(readFileSync(join(home, ".zeron", "env"), "utf8")).toContain("ZERON_RUNNER_ENROLL=org.user.cloud-1.codeC\n");
  });
});
