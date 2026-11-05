-- Samples for the Syntax highlighting page: a short, real-looking piece of
-- each language, with the constructs that are hard to colour (nested and
-- multi-line comments, raw strings, interpolation, heredocs, JSX).

local S: { [string]: string } = {}

S.rust = [==[
//! Relays a device's frames to its peers.
use std::{collections::HashMap, sync::Arc};

/* Block comments /* nest */ in Rust. */
#[derive(Debug, Clone, PartialEq)]
pub struct Relay<'a> {
    name: &'a str,
    peers: HashMap<u64, Arc<Peer>>,
}

const MAX_FRAME: usize = 16 * 1024;

impl<'a> Relay<'a> {
    pub async fn send(&self, id: u64, frame: &[u8]) -> Result<(), Error> {
        let peer = self.peers.get(&id).ok_or(Error::Gone(id))?;
        if frame.len() > MAX_FRAME {
            return Err(Error::TooLarge { len: frame.len() });
        }
        let banner = r#"a "raw" string, \n stays as written"#;
        println!("sent {} bytes to {id:>8}\t{:?}", frame.len(), b'\n');
        peer.write(frame, 0x1F_u8, 1.5e-3).await
    }
}
]==]

S.typescript = [==[
// Presence for a workspace's devices.
import { Room, type Device } from "./room";

export interface Presence<T extends Device = Device> {
  readonly id: string;
  devices: Map<string, T>;
  lastSeen?: number;
}

/**
 * Whether a device is online: seen within `window` ms.
 */
export async function online(room: Room, id: string, window = 30_000): Promise<boolean> {
  const seen = await room.lastSeen(id);
  const pattern = /^dev-[a-f0-9]{8}$/i;
  if (!pattern.test(id) || seen === undefined) return false;
  console.log(`device ${id} seen ${Date.now() - seen}ms ago, ${
    seen > 0 ? "recently" : "never"
  }`);
  return Date.now() - seen < window;
}

@sealed
class Badge {
  constructor(private presence: Presence) {}
  get label(): string { return this.presence.lastSeen ? "Online" : "Away"; }
}
]==]

S.tsx = [==[
import { useState } from "react";

type Props = { title: string; count?: number };

export function Counter({ title, count = 0 }: Props) {
  const [n, setN] = useState(count);
  return (
    <section className="counter" data-count={n}>
      <h2>{title}</h2>
      <Button
        variant="primary"
        onClick={() => {
          setN(n + 1);
        }}
      >
        Clicked {n} {n === 1 ? "time" : "times"}
      </Button>
      {n > 10 && <p className="warn">That's a lot of clicks.</p>}
    </section>
  );
}
]==]

S.javascript = [==[
// Debounce a function: call it once things go quiet.
export function debounce(fn, ms = 120) {
  let timer = null;
  return function (...args) {
    clearTimeout(timer);
    timer = setTimeout(() => fn.apply(this, args), ms);
  };
}

const ratio = total / count / 2; // a division, not a regex
const words = text.split(/\s+/g).filter(Boolean);
const greeting = `Hello, ${user?.name ?? "friend"}!`;
module.exports = { debounce, ratio, words, greeting, empty: null, ok: true };
]==]

S.luau = [==[
--!strict
-- A queue with a fixed capacity.
local Queue = {}
Queue.__index = Queue

export type Queue<T> = typeof(setmetatable({} :: { items: { T }, head: number }, Queue))

--[[ Long comments run
     over lines. ]]
function Queue.new<T>(capacity: number): Queue<T>
	return setmetatable({ items = table.create(capacity), head = 1 }, Queue)
end

function Queue:push(item: any): boolean
	if #self.items >= 0x100 then
		return false
	end
	local text = [=[a long string with ]] inside]=]
	print(`pushed {item} at {self.head + 1}`, "tab\t", 3.5e2)
	table.insert(self.items, item)
	return true
end

return Queue
]==]

S.python = [==[
#!/usr/bin/env python3
"""Summarise a session's tool calls."""
from dataclasses import dataclass, field
import re

PATTERN = re.compile(r"^tool:(\w+)\s*$")

@dataclass(frozen=True)
class Call:
    name: str
    took_ms: float = 0.0
    tags: list[str] = field(default_factory=list)

    def label(self) -> str:
        return f"{self.name} ({self.took_ms:.1f} ms, {{literal}})"

def summarise(calls: list[Call], *, top: int = 5) -> dict[str, float]:
    totals: dict[str, float] = {}
    for call in calls:
        totals[call.name] = totals.get(call.name, 0) + call.took_ms
    if not totals or top <= 0:
        return {}
    return dict(sorted(totals.items(), key=lambda kv: -kv[1])[:top])
]==]

