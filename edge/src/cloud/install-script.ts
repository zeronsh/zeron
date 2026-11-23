/**
 * The Cloud engine installer, run inside the user's sandbox by
 * ProvisionWorkflow's `install-engine` step via `SandboxProvider.exec`.
 *
 * Provider-neutral by contract (sandbox-provider.ts): POSIX sh on Linux
 * x86_64/aarch64 with curl, tar, sudo and systemd; it runs as whatever
 * unprivileged user `exec` uses and installs into that user's home. Its
 * inputs (`ZERON_RUNNER_ENROLL`, `ZERON_EDGE_URL`) come from the sandbox's
 * create-time env, overridable inline by the command (installCommand).
 *
 * Not the public install.sh: that one installs a local-only *user* service.
 * The Cloud device needs a *system* unit — enabled system units are what a
 * provider restarts after stop/resume — whose environment comes from an
 * EnvironmentFile (~/.zeron/env, 0600), because the sandbox env reaches
 * `exec` commands but not systemd services.
 *
 * Idempotent by construction: providers don't retry commands and a lost
 * reply may mean it is still running, so a workflow step retry (or a
 * re-provision of the same sandbox) re-runs this while a previous copy may
 * be mid-flight. Hence the flock, the "already downloaded" skip, atomic
 * env-file replace, and restart-only-on-change.
 *
 * `ZERON_CLOUD_HOME` / `ZERON_CLOUD_UNIT_DIR` exist so the unit tier can run
 * the real script in a temp dir; production never sets them.
 */

export const CLOUD_UNIT_NAME = "zeron-cloud.service";

/**
 * Starts the engine when a machine comes back, without anyone asking over the
 * provider's API. A sleep stops the engine's unit first (it has to drain), and
 * a machine restored from that snapshot can come back with the unit stopped.
 * This one is never stopped: `RemainAfterExit` keeps it active through the
 * snapshot, so it starts again whether the provider brings back every enabled
 * unit or only the ones that were running. `--no-block`: a unit's start must
 * not wait on another's during boot.
 */
export const CLOUD_BOOT_UNIT_NAME = "zeron-cloud-boot.service";
export const CLOUD_BOOT_UNIT = `[Unit]
Description=Start the Zeron Cloud engine when the machine comes back

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/bin/systemctl start --no-block ${CLOUD_UNIT_NAME}

[Install]
WantedBy=multi-user.target
`;

/**
 * Puts back the release's `zeron-engine` beside the `zeron` the unit runs, when
 * it's missing. Since 0.3.0 the release's `zeron` is the zgx app, whose
 * `zeron headless` hands off to that engine: without it the unit exits at
 * once, restarts every second and never enrolls. Machines lost it to the
 * installer, the engine's own updater and the pre-sleep tidy, which all left
 * it out on Cloud (0.3.5's still do), so the unit runs this before every start
 * (`ExecStartPre=-`: it never fails a start). Downloads only while it's
 * missing, the version `current` names, from the edge's own releases.
 */
export const REPAIR_ENGINE_SCRIPT = `#!/bin/sh
# Zeron Cloud: put back a missing zeron-engine (edge/src/cloud/install-script.ts).
home="\${ZERON_CLOUD_HOME:-$HOME}"
cur="$home/.zeron/app/current"
[ -x "$cur/zeron" ] || exit 0
[ -f "$cur/zeron-engine" ] && exit 0
dest="$(readlink -f "$cur")" || exit 0
ver="$(basename "$dest")"
case "$ver" in
  "" | *[!A-Za-z0-9._+-]*) exit 0 ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) exit 0 ;;
esac
[ -n "\${ZERON_EDGE_URL:-}" ] || exit 0
tmp="$(mktemp -d "$home/.zeron/.repair.XXXXXX")" || exit 0
trap 'rm -rf "$tmp"' EXIT
curl -fsSL --retry 3 --connect-timeout 15 --speed-limit 1048576 --speed-time 15 \\
  -o "$tmp/zeron.tar.gz" "$ZERON_EDGE_URL/releases/zeron-$ver-linux-$arch.tar.gz" || exit 0
tar -xzf "$tmp/zeron.tar.gz" -C "$tmp" "zeron-$ver-linux-$arch/zeron-engine" || exit 0
chmod 755 "$tmp/zeron-$ver-linux-$arch/zeron-engine"
mv -f "$tmp/zeron-$ver-linux-$arch/zeron-engine" "$dest/zeron-engine"
echo "zeron cloud: put back zeron-engine for $ver"
`;

