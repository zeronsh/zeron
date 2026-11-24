#!/usr/bin/env bash
# publish.sh's own test: each case runs the real script against releases
# served from this machine (a scratch folder over http on 127.0.0.1, and
# over https with a certificate made for the run, trusted through
# CURL_CA_BUNDLE), holding it to what it promises: it installs only the
# command line the release manifest names, by its sha256, over https (its
# redirects too), checks a plugin, and refuses everything else before
# anything it downloaded runs.
#
# Usage: actions/publish-plugin/test.sh <zeron binary>
#   The binary is packed as the release's zeron-engine (CI passes the one
#   the job built). Without one it fails (exit 2) rather than skip: a test
#   that quietly skips on CI defends nothing.
# Needs: bash, curl, jq, sha256sum, tar (as publish.sh does), python3 and
# openssl. Stops at the first case that surprises it, non-zero.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/publish.sh"
zeron="${1:-}"
if [[ -z "$zeron" || ! -x "$zeron" ]]; then
  echo "usage: $0 <zeron binary> (built: cargo build -p zeron --bin zeron)" >&2
  exit 2
fi
for tool in curl jq sha256sum tar python3 openssl; do
  command -v "$tool" >/dev/null || { echo "test.sh needs $tool" >&2; exit 2; }
done

work="$(mktemp -d "${TMPDIR:-/tmp}/publish-plugin-test.XXXXXX")"
server=""
cleanup() {
  [[ -n "$server" ]] && kill "$server" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) echo "publish.sh has no command line for $(uname -m): nothing to test here" >&2; exit 2 ;;
esac
version="$("$zeron" --version | awk '{print $2}')"
[[ -n "$version" ]] || { echo "$zeron --version said no version" >&2; exit 2; }
file="zeron-$version-linux-$arch.tar.gz"

# ── Releases ────────────────────────────────────────────────────────────────
# One folder per kind of release, each with manifest.json (latest) and
# manifest-<version>.json, as zeron.sh/releases has them.

release() { # <folder> <manifest json>
  mkdir -p "$work/www/$1"
  printf '%s' "$2" >"$work/www/$1/manifest.json"
  printf '%s' "$2" >"$work/www/$1/manifest-$version.json"
}
manifest() { # <file name> <sha256>
  jq -cn --arg v "$version" --arg f "$1" --arg s "$2" '{version: $v, files: {($f): {sha256: $s}}}'
}

mkdir -p "$work/pack/zeron-$version" "$work/empty/zeron-$version"
cp "$zeron" "$work/pack/zeron-$version/zeron-engine"
tar -czf "$work/good.tar.gz" -C "$work/pack" "zeron-$version"
echo "not a command line" >"$work/empty/zeron-$version/README"
tar -czf "$work/hollow.tar.gz" -C "$work/empty" "zeron-$version"
good_sha="$(sha256sum "$work/good.tar.gz" | cut -d' ' -f1)"
hollow_sha="$(sha256sum "$work/hollow.tar.gz" | cut -d' ' -f1)"

release good "$(manifest "$file" "$good_sha")"
cp "$work/good.tar.gz" "$work/www/good/$file"
release wrong-sha "$(manifest "$file" "$(printf '0%.0s' {1..64})")"
cp "$work/good.tar.gz" "$work/www/wrong-sha/$file"
# The manifest names only another machine's file; this one's is served anyway.
other_arch=$([[ "$arch" == x86_64 ]] && echo aarch64 || echo x86_64)
release other-file "$(manifest "zeron-$version-linux-$other_arch.tar.gz" "$good_sha")"
cp "$work/good.tar.gz" "$work/www/other-file/$file"
# The manifest's sha256 is the served tarball's, but it holds no zeron-engine.
release hollow "$(manifest "$file" "$hollow_sha")"
cp "$work/hollow.tar.gz" "$work/www/hollow/$file"
release no-version '{"files": {}}'
# Asked for the pinned version, the manifest names another (with its own
# tarball, which would run): the pin must hold.
moved="zeron-9.9.9-linux-$arch.tar.gz"
release pin-moved "$(jq -cn --arg f "$moved" --arg s "$good_sha" '{version: "9.9.9", files: {($f): {sha256: $s}}}')"
cp "$work/good.tar.gz" "$work/www/pin-moved/$moved"