S.go = [==[
// Package relay forwards frames between peers.
package relay

import (
	"errors"
	"fmt"
	"time"
)

var ErrGone = errors.New("peer gone")

type Peer struct {
	ID       uint64
	LastSeen time.Time
}

func (p *Peer) Send(frame []byte) error {
	if len(frame) == 0 {
		return nil
	}
	query := `SELECT id
FROM peers WHERE id = $1`
	fmt.Printf("peer %d: %q (%c)\n", p.ID, query, 'x')
	return ErrGone
}
]==]

S.swift = [==[
import SwiftUI

/// A device's presence badge.
struct PresenceBadge: View {
    @State private var online = false
    let name: String

    var body: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(online ? Color.green : .gray)
                .frame(width: 8, height: 8)
            Text("\(name) is \(online ? "online" : "away")")
        }
        .task { await refresh() }
    }

    private func refresh() async {
        let banner = """
        Multi-line strings
        keep "quotes" and \(name)
        """
        online = !banner.isEmpty
    }
}
]==]

S.kotlin = [==[
package sh.zeron.android

import kotlinx.coroutines.flow.Flow

/* Kotlin's block comments /* nest */ too. */
data class Session(val id: String, val title: String, val unread: Int = 0)

@Composable
fun SessionRow(session: Session, onOpen: (String) -> Unit) {
    val label = "${session.title} · $unread new"
    val raw = """
        Raw: no \n escapes, but ${session.id} still interpolates
    """.trimIndent()
    when {
        session.unread > 99 -> Badge("99+")
        else -> Text(label)
    }
}
]==]

S.c = [==[
#include <stdio.h>
#include "ring.h"

#define RING_CAP 1024

/* A fixed ring of bytes. */
typedef struct {
    unsigned char data[RING_CAP];
    size_t head, len;
} Ring;

static int ring_push(Ring *r, unsigned char b) {
    if (r->len == RING_CAP) return -1;
    r->data[(r->head + r->len++) % RING_CAP] = b;
    printf("pushed %c (0x%02x)\n", b, b);
    return 0;
}
]==]

S.cpp = [==[
#include <memory>
#include <string>

namespace zeron {

template <typename T>
class Cache final {
public:
    explicit Cache(std::size_t cap) : cap_(cap) {}
    [[nodiscard]] const T* get(const std::string& key) const noexcept;

private:
    std::size_t cap_;
};

auto banner = R"html(<p class="x">raw "C++" string</p>)html";
constexpr auto kLimit = 4'096ull;

}  // namespace zeron
]==]

S.java = [==[
package sh.zeron.relay;

import java.util.Map;

/** Forwards frames to peers. */
public final class Relay implements AutoCloseable {
    private static final int MAX = 16 * 1024;
    private final Map<Long, Peer> peers;

    @Override
    public void close() {
        String block = """
            text blocks run
            over lines
            """;
        System.out.printf("closing %d peers%n", peers.size());
    }
}
]==]

S.csharp = [==[
using System.Text.Json;

namespace Zeron.Sync;

#nullable enable
public sealed record Device(string Id, string Name, bool Online = false);

public static class Presence
{
    public static string Label(Device d) => d.Online ? $"{d.Name} online" : "Away";
}
]==]

S.ruby = [==[
# A tiny presence store.
class Presence
  attr_reader :devices

  def initialize
    @devices = {}
  end

  def online?(id, window: 30)
    seen = @devices[id]
    return false if seen.nil?
    puts "device #{id} seen #{Time.now - seen}s ago"
    Time.now - seen < window
  end
end
]==]

S.json = [==[
{
  "name": "zeron",
  "version": "0.2.129",
  "private": true,
  "engines": { "node": ">=20" },
  "workspaces": ["apps/*", "crates/*"],
  "ratio": 1.5e-3,
  "escaped": "tab\tquote\" unicode é",
  "nothing": null
}
]==]

S.toml = [==[
# Cargo manifest
[package]
name = "zeron-zgx"
version = "0.1.0"
edition = "2021"
published = 1979-05-27T07:32:00Z

[dependencies]
zgx = { git = "https://github.com/zeronsh/zgx", branch = "main" }
serde = { version = "1", features = ["derive"] }

[[bin]]
name = "zeron-zgx"
description = """
A multi-line
basic string."""
path = 'src\main.rs'
]==]