/**
 * Writes the repair script (`REPAIR_ENGINE_SCRIPT`) to `~/.zeron/repair-engine`
 * and a drop-in running it before the engine's unit starts, each only when it
 * differs, reloading systemd only then. Run by the installer and by every
 * wake's first command, so machines set up before it get it as they wake.
 * Best effort: a failure here never stops what follows.
 * `ZERON_CLOUD_HOME` / `ZERON_CLOUD_UNIT_DIR` exist for the unit tier.
 */
export const ENSURE_REPAIR =
  `{ h="\${ZERON_CLOUD_HOME:-$HOME}"; r="$h/.zeron/repair-engine"; ` +
  `dd="\${ZERON_CLOUD_UNIT_DIR:-/etc/systemd/system}/${CLOUD_UNIT_NAME}.d"; c=0; ` +
  `t="$(mktemp)" && printf %s '${btoa(REPAIR_ENGINE_SCRIPT)}' | base64 -d >"$t" && ` +
  `{ cmp -s "$t" "$r" || install -m 0700 "$t" "$r"; } && ` +
  `printf '[Service]\\nExecStartPre=-%s\\n' "$r" >"$t" && ` +
  `{ cmp -s "$t" "$dd/repair-engine.conf" || { sudo mkdir -p "$dd" && sudo install -m 0644 "$t" "$dd/repair-engine.conf" && c=1; }; } && ` +
  // A unit written before OOMPolicy=continue was in it gets it as a drop-in.
  `printf '[Service]\\nOOMPolicy=continue\\n' >"$t" && ` +
  `{ cmp -s "$t" "$dd/oom-policy.conf" || { sudo mkdir -p "$dd" && sudo install -m 0644 "$t" "$dd/oom-policy.conf" && c=1; }; } && ` +
  `{ [ "$c" = 0 ] || sudo systemctl daemon-reload; }; ` +
  `rm -f "$t"; } >/dev/null 2>&1 || true`;