# ── The servers ─────────────────────────────────────────────────────────────
# http and https on 127.0.0.1, both serving www/. Over https, /to-http/<path>
# redirects to http://…/good/<path>, and /tarball-to-http/ serves good/'s
# manifests but redirects its tarball there.

openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
  -subj "/CN=127.0.0.1" -addext "subjectAltName=IP:127.0.0.1" \
  -keyout "$work/key.pem" -out "$work/cert.pem" 2>/dev/null
cat >"$work/serve.py" <<'PY'
import http.server, os, ssl, sys, threading

root, ports_file, cert, key = sys.argv[1:5]

class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *a, **kw):
        super().__init__(*a, directory=root, **kw)

    def log_message(self, *a):
        pass

    def do_GET(self):
        if self.server.tls:
            for prefix, everything in (("/to-http/", True), ("/tarball-to-http/", False)):
                if self.path.startswith(prefix):
                    rest = self.path[len(prefix):]
                    if everything or rest.endswith(".tar.gz"):
                        self.send_response(302)
                        self.send_header("Location", f"http://127.0.0.1:{plain.server_address[1]}/good/{rest}")
                        self.end_headers()
                        return
                    self.path = "/good/" + rest
        super().do_GET()

def server(tls):
    s = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    s.tls = tls
    if tls:
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        s.socket = context.wrap_socket(s.socket, server_side=True)
    threading.Thread(target=s.serve_forever, daemon=True).start()
    return s

plain, secure = server(False), server(True)
with open(ports_file + ".tmp", "w") as f:
    f.write(f"{plain.server_address[1]} {secure.server_address[1]}\n")
os.rename(ports_file + ".tmp", ports_file)
threading.Event().wait()
PY
python3 "$work/serve.py" "$work/www" "$work/ports" "$work/cert.pem" "$work/key.pem" &
server=$!
for _ in $(seq 1 100); do [[ -f "$work/ports" ]] && break; sleep 0.05; done
[[ -f "$work/ports" ]] || { echo "the test's release server didn't start" >&2; exit 2; }
read -r http_port https_port <"$work/ports"
http="http://127.0.0.1:$http_port"
https="https://127.0.0.1:$https_port"

# ── Plugins ─────────────────────────────────────────────────────────────────

plugin() { # <folder> <id>
  mkdir -p "$1"
  printf 'return {}\n' >"$1/init.luau"
  printf '# Focus\n\nWhat you are on.\n' >"$1/README.md"
  printf '## 1.0.0\n\n- First.\n' >"$1/CHANGELOG.md"
  printf '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6"/></svg>' >"$1/icon.svg"
  jq -n --arg id "$2" '{id: $id, name: "Focus", version: "1.0.0", entry: "init.luau", permissions: [],
    description: "What you are on.", icon: "icon.svg", categories: ["productivity"]}' >"$1/zeron-plugin.json"
}
plugin "$work/plugins/ok" focus-ci
plugin "$work/plugins/reserved" all

# ── The cases ───────────────────────────────────────────────────────────────

