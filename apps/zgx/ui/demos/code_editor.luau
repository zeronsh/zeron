-- The gallery's Code editor page: a file in each of a few languages, and
-- one of 20,000 lines to feel its speed, in the editor; beside it the same
-- file read-only, as the Files pane shows one (edits on the left show on
-- the right as they are typed). ⌘F finds, ⌃G goes to a line, ⌘S saves. The
-- gutter folds the code and, under its +, starts a comment on a line; the
-- Rust and Luau files already have a note and a thread standing in them. A
-- page is { id, title, about, demo }.

local ds = require("@ds")
local CodeEditor = require("@ds/code_editor")
local Segmented = require("@ds/segmented")
local Inline = require("@ds/inline_block")
local Comment = require("@ds/comment")
local people = require("@ds/people")

local LUAU = [[
--!strict
-- A tiny event bus: subscribers by topic, each told what was published.
local Bus = {}
Bus.__index = Bus

export type Handler = (payload: any) -> ()

function Bus.new()
	return setmetatable({ topics = {} :: { [string]: { Handler } } }, Bus)
end

function Bus:subscribe(topic: string, handler: Handler): () -> ()
	local list = self.topics[topic]
	if not list then
		list = {}
		self.topics[topic] = list
	end
	table.insert(list, handler)
	return function()
		local i = table.find(list, handler)
		if i then
			table.remove(list, i)
		end
	end
end

function Bus:publish(topic: string, payload: any)
	for _, handler in self.topics[topic] or {} do
		local ok, err = pcall(handler, payload)
		if not ok then
			warn(`handler for {topic} failed: {err}`)
		end
	end
end

return Bus
]]