export const CLOUD_INSTALL_SCRIPT = `#!/bin/sh
# Zeron Cloud engine installer (edge/src/cloud/install-script.ts).
# POSIX sh, idempotent, provider-neutral.
set -eu

: "\${ZERON_RUNNER_ENROLL:?ZERON_RUNNER_ENROLL is required}"
: "\${ZERON_EDGE_URL:?ZERON_EDGE_URL is required}"

run_user="$(id -un)"
home="\${ZERON_CLOUD_HOME:-$HOME}"
unit_dir="\${ZERON_CLOUD_UNIT_DIR:-/etc/systemd/system}"
data="$home/.zeron"
app="$data/app"
unit="$unit_dir/${CLOUD_UNIT_NAME}"

# Everything this script writes is private to the sandbox user.
umask 077
mkdir -p "$app"

# One installer at a time: a retried step may overlap a still-running one.
exec 9>"$data/.cloud-install.lock"
flock 9

case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) echo "zeron cloud install: unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac

# (No pipefail in POSIX sh: a failed fetch yields an empty version, refused below.)
ver="$(curl -fsSL --retry 3 --max-time 30 "$ZERON_EDGE_URL/releases/latest.txt" | tr -d '[:space:]')"
case "$ver" in
  "" | *[!A-Za-z0-9._+-]*) echo "zeron cloud install: bad release version '$ver'" >&2; exit 1 ;;
esac

restart=0
dest="$app/$ver"
# (Whole, its zeron-engine too: \`zeron headless\` runs it, and installs
# made before 2026-11-15 left it out.)
if [ -x "$dest/zeron" ] && [ -f "$dest/zeron-engine" ]; then
  echo "zeron $ver already installed"
else
  tmp="$(mktemp -d "$data/.download.XXXXXX")"
  trap 'rm -rf "$tmp"' EXIT
  # A stalled connection is dropped and redialed (curl retries the timeout):
  # without a floor one bad connection trickled for good, holding the lock
  # every retried install then queued behind.
  curl -fsSL --retry 3 --connect-timeout 15 --speed-limit 1048576 --speed-time 15 \\
    -o "$tmp/zeron.tar.gz" "$ZERON_EDGE_URL/releases/zeron-$ver-linux-$arch.tar.gz"
  mkdir "$tmp/unpacked"
  tar -xzf "$tmp/zeron.tar.gz" -C "$tmp/unpacked" --strip-components=1
  # Probe before switching: a binary that cannot start must not replace one that can.
  "$tmp/unpacked/zeron" --version >/dev/null
  # (zeron-engine stays: \`zeron headless\` runs it. Without it the unit
  # exits at once and the machine never enrolls.)
  rm -rf "$dest"
  mv "$tmp/unpacked" "$dest"
fi
if [ "$(readlink "$app/current" 2>/dev/null || true)" != "$dest" ]; then
  ln -sfn "$dest" "$app/current"
  restart=1
fi

# EnvironmentFile for the unit (0600: it holds the one-time enrollment
# code): the session's identity and project, exactly as provisioned.
put() { if [ -n "$2" ]; then printf '%s=%s\\n' "$1" "$2"; fi; }
env_new="$(mktemp "$data/.env.XXXXXX")"
{
  put ZERON_RUNNER_ENROLL "$ZERON_RUNNER_ENROLL"
  put ZERON_EDGE_URL "$ZERON_EDGE_URL"
  printf 'ZERON_DEVICE_NAME="%s"\\n' "\${ZERON_DEVICE_NAME:-Cloud session}"
  put ZERON_DEVICE_PLATFORM "\${ZERON_DEVICE_PLATFORM:-cloud}"
  put ZERON_CLOUD_ACCOUNT "\${ZERON_CLOUD_ACCOUNT:-}"
  put ZERON_CLOUD_CHAT "\${ZERON_CLOUD_CHAT:-}"
  put ZERON_CLOUD_REPO "\${ZERON_CLOUD_REPO:-}"
  put ZERON_CLOUD_PATH "\${ZERON_CLOUD_PATH:-}"
  put ZERON_CLOUD_BRANCH "\${ZERON_CLOUD_BRANCH:-}"
  put ZERON_AUTO_UPDATE 1
  put HOME "$home"
} >"$env_new"
chmod 600 "$env_new"
if cmp -s "$env_new" "$data/env"; then
  rm -f "$env_new"
else
  mv -f "$env_new" "$data/env"
  restart=1
fi

unit_new="$(mktemp "$data/.unit.XXXXXX")"
cat >"$unit_new" <<UNIT
[Unit]
Description=Zeron Cloud engine
StartLimitIntervalSec=0

[Service]
User=$run_user
WorkingDirectory=$home
Environment=HOME=$home
EnvironmentFile=$data/env
ExecStart=$app/current/zeron headless
Restart=always
RestartSec=1
# A program an agent runs that the kernel kills for memory dies alone: the
# engine (and every chat on the machine) carries on. systemd's default stops
# the whole unit, so one runaway build or test restarted the engine.
OOMPolicy=continue

[Install]
WantedBy=multi-user.target
UNIT
if sudo cmp -s "$unit_new" "$unit"; then
  rm -f "$unit_new"
else
  sudo install -m 0644 "$unit_new" "$unit"
  rm -f "$unit_new"
  restart=1
fi

boot_new="$(mktemp "$data/.unit.XXXXXX")"
cat >"$boot_new" <<UNIT
${CLOUD_BOOT_UNIT}UNIT
if sudo cmp -s "$boot_new" "$unit_dir/${CLOUD_BOOT_UNIT_NAME}"; then
  rm -f "$boot_new"
else
  sudo install -m 0644 "$boot_new" "$unit_dir/${CLOUD_BOOT_UNIT_NAME}"
  rm -f "$boot_new"
fi

# A zeron-engine an update leaves out is put back before each start.
${ENSURE_REPAIR}
sudo systemctl daemon-reload
sudo systemctl enable ${CLOUD_UNIT_NAME} ${CLOUD_BOOT_UNIT_NAME} >/dev/null
sudo systemctl start ${CLOUD_BOOT_UNIT_NAME}
if [ "$restart" = 1 ]; then
  sudo systemctl restart ${CLOUD_UNIT_NAME}
else
  sudo systemctl start ${CLOUD_UNIT_NAME}
fi
echo "zeron cloud: $ver ready"
`;

