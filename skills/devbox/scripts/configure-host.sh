#!/usr/bin/env bash
# Shared owner-private bootstrap. Never provision, authenticate or start an engine.
set -euo pipefail
: "${CREW_DEVBOX_NAME:?Set the owner-qualified devbox name}"
: "${COMET_DEVBOX_PROVIDER:?Set namespace or boat}"
: "${COMET_DEVBOX_ID:?Set the immutable provider ID}"
: "${CREW_BINARY:?Set the immutable verified Crew executable}"
: "${CREW_BINARY_SHA256:?Set its verified raw executable SHA-256}"
export CREW_CHANNEL="${CREW_CHANNEL:-production}"
here="$(dirname "${BASH_SOURCE[0]}")"
for tool in node npm python3 omp gcloud; do command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 1; }; done
case "$COMET_DEVBOX_PROVIDER" in
  boat) command -v boat >/dev/null ;;
  namespace) command -v tmux >/dev/null ;;
  *) echo 'Expected namespace or boat.' >&2; exit 1 ;;
esac
if [[ "${CREW_DEVBOX_SETUP_PROTECTED:-}" != 1 ]]; then
  exec python3 "$here/keepalive.py" env CREW_DEVBOX_SETUP_PROTECTED=1 bash "$0" "$@"
fi
python3 "$here/configure-host.py" configure
browser_home="$HOME/.local/share/crew-devbox-browser"
mkdir -p "$browser_home" "$HOME/.local/bin" "$HOME/.config/crew-devbox"
npm install --prefix "$browser_home" --no-audit --no-fund --save-exact playwright@1.57.0
export PLAYWRIGHT_BROWSERS_PATH="$browser_home/browsers"
browser="$(node -e 'process.stdout.write(require(process.argv[1]).chromium.executablePath())' "$browser_home/node_modules/playwright")"
if [[ ! -x "$browser" ]]; then
  node "$browser_home/node_modules/playwright/cli.js" install --with-deps chromium
fi
[[ -x "$browser" ]] || { echo 'Chromium installation did not produce an executable.' >&2; exit 1; }
printf '#!/usr/bin/env bash\nexec %q "$@"\n' "$browser" > "$HOME/.local/bin/crew-chromium"
chmod 755 "$HOME/.local/bin/crew-chromium"

cat > "$HOME/.local/bin/crew-devbox-gcloud-login" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
command -v gcloud >/dev/null || { echo 'gcloud is required for gcloud login.' >&2; exit 1; }
export DISPLAY=:99
export BROWSER=/bin/echo
exec gcloud auth login --force "$@"
SH
chmod 755 "$HOME/.local/bin/crew-devbox-gcloud-login"

# Preserve personal guidance; migrate only this setup's bounded managed block.
python3 - "$CREW_DEVBOX_NAME" "$CREW_CHANNEL" "$COMET_DEVBOX_PROVIDER" <<'PY'
from pathlib import Path
import sys
name, channel, provider = sys.argv[1:]
start, end = '<!-- crew-devbox:start -->', '<!-- crew-devbox:end -->'
blocks = [(start, end), ('<!-- crew-namespace-devbox:start -->', '<!-- crew-namespace-devbox:end -->')]
block = f'''{start}
## Devbox execution
You are running on {provider} devbox `{name}`, not the user's Mac.
- Credentials must be created and stored on this devbox. Preserve interactive browser approval and MFA; never copy credential directories or use gcloud remote bootstrap.
- Setup owns the short-lived callback tunnels. For gcloud reauthentication use `~/.local/bin/crew-devbox-gcloud-login`; if localhost:8085 is not forwarded, rerun the devbox skill's authentication phase. Do not start a second login while the existing flow is waiting.
- Never print tokens, keys, cookies, callback codes, or secret files. Use Crew sign-in, Crew Agent Auth, and Infisical localdev rendering, not production credentials or separate model-provider logins.
- Chromium is `~/.local/bin/crew-chromium`. Keep CDP, Tilt and credential callbacks loopback-only. The web app must retain its maintained Docker-reachable binding for pod callbacks through `host.docker.internal:3000`; keep public inbound app access denied by provider private access or the host firewall. Never publish these services through provider hosting.
- Crew protects executing turns, not parked harnesses. For independent work that outlives a turn, use `~/.local/bin/crew-devbox-keepalive <command> [arguments...]`. Namespace uses owned task markers; Boat renews a finite lease and stops the command if renewal fails. Inspect partial work before retrying.
- A crash can leave a Namespace marker behind. Remove only confirmed abandoned owned markers; never clear all tasks. Boat stop does not run ordinary process cleanup, and resume restarts processes rather than restoring memory.
- Keep `ASHLER_INCREMENTAL_TSC_CHECKS=false`; do not run local typechecks. Use Docker + kind + Tilt, not OrbStack. Never reset or seed a shared or production database.
{end}'''
for relative in ('.omp/agent/AGENTS.md', '.claude/CLAUDE.md', '.codex/AGENTS.md'):
    path = Path.home() / relative
    old = path.read_text() if path.exists() else ''
    for first, last in blocks:
        if first in old or last in old:
            if old.count(first) != 1 or old.count(last) != 1 or old.index(first) > old.index(last):
                raise SystemExit(f'Malformed managed guidance in {path}')
            before, rest = old.split(first, 1)
            _, after = rest.split(last, 1)
            old = before + after
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(old.rstrip() + '\n\n' + block + '\n')

# Namespace runs its declared dev-shell at boot. Boat uses one systemd unit.
path = Path.home() / '.bashrc'
old = path.read_text() if path.exists() else ''
managed = [
    (f'# >>> Crew Devbox {channel} autostart >>>', f'# <<< Crew Devbox {channel} autostart <<<'),
    ('# >>> Crew Devbox autostart >>>', '# <<< Crew Devbox autostart <<<'),
]
for first, last in managed:
    if first in old or last in old:
        if old.count(first) != 1 or old.count(last) != 1 or old.index(first) > old.index(last):
            raise SystemExit(f'Malformed Crew startup block in {path}')
        before, rest = old.split(first, 1)
        _, after = rest.split(last, 1)
        old = before + after
if provider == 'namespace':
    first, last = f'# >>> Crew Devbox {channel} autostart >>>', f'# <<< Crew Devbox {channel} autostart <<<'
    old = old.rstrip() + f'\n\n{first}\n"$HOME/.local/bin/crew-devbox-autostart" {channel}\n{last}\n'
path.write_text(old)
PY
"$HOME/.local/bin/crew-chromium" --version
printf 'Configured %s. Existing engines were NOT restarted.\n' "$CREW_CHANNEL"
