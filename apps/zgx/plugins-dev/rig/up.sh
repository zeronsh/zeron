#!/bin/sh
# Starts the rig's three staging engines (A1 27891, A2 27892 = test user U1;
# B 27893 = test user U2), signs in any that aren't, and prints each one's
# port, data dir, device id, account and plugin signing key, then checks
# them. Idempotent. State: $ZERON_RIG_STATE (see lib.mjs).
exec node "$(dirname "$0")/rig.mjs" up "$@"
