#!/bin/zsh
# The end-to-end smoke check: the release app, driven through `zgx inspect`,
# against an engine of its own (port 27898, its own data and home) whose
# agent is a fake ACP agent (scripts/smoke_agent.py, as Grok). One walk through the core flows, a line per step with its time.
#
#   scripts/smoke.sh            run against target/release/zeron-zgx as built, clean up
#   scripts/smoke.sh --build    rebuild it first if src/ is newer and nothing runs from it
#   scripts/smoke.sh --keep     leave the app, the engine and the run folder up afterwards
#   scripts/smoke.sh --explore  only launch (app + engine) and print how to reach them
#   scripts/smoke.sh --step x   run the steps up to and including x
#
# Isolated: never the production engine (27654, ~/.comet-native) or the
# staging one (27871, ~/.zeron-staging). The app runs on a frozen copy of
# ui/ (ZERON_ZGX_UI), so other saves under ui/ can't reload it mid-run, in a
# small corner window that keeps no frame (ZERON_ZGX_FRAME), as a renamed
# binary (macOS keys a window's saved frame by the process's name).
#
# Exits 0 when every step passes.

set -u
ZGX=${0:A:h:h}
ROOT=${ZGX:h:h}
PORT=27898
KEEP=0
BUILD=0
EXPLORE=0
UNTIL=""
while (( $# )); do
	case $1 in
		--keep) KEEP=1 ;;
		--build) BUILD=1 ;;
		--explore) EXPLORE=1; KEEP=1 ;;
		--step) UNTIL=$2; shift ;;
		*) print -u2 "smoke: unknown argument $1"; exit 2 ;;
	esac
	shift
done

say() { print -r -- "smoke: $*" }
die() { print -u2 -r -- "smoke: $*"; exit 1 }

# ---- the binaries -----------------------------------------------------------
APP=$ZGX/target/release/zeron-zgx
ENGINE=$ROOT/target/debug/zeron-engine
# (scripts/smoke_agent.py: fake-acp.sh's happy turn, on every prompt, in a
# process that lives across turns as a real agent's does.)
FAKE=$ZGX/scripts/smoke_agent.py
[[ -x $FAKE ]] || die "no fake agent at $FAKE"
# The shared release binary is never rebuilt under a running instance (an
# in-place rebuild invalidates its signature and macOS kills every window
# running it): the run uses a clone of it as it is, and says when the source
# is newer. `--build` rebuilds only when nothing runs from it.
STALE=0
[[ -x $APP ]] || die "no app at $APP: build it once (cargo build --release --bin zeron-zgx)"
[[ -n $(find $ZGX/src $ZGX/Cargo.toml $ZGX/Cargo.lock -newer $APP -print -quit 2>/dev/null) ]] && STALE=1
if (( STALE && BUILD )); then
	if pgrep -f "^$APP( |\$)" >/dev/null 2>&1; then
		die "--build: target/release/zeron-zgx is running (pids $(pgrep -f "^$APP( |\$)" | tr '\n' ' ')); not rebuilt under it"
	fi
	say "building the app (its source is newer than target/release/zeron-zgx)"
	(cd $ZGX && cargo build --release --bin zeron-zgx --quiet) || die "the app didn't build"
	STALE=0
fi
(( STALE )) && say "note: src/ is newer than target/release/zeron-zgx; this run is of the binary as built (ui/ is current). --build rebuilds it when nothing runs from it."
if [[ ! -x $ENGINE ]]; then
	say "building the engine (none at target/debug/zeron-engine)"
	(cd $ROOT && cargo build -p zeron-engine --quiet) || die "the engine didn't build"
fi

# ---- the run's folder, and leaving nothing behind -------------------------------
if lsof -nP -iTCP:$PORT -sTCP:LISTEN >/dev/null 2>&1; then
	OLD=$(lsof -nP -t -iTCP:$PORT -sTCP:LISTEN | head -1)
	if ps -o command= -p $OLD | grep -q "zgx-smoke"; then
		say "stopping a smoke engine left from an earlier run ($OLD)"
		kill $OLD; sleep 1
	else
		die "port $PORT is taken by $(ps -o command= -p $OLD | cut -c1-80): not ours, so not touched"
	fi
