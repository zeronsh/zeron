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
ver="$(curl -fsSL --retry 3 "$ZERON_EDGE_URL/releases/latest.txt" | tr -d '[:space:]')"
case "$ver" in
  "" | *[!A-Za-z0-9._+-]*) echo "zeron cloud install: bad release version '$ver'" >&2; exit 1 ;;
esac

restart=0
dest="$app/$ver"
if [ -x "$dest/zeron" ]; then
  echo "zeron $ver already installed"
else
  tmp="$(mktemp -d "$data/.download.XXXXXX")"
  trap 'rm -rf "$tmp"' EXIT
  curl -fsSL --retry 3 -o "$tmp/zeron.tar.gz" "$ZERON_EDGE_URL/releases/zeron-$ver-linux-$arch.tar.gz"
  mkdir "$tmp/unpacked"
  tar -xzf "$tmp/zeron.tar.gz" -C "$tmp/unpacked" --strip-components=1
  # Probe before switching: a binary that cannot start must not replace one that can.
  "$tmp/unpacked/zeron" --version >/dev/null
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
After=network-online.target
Wants=network-online.target

[Service]
User=$run_user
WorkingDirectory=$home
Environment=HOME=$home
EnvironmentFile=$data/env
ExecStart=$app/current/zeron headless
Restart=always
RestartSec=5

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

sudo systemctl daemon-reload
sudo systemctl enable ${CLOUD_UNIT_NAME} >/dev/null
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
