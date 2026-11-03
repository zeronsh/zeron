#!/bin/sh
# Stops the rig's engines only: the PIDs it tracks under $ZERON_RIG_STATE,
# and only while they are still zeron-engine processes.
exec node "$(dirname "$0")/rig.mjs" down "$@"