S.yaml = [==[
# CI for the desktop app
name: release
on:
  push:
    tags: ["v*"]
env:
  CARGO_TERM_COLOR: always
  RETRIES: 3
defaults: &defaults
  runs-on: macos-15
  timeout-minutes: 25
jobs:
  build:
    <<: *defaults
    steps:
      - uses: actions/checkout@v4
      - name: Build
        run: |
          cargo build --release
          echo "done: ${{ github.ref }}"
      - name: Notify
        if: success()
        with: { channel: "#releases", quiet: false }
]==]

S.markdown = [==[
# Release notes

Zeron **0.2.129** reports itself as `CLAUDE_CODE_ENTRYPOINT` for every
Claude it starts. See [the changelog](https://zeron.sh/changelog) and
the _plan mode_ notes.

## What changed

- [x] Access modes for every agent
- [ ] Folded bubbles fade out
1. Long chats scroll smoothly again

> Upgrade with the in-app updater; no restart needed.

```rust
fn main() {
    println!("fenced code is coloured as its language");
}
```

| Platform | Status |
| -------- | ------ |
| macOS    | ~~beta~~ shipped |
]==]

S.shell = [==[
#!/usr/bin/env bash
# Deploy the engine to a devbox.
set -euo pipefail

HOST="${1:-dev}"
VERSION=$(git describe --tags --always)
export RUST_LOG=info

if [[ -z "$HOST" ]]; then
  echo "usage: $0 <host>" >&2
  exit 1
fi

cargo build --release -p zeron-engine 2>/dev/null | tee build.log
scp target/release/zeron "$HOST":~/.local/bin/zeron
ssh "$HOST" bash <<-EOF
	systemctl --user restart zeron
	echo "deployed $VERSION"
EOF
for f in logs/*.log; do gzip -9 "$f"; done
]==]

S.html = [==[
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>Zeron &mdash; agents everywhere</title>
  <style>
    body { font: 15px/1.6 "Geist", sans-serif; color: #1b1b1d; }
  </style>
</head>
<body>
  <!-- The hero, above the fold. -->
  <main id="hero" class="hero dark" data-step=1>
    <h1>Every agent, every device</h1>
    <button type="button" disabled>Download</button>
  </main>
  <script type="module">
    const hero = document.querySelector("#hero");
    hero.addEventListener("click", () => console.log(`clicked ${Date.now()}`));
  </script>
</body>
</html>
]==]

S.css = [==[
/* The composer pill. */
@import url("tokens.css");

:root {
  --composer-radius: 26px;
}

.composer > .pill:hover,
#composer[data-open="true"]::after {
  border-radius: var(--composer-radius);
  background: rgba(8, 8, 10, 0.35) !important;
  box-shadow: 0 12px 32px 0 #00000066;
  transition: opacity 120ms ease-out;
  width: calc(100% - 2rem);
}

@media (prefers-color-scheme: light) {
  .composer { color: #1b1b1d; }
}
]==]

S.sql = [==[
-- Sessions with unread turns, newest first.
SELECT s.id, s.title, COUNT(t.id) AS unread
FROM sessions s
LEFT JOIN turns t ON t.session_id = s.id AND t.read_at IS NULL
WHERE s.archived = false
  AND s.title ILIKE '%relay%'
  AND s.note <> 'it''s quoted'
GROUP BY s.id
HAVING COUNT(t.id) > 0
ORDER BY MAX(t.created_at) DESC
LIMIT 50;

/* Block comments
   run over lines. */
CREATE INDEX IF NOT EXISTS turns_unread ON turns (session_id) WHERE read_at IS NULL;
]==]

S.wgsl = [==[
// The edge fade: alpha ramps in over each band.
struct Uniforms {
    size: vec2<f32>,
    band: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

@fragment
fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4<f32> {
    let p = uv * u.size;
    let edge = min(min(p.x, p.y), min(u.size.x - p.x, u.size.y - p.y));
    let a = clamp(edge / u.band, 0.0, 1.0);
    return vec4<f32>(1.0, 1.0, 1.0, a * a); /* ramp squared */
}
]==]

S.diff = [==[
diff --git a/crates/ui/src/syntax_cache.rs b/crates/ui/src/syntax_cache.rs
index 3f1c2a9..8b7afb3 100644
--- a/crates/ui/src/syntax_cache.rs
+++ b/crates/ui/src/syntax_cache.rs
@@ -14,7 +14,7 @@ use zeron_syntax::{HighlightedDocument, LanguageId};
 pub const QUERY_GENERATION: u32 = 1;
-const MAX_DOCUMENTS: usize = 96;
+const MAX_DOCUMENTS: usize = 128;
 const MAX_RETAINED_BYTES: usize = 24 * 1024 * 1024;
\ No newline at end of file
]==]

S.jsx = S.tsx
S.lua = S.luau

return S
