#!/usr/bin/env bash
# Zeron (apps/zgx) against the staging edge: a development build, which signs
# in through staging (https://staging.zeron.sh, WorkOS's test environment)
# and starts its own engine for it, on port 27871 with its data in
# ~/.zeron-staging (apps/zgx/src/engine/manager.rs). It runs beside an
# installed Zeron, whose engine (port 27654, ~/.zeron) it never uses.
#
#   scripts/staging-app.sh            # build the engine and the app, then open it
#   scripts/staging-app.sh gallery    # any of the app's launch words (apps/zgx/README.md)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# A shell inside Zeron carries the installed app's settings (its edge, data
# dir, port, sign-in client): the build's staging defaults hold only without them.
unset ZERON_APP ZERON_ORIGIN ZERON_MANAGED_ENGINE ZERON_DATA_DIR ZERON_IPC_PORT \
  ZERON_EDGE_URL ZERON_WORKOS_CLIENT_ID ZERON_CALLBACK_PORT ZERON_IPC_TOKEN ZERON_IPC_TOKEN_FILE

# The engine it starts, built as releases build it (no Luau asserts).
(cd "$ROOT" && LUAU_CXXFLAGS=-DNDEBUG cargo build --release -p zeron --bin zeron-engine)
cd "$ROOT/apps/zgx"
ZERON_ZGX_ENGINE_BIN="$ROOT/target/release/zeron-engine" exec cargo run --release -- "${@:-main}"