/** `orgId.userId.deviceId.code` (ids + base64url only). */
const ENROLL_RE = /^[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$/;
const ORIGIN_RE = /^https?:\/\/[A-Za-z0-9.-]+(:\d{1,5})?$/;
const ID_RE = /^[A-Za-z0-9_-]{1,128}$/;
/** Git remote / checkout path / branch: no quotes, spaces, `$` or `..`. */
const REPO_RE = /^https:\/\/[A-Za-z0-9.-]+(\/[A-Za-z0-9._-]+)+$/;
const PATH_RE = /^\/[A-Za-z0-9._\/-]{1,255}$/;
const BRANCH_RE = /^[A-Za-z0-9._\/-]{1,200}$/;

export const isEdgeOrigin = (value: string): boolean => ORIGIN_RE.test(value);
export const isRepoUrl = (value: string): boolean => REPO_RE.test(value) && !value.includes("..");
export const isCheckoutPath = (value: string): boolean => PATH_RE.test(value) && !value.split("/").includes("..");
export const isBranch = (value: string): boolean => BRANCH_RE.test(value) && !value.includes("..");

/** Everything a session sandbox is provisioned with (also its create env). */
export interface SessionEnv {
  readonly ZERON_RUNNER_ENROLL: string;
  readonly ZERON_EDGE_URL: string;
  readonly ZERON_DEVICE_NAME: string;
  readonly ZERON_DEVICE_PLATFORM: string;
  readonly ZERON_CLOUD_ACCOUNT: string;
  readonly ZERON_CLOUD_CHAT: string;
  readonly ZERON_CLOUD_REPO: string;
  readonly ZERON_CLOUD_PATH: string;
  readonly ZERON_CLOUD_BRANCH: string;
}

const CHECKS: Record<keyof SessionEnv, (v: string) => boolean> = {
  ZERON_RUNNER_ENROLL: (v) => ENROLL_RE.test(v),
  ZERON_EDGE_URL: isEdgeOrigin,
  ZERON_DEVICE_NAME: (v) => /^[A-Za-z0-9 ._-]{1,64}$/.test(v),
  ZERON_DEVICE_PLATFORM: (v) => ID_RE.test(v),
  ZERON_CLOUD_ACCOUNT: (v) => ID_RE.test(v),
  ZERON_CLOUD_CHAT: (v) => ID_RE.test(v),
  ZERON_CLOUD_REPO: isRepoUrl,
  ZERON_CLOUD_PATH: isCheckoutPath,
  ZERON_CLOUD_BRANCH: isBranch
};

/**
 * The shell command sent through `SandboxProvider.exec`. The script travels
 * base64-encoded (no quoting hazards) and every value is passed explicitly
 * rather than relying on the sandbox env alone, because a re-provision of an
 * existing sandbox mints a fresh enrollment code that the sandbox's
 * create-time env does not have. Every value is validated to a quote-free
 * charset first.
 */
export const installCommand = (env: SessionEnv): string => {
  const assignments = (Object.keys(CHECKS) as (keyof SessionEnv)[]).map((name) => {
    const value = env[name];
    if (!CHECKS[name](value)) throw new Error(`bad ${name}`);
    return `${name}='${value}'`;
  });
  const script = btoa(CLOUD_INSTALL_SCRIPT);
  return (
    `f="$(mktemp)" && printf %s '${script}' | base64 -d >"$f" && ` +
    `{ ${assignments.join(" ")} sh "$f"; rc=$?; rm -f "$f"; exit $rc; }`
  );
};

/** Wake fallback when the engine doesn't come back on its own after resume. */
export const RESTART_ENGINE_COMMAND = `sudo systemctl restart ${CLOUD_UNIT_NAME}`;

/** A wake's first command: the sleep stopped the unit before the provider
 * snapshotted the machine, and a resume from that snapshot can bring systemd
 * back with the unit still stopped. Sent as soon as the resume is accepted, so
 * it may be the first to touch a machine still restoring its `/etc`: a unit
 * systemd hasn't read yet gets one reload. `--no-block` answers at once (the
 * engine's first token says it's up). Idempotent: a no-op when it's running. */
export const START_ENGINE_COMMAND =
  // (First, the repair of a missing zeron-engine before each start, for
  // machines set up before it: a no-op once it's there.)
  `${ENSURE_REPAIR}; ` +
  `sudo systemctl start --no-block ${CLOUD_UNIT_NAME} || ` +
  `{ sudo systemctl daemon-reload; sudo systemctl start --no-block ${CLOUD_UNIT_NAME}; }`;

/** A sleep's first step: the engine starts no new work, so nothing begins that
 * the stop would cut off; what arrives meanwhile waits for the woken machine.
 * Exits {@link QUIESCE_BUSY_EXIT} when a turn is in flight or a message is
 * waiting (mirrors `zeron quiesce`, apps/zeron/src/main.rs). */
export const QUIESCE_ENGINE_COMMAND = `"$HOME/.zeron/app/current/zeron" quiesce`;
export const QUIESCE_BUSY_EXIT = 75;

/**
 * Before a sleep's stop: systemd delivers SIGTERM and the engine drains. The
 * unit stays enabled, so it starts again when the sandbox resumes.
 *
 * Machines provisioned before these units existed pick them up here, the one
 * command every machine runs right before the snapshot its next wake restores
 * (nothing re-runs the installer): the wait for `network-online.target` goes
 * (the engine retries its own connections, and that wait can hold `systemctl
 * start` on a resumed machine), a crash restarts in a second, and the boot
 * unit appears. Then everything written is flushed to the disk, as the
 * provider's snapshot follows. Best effort: only the stop decides the exit status.
 * `ZERON_CLOUD_UNIT_DIR` exists for the unit tier.
 */
export const STOP_ENGINE_COMMAND = [
  `d="\${ZERON_CLOUD_UNIT_DIR:-/etc/systemd/system}"`,
  `if [ -f "$d/${CLOUD_UNIT_NAME}" ] && t="$(mktemp)"; then`,
  `  if awk '/^After=network-online.target$/||/^Wants=network-online.target$/||/^StartLimitIntervalSec=/{next} ` +
    `/^RestartSec=5$/{print "RestartSec=1";next} {print} /^Description=Zeron Cloud engine$/{print "StartLimitIntervalSec=0"}' ` +
    `"$d/${CLOUD_UNIT_NAME}" >"$t" && grep -q '^ExecStart=' "$t"; then`,
  `    cmp -s "$t" "$d/${CLOUD_UNIT_NAME}" || sudo install -m 0644 "$t" "$d/${CLOUD_UNIT_NAME}"`,
  `  fi`,
  `  printf '%s' '${CLOUD_BOOT_UNIT}' >"$t"`,
  `  cmp -s "$t" "$d/${CLOUD_BOOT_UNIT_NAME}" || sudo install -m 0644 "$t" "$d/${CLOUD_BOOT_UNIT_NAME}"`,
  `  rm -f "$t"`,
  `  sudo systemctl daemon-reload`,
  `  sudo systemctl enable ${CLOUD_BOOT_UNIT_NAME} >/dev/null 2>&1`,
  `  sudo systemctl start ${CLOUD_BOOT_UNIT_NAME}`,
  `fi`,
  `sudo systemctl stop ${CLOUD_UNIT_NAME}`,
  // The provider snapshots the disk next, from outside the machine: what the
  // engine wrote on its way out (its databases' last checkpoint) must be on the
  // disk by then, not in the page cache. A snapshot without it has held a
  // database with pages from two moments, unreadable on the next wake.
  `s=$?`,
  `sync`,
  `exit $s`
].join("\n");