passed=0
last_out=""
# expect <name> <exit> <words it says> [VAR=value …]: run publish.sh as the
# action does (a scratch HOME and RUNNER_TEMP, no deploy token unless given),
# and hold it to its exit code and words.
expect() {
  local name="$1" code="$2" words="$3"
  shift 3
  local run="$work/runs/$passed"
  mkdir -p "$run/home" "$run/temp"
  local out status=0
  out="$(cd "$run" && env -i PATH="$PATH" HOME="$run/home" RUNNER_TEMP="$run/temp" \
    CURL_CA_BUNDLE="$work/cert.pem" "$@" bash "$script" 2>&1)" || status=$?
  last_out="$out"
  if [[ "$status" != "$code" || "$out" != *"$words"* ]]; then
    printf 'FAIL %s\n  wanted exit %s saying "%s"; got exit %s:\n%s\n' "$name" "$code" "$words" "$status" "$out" >&2
    exit 1
  fi
  passed=$((passed + 1))
  echo "ok $passed: $name"
}
# publish.sh says "Zeron <version> (<arch>)" once it has run the command
# line it downloaded: the last case must not have run one.
ran="Zeron $version ($arch)"
never_ran() {
  if [[ "$last_out" == *"$ran"* ]]; then
    echo "FAIL $1: the downloaded command line ran" >&2
    exit 1
  fi
}

ok="$work/plugins/ok"
expect "no key: checks a plugin that meets the rules" 0 "focus-ci 1.0.0 meets the store's rules." \
  ZERON_RELEASES_URL="$http/good" ZERON_PLUGIN_PATH="$ok"
[[ "$last_out" == *"$ran"* && "$last_out" == *"checking only"* ]] || { echo "FAIL the check didn't say what ran: $last_out" >&2; exit 1; }
expect "no key: a plugin that doesn't is exit 1 with the rule's word" 1 'id_reserved (id): "all"' \
  ZERON_RELEASES_URL="$http/good" ZERON_PLUGIN_PATH="$work/plugins/reserved"
expect "a pinned version reads manifest-<version>.json" 0 "meets the store's rules." \
  ZERON_RELEASES_URL="$http/good" ZERON_VERSION="$version" ZERON_PLUGIN_PATH="$ok"
expect "over https (the certificate made for this run)" 0 "meets the store's rules." \
  ZERON_RELEASES_URL="$https/good" ZERON_PLUGIN_PATH="$ok"
expect "a tarball that isn't the manifest's sha256 is never run" 2 "isn't what Zeron's release manifest says (sha256)" \
  ZERON_RELEASES_URL="$http/wrong-sha" ZERON_PLUGIN_PATH="$ok"
never_ran "the wrong sha256"
expect "a manifest naming another file than this machine's" 2 "Zeron $version has no $file." \
  ZERON_RELEASES_URL="$http/other-file" ZERON_PLUGIN_PATH="$ok"
never_ran "another file"
expect "a tarball without zeron-engine" 2 "$file has no zeron-engine." \
  ZERON_RELEASES_URL="$http/hollow" ZERON_PLUGIN_PATH="$ok"
expect "a pinned version whose manifest names another is refused" 2 "Zeron's release manifest for $version names version 9.9.9: not the version pinned." \
  ZERON_RELEASES_URL="$http/pin-moved" ZERON_VERSION="$version" ZERON_PLUGIN_PATH="$ok"
never_ran "the pin moved"
expect "a pinned version and its sha256 run that command line" 0 "meets the store's rules." \
  ZERON_RELEASES_URL="$http/good" ZERON_VERSION="$version" ZERON_SHA256="$good_sha" ZERON_PLUGIN_PATH="$ok"
expect "the sha256 input in capitals is the same sha256" 0 "meets the store's rules." \
  ZERON_RELEASES_URL="$http/good" ZERON_VERSION="$version" ZERON_SHA256="$(tr 'a-f' 'A-F' <<<"$good_sha")" ZERON_PLUGIN_PATH="$ok"
expect "a tarball that isn't the pinned sha256 is never run (the manifest agreeing with it)" 2 "$file isn't the one pinned (the sha256 input)." \
  ZERON_RELEASES_URL="$http/good" ZERON_VERSION="$version" ZERON_SHA256="$hollow_sha" ZERON_PLUGIN_PATH="$ok"
