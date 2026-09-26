#!/bin/sh
# fake devin acp server for zeron-harness sign-in tests.
#
# advertises only `devin-browser` (devin has no default method), carries an
# unrelated url in its handshake (the sign-in must not announce it), prints
# its sign-in page to stderr, and saves the login under $XDG_DATA_HOME the
# way `devin acp` does.

emit() { printf '%s\n' "$1"; }
rid() { printf '%s' "$1" | sed 's/.*"id":\([0-9]*\).*/\1/'; }
has() { case "$1" in *"$2"*) return 0 ;; *) return 1 ;; esac; }

while read -r line; do
  has "$line" '"id":' || continue
  id=$(rid "$line")
  if has "$line" '"method":"initialize"'; then
    emit "{\"id\":$id,\"result\":{\"protocolVersion\":1,\"agentInfo\":{\"name\":\"devin\",\"website\":\"https://devin.ai/\"},\"authMethods\":[{\"id\":\"devin-browser\",\"name\":\"Log in with browser\"}]}}"
  elif has "$line" '"method":"authenticate"'; then
    has "$line" '"methodId":"devin-browser"' || exit 1
    [ -n "$XDG_DATA_HOME" ] || exit 2
    printf 'Opening https://app.devin.ai/auth/cli/continue?redirect_uri=http%%3A%%2F%%2F127.0.0.1%%3A45678%%2Fcallback&state=s\n' >&2
    sleep 0.2
    mkdir -p "$XDG_DATA_HOME/devin"
    printf 'windsurf_api_key = "fake"\n' > "$XDG_DATA_HOME/devin/credentials.toml"
    emit "{\"id\":$id,\"result\":{}}"
  else
    emit "{\"id\":$id,\"result\":{}}"
  fi
done
