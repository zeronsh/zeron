#!/bin/zsh
# A hover lane mounted while a measured list scrolls, then unmounted before
# it stops, stays drawn where it last stood. Run from apps/zgx with a built
# release app. Prints "DIFF …" (the stale pixels, in snapshot pixels) or
# "same". See ../ASK.md.
set -e
HERE=${0:A:h}
TMP=$(mktemp -d)
# Zeron's ui with the lane as it was before the app's workaround.
cp -R ui $TMP/ui
git show dd32e339:apps/zgx/ui/ds/transcript.luau > $TMP/ui/ds/transcript.luau
swiftc -O -o $TMP/pxdiff $HERE/pxdiff.swift
mkdir -p $TMP/eng $TMP/home
ZGX_CHECK=0 ZERON_ZGX_UI=$TMP/ui ZERON_ZGX_ENGINE_SPAWN=0 ZERON_ZGX_ENGINE_PORT=27897 \
	ZERON_ZGX_ENGINE_DATA=$TMP/eng HOME=$TMP/home \
	./target/release/zeron-zgx fixture=chat_fixture.luau --inspect >$TMP/host.log 2>&1 &
P=$!
trap "kill $P 2>/dev/null; rm -rf $TMP" EXIT
sleep 3
i() { zgx inspect --app $P "$@"; }
snap() { i snapshot '#transcript' | grep -o '/var/[^ ]*png'; }
i command go_s1 >/dev/null; sleep 1
# A short reply to a prompt sent from here: the room under it is empty.
i type 'role:textbox "Message"' "did u call any show image tool" >/dev/null
i key Enter >/dev/null; sleep 1
i command engine_copy >/dev/null; i command start_reply >/dev/null
i command stream_words >/dev/null; i command end_reply >/dev/null; sleep 0.5
# Pull the list up 400 points (the reply's text near y 560), put the
# pointer above the reply, then fling back: the reply's text and lane pass
# under the still pointer, its lane mounts and unmounts mid-fling.
i hover 900,620 >/dev/null; i replay '#transcript' $HERE/up400.tape >/dev/null; sleep 0.4
i hover 400,345 >/dev/null; sleep 0.05
i replay '#transcript' $HERE/down400.tape >/dev/null; sleep 0.4
cp $(snap) $TMP/drawn.png
# The same frame painted whole.
i switch partial_repaint off >/dev/null; i zoom in >/dev/null; sleep 0.25; i zoom reset >/dev/null; sleep 0.35
cp $(snap) $TMP/whole.png
i switch partial_repaint on >/dev/null
T=8 $TMP/pxdiff $TMP/drawn.png $TMP/whole.png