local RUST = [[
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// A cache that forgets entries older than its time to live.
pub struct TtlCache<V> {
    ttl: Duration,
    entries: HashMap<String, (Instant, V)>,
}

impl<V: Clone> TtlCache<V> {
    pub fn new(ttl: Duration) -> Self {
        Self { ttl, entries: HashMap::new() }
    }

    pub fn get(&mut self, key: &str) -> Option<V> {
        let fresh = match self.entries.get(key) {
            Some((at, _)) => at.elapsed() < self.ttl,
            None => return None,
        };
        if !fresh {
            self.entries.remove(key);
            return None;
        }
        self.entries.get(key).map(|(_, v)| v.clone())
    }

    pub fn insert(&mut self, key: impl Into<String>, value: V) {
        self.entries.insert(key.into(), (Instant::now(), value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forgets_old_entries() {
        let mut cache = TtlCache::new(Duration::from_millis(0));
        cache.insert("a", 1);
        assert_eq!(cache.get("a"), None);
    }
}
]]

local TYPESCRIPT = [[
import { useEffect, useState } from "react";

type Status = "idle" | "loading" | "ready" | "failed";

interface Session {
  id: string;
  title: string;
  updatedAt: number;
}

/** Sessions for a project, refreshed every `everyMs`. */
export function useSessions(projectId: string, everyMs = 30_000) {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [status, setStatus] = useState<Status>("idle");

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      setStatus("loading");
      try {
        const res = await fetch(`/api/projects/${projectId}/sessions`);
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const body: Session[] = await res.json();
        if (!cancelled) {
          setSessions(body.sort((a, b) => b.updatedAt - a.updatedAt));
          setStatus("ready");
        }
      } catch {
        if (!cancelled) setStatus("failed");
      }
    };
    load();
    const timer = setInterval(load, everyMs);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [projectId, everyMs]);

  return { sessions, status };
}
]]

local PYTHON = [[
"""Summarise a log file: requests by status, and the slowest paths."""
from collections import Counter
from dataclasses import dataclass
import re
import sys

LINE = re.compile(r'"(?P<method>\w+) (?P<path>\S+)" (?P<status>\d{3}) (?P<ms>\d+)ms')


@dataclass
class Request:
    method: str
    path: str
    status: int
    ms: int


def parse(lines):
    for line in lines:
        match = LINE.search(line)
        if match:
            yield Request(
                method=match["method"],
                path=match["path"],
                status=int(match["status"]),
                ms=int(match["ms"]),
            )


def main(path: str) -> None:
    with open(path) as f:
        requests = list(parse(f))
    by_status = Counter(r.status for r in requests)
    for status, n in sorted(by_status.items()):
        print(f"{status}: {n}")
    for r in sorted(requests, key=lambda r: r.ms, reverse=True)[:5]:
        print(f"{r.ms:>6}ms  {r.method} {r.path}")


if __name__ == "__main__":
    main(sys.argv[1])
]]

local JSON = [[
{
  "name": "zeron-zgx",
  "version": "0.2.129",
  "window": { "width": 1140, "height": 800, "frosted": true },
  "themes": ["zeron-dark", "zeron-light"],
  "fonts": {
    "sans": "Geist",
    "mono": "Geist Mono",
    "serif": "Instrument Serif"
  },
  "editor": {
    "tabSize": 4,
    "wrap": false,
    "lineNumbers": true,
    "rulers": [100]
  },
  "harnesses": [
    { "id": "claude", "models": ["opus", "sonnet", "haiku"] },
    { "id": "codex", "models": ["gpt-5", "gpt-5-mini"] }
  ]
}
]]

-- A Rust file of 20,000 lines: the stress case. Typing at its end, going to
-- its middle and finding across all of it should feel as a short file does.
local function huge(lines: number): string
	local body = {
		"/// Computes item %d's weight, scaled.",
		"pub fn compute_%d(input: &[u8], scale: f32) -> Result<Vec<u32>, Error> {",
		'    let name = "item %d"; // a comment',
		"    let total: u32 = input.iter().map(|b| *b as u32 * %d).sum();",
		"    if total > 0x%x {",
		"        return Err(Error::Overflow(total));",
		"    }",
		"    Ok(vec![total; scale as usize])",
		"}",
		"",
	}
	local out = table.create(lines)
	for i = 1, lines do
		local template = body[(i - 1) % #body + 1]
		out[i] = if string.find(template, "%%") then string.format(template, i) else template
	end
	return table.concat(out, "\n")
end

type Sample = { id: string, label: string, language: string?, text: string }
local SAMPLES: { Sample } = {
	{ id = "luau", label = "Luau", language = "luau", text = LUAU },
	{ id = "rust", label = "Rust", language = "rust", text = RUST },
	{ id = "ts", label = "TypeScript", language = "typescript", text = TYPESCRIPT },
	{ id = "py", label = "Python", language = "python", text = PYTHON },
	{ id = "json", label = "JSON", language = "json", text = JSON },
	{ id = "large", label = "10,000 lines", language = "rust", text = "" },
	{ id = "huge", label = "20,000 lines", language = "rust", text = "" },
}
local BY_ID: { [string]: Sample } = {}
for _, s in SAMPLES do
	BY_ID[s.id] = s
end

local picked = ui.state("code_editor_sample", "luau")
-- Each file's text, kept as it is edited (the big one made on first view).
local texts: { [string]: State<string> } = {}
local function text_of(id: string): State<string>
	if not texts[id] then
		local s = BY_ID[id]
		texts[id] = ui.state(`code_editor_text:{id}`, if id == "huge" then huge(20000) elseif id == "large" then huge(10000) else s.text)
	end
	return texts[id]
end
local wrap = ui.state("code_editor_wrap", false)
local reveal = ui.state("code_editor_reveal", nil :: number?)
local said = ui.state("code_editor_said", "")

-- What stands between each file's lines, and what is being written there.
type Item = { key: string, line: number, kind: string, comments: { Comment.Comment }?, from: string?, body: string? }
local ME: people.Who = { id = "mira", name = "Mira" }
local ADA: people.Who = { id = "ada", name = "Ada" }
local function seeded(id: string): { Item }
	if id == "rust" then
		return {
			{
				key = "note",
				line = 22,
				kind = "note",
				from = "Claude",
				body = "An entry past its time to live stays in the map until it is asked for. `insert` could sweep them, or a timer.",
			},
			{
				key = "thread",
				line = 34,
				kind = "thread",
				comments = {
					{ author = ADA, at = os.time() - 7200, body = "A zero TTL means nothing is ever fresh. Is that what this test wants?" },
					{ author = ME, at = os.time() - 1800, body = "It is: the entry is stale the moment it lands.", mine = true },
				},
			},
		}
	elseif id == "luau" then
		return {
			{
				key = "note",
				line = 25,
				kind = "note",
				from = "Claude",
				body = "`table.remove` in a handler shifts the list while `publish` walks it, so the next handler is skipped.",
			},
		}
	end
	return {}
end
local placed: { [string]: State<{ Item }> } = {}
local function blocks_of(id: string): State<{ Item }>
	local b = placed[id]
	if not b then
		b = ui.state(`code_editor_blocks_{id}`, seeded(id))
		placed[id] = b
	end
	return b
end
local drafts: { [string]: State<string> } = {}
local function draft_of(key: string): State<string>
	local d = drafts[key]
	if not d then
		d = ui.state(`code_editor_draft_{key}`, "")
		drafts[key] = d
	end
	return d
end
local function without(list: { Item }, key: string): { Item }
	local keep: { Item } = {}
	for _, b in list do
		if b.key ~= key then
			table.insert(keep, b)
		end
	end
	return keep
end

local function block_view(id: string, label: string): CodeEditor.BlockView
	local blocks = blocks_of(id)
	return function(b: CodeEditor.Block, c: CodeEditor.BlockContext): Node
		if b.kind == "note" then
			return Inline.Note {
				inset = c.inset,
				joined = c.joined,
				from = b.from,
				body = b.body,
				actions = {
					{
						label = "Dismiss",
						on_click = function()
							blocks:set(without(blocks:peek(), b.key))
						end,
					},
				},
			}
		elseif b.kind == "thread" then
			-- (A state's tables are frozen; a card keeps its props.)
			local comments: { Comment.Comment } = {}
			for _, comment in b.comments do
				table.insert(comments, table.clone(comment))
			end
			return Inline.Thread {
				inset = c.inset,
				joined = c.joined,
				comments = comments,
				draft = draft_of(`{id}-{b.key}`),
				me = ME,
			}
		end
		return Inline.Composer {
			inset = c.inset,
			joined = c.joined,
			value = draft_of(`{id}-{b.key}`),
			location = `{label}:{b.line}`,
			on_cancel = function()
				blocks:set(without(blocks:peek(), b.key))
			end,
			on_submit = function(text: string)
				local list = without(blocks:peek(), b.key)
				table.insert(list, {
					key = b.key,
					line = b.line,
					kind = "thread",
					comments = { { author = ME, at = os.time(), body = text, mine = true } },
				})
				blocks:set(list)
			end,
		}
	end
end

local function options(): { Segmented.Option }
	local out: { Segmented.Option } = {}
	for _, s in SAMPLES do
		table.insert(out, { value = s.id, label = s.label })
	end
	return out
end

-- A titled pane: what it shows above it.
local function pane(title: string, detail: Child, body: Node): Node
	return ui.column {
		grow = 1,
		basis = 0,
		min_w = 520,
		gap = "$space.2",
		ui.row {
			gap = "$space.2",
			items = "center",
			h = 28,
			ds.Text { title, style = "label", tone = "tertiary" },
			ui.div { grow = 1 },
			detail,
		},
		ds.Surface { kind = "sunken", p = 0, grow = 1, min_h = 0, w_full = true, overflow = "hidden", body },
	}
end

local function go_button(line: number): Node
	return ds.Button {
		tone = "ghost",
		size = "sm",
		on_click = function()
			reveal:set(line)
		end,
		`Line {line}`,
	}
end

return {
	id = "code-editor",
	title = "Code editor",
	fill = true,
	about = "Code on zgx's text engine: every line laid out only as it shows, so 20,000 lines type as fast as 20. Smart indent, brackets, comments, moving lines, find and replace, go to line. It folds, and takes comments and notes between its lines.",
	demo = function(): Node
		return ui.column {
			gap = "$space.4",
			w_full = true,
			h_full = true,
			ui.row {
				gap = "$space.3",
				items = "center",
				Segmented { value = picked, options = options(), label = "File" },
				ui.div { grow = 1 },
				ds.Text {
					function()
						return said:get()
					end,
					style = "caption",
					tone = "tertiary",
				},
			},
			ui.row {
				gap = "$space.4",
				w_full = true,
				grow = 1,
				min_h = 0,
				flex_wrap = true,
				items = "stretch",
				ui.presence(picked, function(id: string): Node
					local s = BY_ID[id]
					return pane(
						"EDITOR",
						ds.Text { "⌘F find  ⌃G line  ⌥⌘[ fold  ⇧⌥⌘[ fold all", style = "caption", tone = "tertiary" },
						CodeEditor {
							value = text_of(id),
							language = s.language,
							wrap = wrap,
							label = s.label,
							w_full = true,
							h_full = true,
							blocks = blocks_of(id),
							block_view = block_view(id, `{s.label}.sample`),
							on_add_block = function(line: number)
								local blocks = blocks_of(id)
								local list = table.clone(blocks:peek())
								table.insert(list, { key = `comment-{line}`, line = line, kind = "composer" })
								blocks:set(list)
							end,
							on_block_moved = function(key: string, line: number)
								local list = table.clone(blocks_of(id):peek())
								for i, b in list do
									if b.key == key then
										local moved = table.clone(b)
										moved.line = line
										list[i] = moved
									end
								end
								blocks_of(id):set(list)
							end,
							on_save = function(text: string)
								local _, lines = string.gsub(text, "\n", "")
								said:set(`Saved {s.label}: {lines + 1} lines`)
							end,
						}
					)
				end, { grow = 1, basis = 0, min_w = 520 }),
				ui.presence(picked, function(id: string): Node
					local s = BY_ID[id]
					return pane(
						"READ-ONLY",
						ui.row { gap = "$space.1", go_button(1), go_button(if id == "huge" or id == "large" then 9876 else 20) },
						CodeEditor {
							value = text_of(id),
							language = s.language,
							readonly = true,
							wrap = wrap,
							reveal = reveal,
							label = `{s.label} (read-only)`,
							w_full = true,
							h_full = true,
						}
					)
				end, { grow = 1, basis = 0, min_w = 520 }),
			},
		}
	end,
}