never_ran "another sha256 pinned"
expect "a sha256 input that isn't one" 2 "sha256 is the command line tarball's sha256: 64 hex digits." \
  ZERON_RELEASES_URL="$http/good" ZERON_SHA256="abc" ZERON_PLUGIN_PATH="$ok"
expect "a manifest that names no version" 2 "Zeron's release manifest names no version." \
  ZERON_RELEASES_URL="$http/no-version" ZERON_PLUGIN_PATH="$ok"
expect "releases over plain http anywhere but this machine" 2 "Zeron's releases are fetched over https." \
  ZERON_RELEASES_URL="http://example.com/releases" ZERON_PLUGIN_PATH="$ok"
expect "an https manifest redirected to http isn't followed" 2 "Couldn't read Zeron's release manifest" \
  ZERON_RELEASES_URL="$https/to-http" ZERON_PLUGIN_PATH="$ok"
expect "an https tarball redirected to http isn't followed" 2 "Couldn't download $file." \
  ZERON_RELEASES_URL="$https/tarball-to-http" ZERON_PLUGIN_PATH="$ok"
never_ran "the tarball redirected to http"
touch_target="$work/runs/injected"
expect "a version that isn't one is refused, never run as a command" 2 "zeron-version is latest or a version like 0.4.0." \
  ZERON_RELEASES_URL="$http/good" ZERON_VERSION="1.0;touch $touch_target" ZERON_PLUGIN_PATH="$ok"
[[ ! -e "$touch_target" ]] || { echo "FAIL the version ran as a command" >&2; exit 1; }
expect "an empty version is refused" 2 "zeron-version is latest or a version like 0.4.0." \
  ZERON_RELEASES_URL="$http/good" ZERON_VERSION=" " ZERON_PLUGIN_PATH="$ok"

# With a deploy token (made for the run as the store makes one: zdt_, 32
# bytes in base62, a CRC-32 checksum; the store is a closed port here, so
# nothing could be sent anyway).
token="$(python3 -c '
import zlib
A = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"
def b62(n, w):
    s = ""
    while n:
        s = A[n % 62] + s
        n //= 62
    return s.rjust(w, "0")
before = "zdt_" + b62(int.from_bytes(bytes(range(32)), "big"), 43)
print(before + b62(zlib.crc32(before.encode()), 6))')"
offline="http://127.0.0.1:9"
expect "a dry run with a token checks and packs, and sends nothing" 0 "Not sent (--dry-run)." \
  ZERON_RELEASES_URL="$http/good" ZERON_DEPLOY_TOKEN="$token" ZERON_DRY_RUN=true ZERON_EDGE_URL="$offline" ZERON_PLUGIN_PATH="$ok"
expect "a mistyped token stops the step before anything is sent" 2 "damaged: ZERON_DEPLOY_TOKEN" \
  ZERON_RELEASES_URL="$http/good" ZERON_DEPLOY_TOKEN="${token%?}0" ZERON_EDGE_URL="$offline" ZERON_PLUGIN_PATH="$ok"
for word in yes True 1 " true"; do
  expect "dry-run \"$word\" is refused, never taken as false" 2 "dry-run is true or false." \
    ZERON_RELEASES_URL="$http/good" ZERON_DEPLOY_TOKEN="$token" ZERON_DRY_RUN="$word" ZERON_EDGE_URL="$offline" ZERON_PLUGIN_PATH="$ok"
  never_ran "dry-run $word"
done
expect "a path that reads like an option is a path" 2 "--help: there's no plugin folder there" \
  ZERON_RELEASES_URL="$http/good" ZERON_PLUGIN_PATH="--help"
expect "and so with a token" 2 "--dry-run: there's no plugin folder there" \
  ZERON_RELEASES_URL="$http/good" ZERON_DEPLOY_TOKEN="$token" ZERON_EDGE_URL="$offline" ZERON_PLUGIN_PATH="--dry-run"

echo "publish.sh: $passed of $passed cases as promised"