fi
RUN=$(mktemp -d "${TMPDIR:-/tmp}/zgx-smoke.XXXXXX")
APP_PID=""
ENGINE_PID=""
finish() {
	[[ -n $APP_PID ]] && kill $APP_PID 2>/dev/null
	[[ -n $ENGINE_PID ]] && kill $ENGINE_PID 2>/dev/null
	sleep 0.5
	[[ -n $APP_PID ]] && kill -9 $APP_PID 2>/dev/null
	[[ -n $ENGINE_PID ]] && kill -9 $ENGINE_PID 2>/dev/null
	rm -rf $RUN
}
(( KEEP )) || trap finish EXIT INT TERM

mkdir -p $RUN/bin $RUN/home/smoke-project $RUN/apphome $RUN/engine
# (Clones: no space taken.)
cp -c $APP $RUN/bin/zgx-smoke
cp -cR $ZGX/ui $RUN/ui
# The scratch project: a git repository with a few files to open and attach.
(
	cd $RUN/home/smoke-project
	git init -q -b main
	printf '# Smoke project\n\nA scratch project for scripts/smoke.sh.\n' > README.md
	printf 'first line\nsecond line\n' > notes.txt
	cp $ZGX/ui/assets/favicons/zeron.png picture.png
	git add -A && git -c user.name=smoke -c user.email=smoke@example.invalid commit -qm "smoke project"
)

# Only what we set: none of this shell's Zeron settings (they name the
# engine this shell runs under).
CLEAN=(env -i PATH=$PATH TMPDIR=${TMPDIR:-/tmp} LANG=${LANG:-en_US.UTF-8} USER=$USER LOGNAME=${LOGNAME:-$USER} SHELL=/bin/zsh TERM=xterm-256color)

# ---- the engine -----------------------------------------------------------------
# (`zgx-smoke` in its argv[0], so a later run knows it as ours.)
ln -s $ENGINE $RUN/bin/zgx-smoke-engine
$CLEAN HOME=$RUN/home RUST_LOG=${SMOKE_ENGINE_LOG:-info} ZERON_DATA_DIR=$RUN/engine ZERON_IPC_PORT=$PORT GROK_EXECUTABLE=$FAKE \
	$RUN/bin/zgx-smoke-engine > $RUN/engine.log 2>&1 &
ENGINE_PID=$!
for _ in {1..100}; do
	[[ -s $RUN/engine/ipc-token ]] && lsof -nP -iTCP:$PORT -sTCP:LISTEN >/dev/null 2>&1 && break
	sleep 0.1
done
[[ -s $RUN/engine/ipc-token ]] || { tail -5 $RUN/engine.log; die "the engine didn't start"; }

# ---- the app ----------------------------------------------------------------------
# A small window in the display's top-right corner.
GEOM=$(osascript -l JavaScript -e 'ObjC.import("AppKit"); var s=$.NSScreen.mainScreen; [s.backingScaleFactor, s.frame.size.width].join(",")' 2>/dev/null)
SCALE=${GEOM%%,*}; SCREEN_W=${GEOM#*,}
W=1040; H=700
FRAME="$(( (SCREEN_W - W - 12) * SCALE )),$(( 40 * SCALE )),$(( W * SCALE )),$(( H * SCALE )),$SCALE"
(
	cd $RUN
	exec $CLEAN HOME=$RUN/apphome ZGX_CHECK=0 ZGX_RENDER_WHEN_COVERED=1 \
		ZERON_ZGX_ENGINE_SPAWN=0 ZERON_ZGX_ENGINE_PORT=$PORT ZERON_ZGX_ENGINE_DATA=$RUN/engine \
		ZERON_ZGX_UI=$RUN/ui ZERON_ZGX_FRAME=$FRAME \
		$RUN/bin/zgx-smoke main --inspect > $RUN/app.log 2>&1
) &
APP_PID=$!
for _ in {1..100}; do
	(cd /tmp && zgx inspect --app $APP_PID state >/dev/null 2>&1) && break
	sleep 0.2
done

if (( EXPLORE )); then
	say "app $APP_PID, engine $ENGINE_PID on $PORT, run folder $RUN"
	say "  zgx inspect --app $APP_PID tree"
	say "  python3 -I $ZGX/scripts/smoke_rpc.py $PORT $RUN/engine <Method> '<json>'"
	say "  stop: kill $APP_PID $ENGINE_PID; rm -rf $RUN"
	exit 0
fi

python3 -I $ZGX/scripts/smoke.py --app $APP_PID --port $PORT --run $RUN --ui $RUN/ui ${UNTIL:+--until $UNTIL}
STATUS=$?
(( KEEP )) && say "kept: app $APP_PID, engine $ENGINE_PID, $RUN"
exit $STATUS
