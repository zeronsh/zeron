--!strict
-- A main chat's notes, in a tab of its panel: a blank page for the person
-- to write on, Markdown shown as it is typed (ds/markdown_edit, the
-- composer's). One note per main chat, synced across the person's
-- computers: the session doc's `notes` text (crates/doc notes.rs,
-- docs/session-notes.md), read and written through this device's own
-- engine (src/notes.rs, screens/panel/notes_api), as the shipping app's
-- Notes tab does (crates/ui notes.rs), whose rules these are:
--
-- Writes are compare-and-set against the hash of the text the engine last
-- reported, the base: 750 ms after typing stops, at once as the editor loses
-- the keyboard or the tab goes. One write is in flight at a time. A change
-- from another computer that lands meanwhile is held, then merged into the
-- editor (`rebase`, which keeps both sides' typing); the caret and the
-- selection stay where they were in the text. Reports carry a version
-- (`epoch`, `seq`): one no newer than the base (a late echo of this
-- editor's own write) is dropped; a newer one merges, even if its text is
-- one this editor wrote before. Only a write whose reply was lost is taken
-- without a merge when the engine reports its text.
--
-- An engine from before notes keeps them here, on this computer only
-- (`session_notes_local`), and says "Syncs once Zeron updates"; the first
-- report from an engine that has them takes them in (both kept, if both
-- have words) and they're kept here no more.
--
-- The editor's text and the base are kept outside the view, by chat, so a
-- tab closed and opened again, another session in between or a reload finds
-- them as they were; a write still lands after its tab has gone.
--
--   Notes.Tab { chat }       the tab's body
--   Notes.focus(chat)        the keyboard to its editor as it next shows
--   Notes.flush(chat)        write what's unwritten now
--   Notes.text(chat)         its editor's text (a state)
--   Notes.diff, Notes.rebase, Notes.map_offset, Notes.hash, Notes.supersedes
--                            the merge's parts (crates/doc notes.rs's)

local markdown = require("@ds/markdown_edit")
local api = require("@app/screens/panel/notes_api")

local M = {}

-- Quiet after the last keystroke before the notes are written, and the
-- wait before a write that failed (not refused) is tried again.
M.DEBOUNCE_MS = 750
local RETRY_MS = 2000

export type Snapshot = { text: string, hash: string, epoch: number, seq: number }
export type Edit = { from: number, to: number, insert: string }

-- Notes kept on this computer while the engine can't sync them, by chat.
local kept = ui.persist("session_notes_local", {} :: { [string]: string })

-- ---------------------------------------------------------------------------
-- The merge (crates/doc notes.rs), in bytes from 0 as an input's selection
-- counts them; edits never split a character.

local function continuation(s: string, at: number): boolean
	-- (The byte after offset `at`, 0-based: a UTF-8 continuation byte.)
	local b = string.byte(s, at + 1)
	return b ~= nil and b >= 0x80 and b < 0xC0
end

-- The smallest single replacement turning `old` into `new`: bytes
-- `from`..`to` of `old` (0-based, `to` exclusive) become `insert`. Nil when
-- they're equal.
function M.diff(old: string, new: string): Edit?
	if old == new then
		return nil
	end
	local o, n = #old, #new
	local prefix = 0
	local most = math.min(o, n)
	while prefix < most and string.byte(old, prefix + 1) == string.byte(new, prefix + 1) do
		prefix += 1
	end
	while prefix > 0 and (continuation(old, prefix) or continuation(new, prefix)) do
		prefix -= 1
	end
	local room = most - prefix
	local suffix = 0
	while suffix < room and string.byte(old, o - suffix) == string.byte(new, n - suffix) do
		suffix += 1
	end
	while suffix > 0 and (continuation(old, o - suffix) or continuation(new, n - suffix)) do
		suffix -= 1
	end
	return { from = prefix, to = o - suffix, insert = string.sub(new, prefix + 1, n - suffix) }
end

-- Where an offset into the old text lands in the new one: before the edit it
-- stays, after it it shifts, inside the replaced range it goes to the end of
-- what was put in.
function M.map_offset(e: Edit, offset: number): number
	if offset <= e.from then
		return offset
	elseif offset >= e.to then
		return offset - (e.to - e.from) + #e.insert
	end
	return e.from + #e.insert
end

-- Three-way merge of two single-range edits of `base`: edits that don't
-- overlap both apply; overlapping ones honour both deletions and keep both
-- insertions, the remote one first. Nobody's typing is dropped.
function M.rebase(base: string, mine: string, remote: string): string
	if mine == remote then
		return mine
	end
	local l, r = M.diff(base, mine), M.diff(base, remote)
	if not l then
		return remote
	elseif not r then
		return mine
	end
	local function cut(a: number, b: number): string
		return string.sub(base, a + 1, b)
	end
	if r.to <= l.from then
		return cut(0, r.from) .. r.insert .. cut(r.to, l.from) .. l.insert .. cut(l.to, #base)
	elseif l.to <= r.from then
		return cut(0, l.from) .. l.insert .. cut(l.to, r.from) .. r.insert .. cut(r.to, #base)
	end
	local from, to = math.min(l.from, r.from), math.max(l.to, r.to)
	return cut(0, from) .. r.insert .. l.insert .. cut(to, #base)
end

-- The notes' hash (crates/doc `notes_hash`): FNV-1a 64 over the bytes, 16
-- lowercase hex digits, in two 32-bit halves.
function M.hash(text: string): string
	local hi, lo = 0xcbf29ce4, 0x84222325
	for i = 1, #text do
		lo = bit32.bxor(lo, string.byte(text, i))
		-- × 0x100000001b3: × 0x1b3, and the low half shifted 40 up (8 into
		-- the high half).
		local low = lo * 0x1b3
		local carry = math.floor(low / 0x100000000)
		hi = (hi * 0x1b3 + carry + bit32.lshift(lo, 8)) % 0x100000000
		lo = low % 0x100000000
	end
	return string.format("%08x%08x", hi, lo)
end

-- Whether `s` came after `base`: from another of the engine's doc
-- instances, or later from the same one.
function M.supersedes(s: Snapshot, base: Snapshot): boolean
	return s.epoch ~= base.epoch or s.seq > base.seq
end

-- ---------------------------------------------------------------------------
-- A chat's notes, kept outside the views.

type Doc = {
	chat: string,
	-- The editor's text.
	text: State<string>,
	-- "loading" (nothing heard yet: read only), "synced", or "local" (an
	-- engine from before notes: kept on this computer).
	mode: State<string>,
	-- What went wrong saving, in words; nil while all is well.
	problem: State<string?>,
	-- The notes as the engine last reported them: the next write's base.
	base: State<Snapshot?>,
	-- A newer report heard while a write was out.
	held: Snapshot?,
	-- The hash of a write that failed without a reply: it may have landed.
	unconfirmed: string?,
	in_flight: boolean,
	-- The pending write's ticket (a newer one cancels it).
	ticket: number,
	-- The editor, while one shows; and whether it wants the keyboard.
	editor: any?,
	focus: boolean,
	-- Local notes taken into the synced ones, kept until a write lands them.
	migrating: boolean,
}

local docs: { [string]: Doc } = {}

local function doc(chat: string): Doc
	local d = docs[chat]
	if d then
		return d
	end
	local made: Doc = {
		chat = chat,
		text = ui.state(`session_notes.text.{chat}`, ""),
		mode = ui.state(`session_notes.mode.{chat}`, "loading"),
		problem = ui.state(`session_notes.problem.{chat}`, nil :: string?),
		base = ui.state(`session_notes.base.{chat}`, nil :: Snapshot?),
		held = nil,
		unconfirmed = nil,
		in_flight = false,
		ticket = 0,
		editor = nil,
		focus = false,
		migrating = false,
	}
	docs[chat] = made
	return made
end

function M.text(chat: string): State<string>
	return doc(chat).text
end

local function forget_kept(chat: string)
	if kept:peek()[chat] ~= nil then
		kept:set_key(chat, nil)
	end
end

-- The editor shows `new` in place of `old`: its caret and selection stay
-- where they were in the text.
local function show(d: Doc, old: string, new: string)
	local el = if d.editor then d.editor:peek() else nil
	local a, b = nil, nil
	if el then
		a, b = el:selection_bytes()
	end
	d.text:set(new)
	local e = M.diff(old, new)
	if el and e and a and b then
		el:set_selection_bytes(M.map_offset(e, a), M.map_offset(e, b))
	end
end

local write: (d: Doc) -> ()

local function schedule(d: Doc, delay: number)
	d.ticket += 1
	local mine = d.ticket
	ui.after(delay, function()
		if d.ticket == mine then
			write(d)
		end
	end)
end

-- Kept on this computer from now on (an engine from before notes).
local function go_local(d: Doc)
	if d.mode:peek() == "local" then
		return
	end
	local was = d.mode:peek()
	d.mode:set("local")
	d.base:set(nil)
	d.held, d.unconfirmed, d.migrating = nil, nil, false
	d.problem:set(nil)
	local saved = kept:peek()[d.chat]
	if was == "loading" and saved and d.text:peek() == "" then
		show(d, "", saved)
	elseif d.text:peek() ~= "" then
		kept:set_key(d.chat, d.text:peek())
	end
end

-- Another computer's notes, folded into the editor with what's typed here
-- that the engine hasn't seen.
local function merge(d: Doc, remote: Snapshot)
	local base = d.base:peek()
	local mine = d.text:peek()
	local merged = M.rebase(if base then base.text else "", mine, remote.text)
	-- (The base first: the editor's change then reads as nothing to write.)
	d.base:set(remote)
	if merged ~= mine then
		show(d, mine, merged)
	end
	if merged ~= remote.text then
		schedule(d, 0)
	end
end

local function take_remote(d: Doc, remote: Snapshot)
	local base = d.base:peek()
	if not base or not M.supersedes(remote, base) then
		return
	end
	if remote.hash == base.hash or d.unconfirmed == remote.hash then
		d.unconfirmed = nil
		d.base:set(remote)
		return
	end
	merge(d, remote)
end

-- What the engine reports.
local function receive(d: Doc, r: any?)
	if type(r) ~= "table" then
		return
	end
	if r.unsupported then
		go_local(d)
		return
	end
	if type(r.text) ~= "string" or type(r.hash) ~= "string" then
		return
	end
	local remote: Snapshot = { text = r.text, hash = r.hash, epoch = tonumber(r.epoch) or 0, seq = tonumber(r.seq) or 0 }
	local base = d.base:peek()
	if not base then
		-- The first report: the editor starts from it, with any notes kept on
		-- this computer before (both kept, theirs first).
		local mine = if d.mode:peek() == "local" then d.text:peek() else (kept:peek()[d.chat] or "")
		local start = remote.text
		if mine ~= "" and mine ~= remote.text then
			if remote.text == "" then
				start = mine
			else
				local sep = if string.sub(remote.text, -1) == "\n" then "" else "\n"
				start = remote.text .. sep .. mine
			end
		end
		d.mode:set("synced")
		d.problem:set(nil)
		d.base:set(remote)
		local old = d.text:peek()
		if start ~= old then
			show(d, old, start)
		end
		if start ~= remote.text then
			d.migrating = true
			schedule(d, 0)
		else
			forget_kept(d.chat)
		end
		return
	end
	if not M.supersedes(remote, base) then
		return
	end
	if d.in_flight then
		d.held = remote
		return
	end
	take_remote(d, remote)
end

local function message(err: any): string
	return if type(err) == "table" and err.message then tostring(err.message) else tostring(err)
end

local function written(d: Doc, text: string, reply: any?, err: string?)
	d.in_flight = false
	local retry: number? = 0
	local kind = if type(reply) == "table" then reply.kind else nil
	if kind == "applied" then
		d.problem:set(nil)
		d.unconfirmed = nil
		d.base:set({ text = text, hash = reply.hash, epoch = tonumber(reply.epoch) or 0, seq = tonumber(reply.seq) or 0 })
		if d.migrating then
			d.migrating = false
			forget_kept(d.chat)
		end
	elseif kind == "conflict" and type(reply.current) == "table" then
		-- (Read at the write: newer than anything the watch sent first.)
		d.problem:set(nil)
		local c = reply.current
		d.held = { text = c.text or "", hash = c.hash or "", epoch = tonumber(c.epoch) or 0, seq = tonumber(c.seq) or 0 }
	elseif kind == "tooLarge" then
		local kb = math.floor((tonumber(reply.limit) or 0) / 1024)
		d.problem:set(ui.t("panel.notes.too_large", { kb = kb }):get())
		-- (The next edit tries again; the same text can't land.)
		retry = nil
	elseif err == "unsupported" then
		go_local(d)
		return
	else
		d.problem:set(ui.t("panel.notes.save_failed"):get())
		d.unconfirmed = M.hash(text)
		retry = RETRY_MS
	end
	local held = d.held
	if held then
		d.held = nil
		take_remote(d, held)
	end
	local base = d.base:peek()
	if base and base.text ~= d.text:peek() and retry then
		schedule(d, retry)
	end
end

write = function(d: Doc)
	if d.in_flight or d.mode:peek() ~= "synced" then
		return
	end
	local base = d.base:peek()
	if not base then
		return
	end
	local text = d.text:peek()
	if text == base.text then
		return
	end
	d.in_flight = true
	api.set(d.chat, base.hash, text):after(function(reply: any)
		written(d, text, reply, nil)
	end, function(err: any)
		written(d, text, nil, message(err))
	end)
end

-- The editor's text changed (typed, or put there).
local function edited(d: Doc)
	local mode = d.mode:peek()
	local text = d.text:peek()
	if mode == "local" then
		if (kept:peek()[d.chat] or "") ~= text then
			kept:set_key(d.chat, if text == "" then nil else text)
		end
		return
	end
	local base = d.base:peek()
	if mode == "synced" and base and base.text ~= text then
		schedule(d, M.DEBOUNCE_MS)
	end
end

function M.flush(chat: string)
	local d = docs[chat]
	if not d then
		return
	end
	-- (One out already writes what's left as it comes back.)
	d.ticket += 1
	write(d)
end

function M.focus(chat: string)
	local d = doc(chat)
	d.focus = true
	local el = if d.editor then d.editor:peek() else nil
	if el then
		d.focus = false
		ui.after(0, function()
			el:focus()
		end)
	end
end

-- For a fixture: a report as the engine would send it.
M.receive = function(chat: string, r: any)
	receive(doc(chat), r)
end

-- ---------------------------------------------------------------------------
-- The tab.

local function enter(d: Doc, e: any): boolean
	-- Enter in a list: the next item's marker after the break (or, on an
	-- empty item, the item undone), as the composer's Shift+Enter.
	if e.key ~= "Enter" or e.cmd or e.ctrl or e.alt or e.shift then
		return false
	end
	local el = if d.editor then d.editor:peek() else nil
	if not el or d.mode:peek() == "loading" then
		return false
	end
	local a, b = el:selection_bytes()
	if not a or not b then
		return false
	end
	local from, to, text = markdown.newline(d.text:peek(), a, b)
	if from and to and text then
		el:set_selection_bytes(from, to)
		el:replace_selection(text)
		return true
	end
	return false
end

M.Tab = ui.component(function(props: { chat: string }): Node
	local chat = props.chat
	local d = doc(chat)
	local editor = ui.ref()
	d.editor = editor
	ui.on_cleanup(function()
		if d.editor == editor then
			d.editor = nil
		end
		-- (Leaving: what's unwritten goes now.)
		M.flush(chat)
	end)
	ui.on_cleanup(api.watch(chat))
	ui.effect(function()
		receive(d, api.report(chat))
	end)
	ui.effect(function()
		d.text:get()
		edited(d)
	end)

	local prose = (ui.theme.get("zeron.prose.size") :: number?) or 15
	return ui.column {
		id = "panel-notes",
		grow = 1,
		w_full = true,
		min_h = 0,
		ui.textarea {
			id = "panel-notes-editor",
			ref = editor,
			label = ui.t("panel.notes.label"),
			value = d.text,
			placeholder = ui.t("panel.notes.placeholder"),
			read_only = function(): boolean
				return d.mode:get() == "loading"
			end,
			wrap = true,
			highlight = markdown.highlight,
			size = prose,
			line_height = "$zeron.prose.line",
			grow = 1,
			w_full = true,
			min_h = 0,
			px = "$space.5",
			pt = "$space.4",
			pb = "$space.3",
			on_mount = function(el: any)
				if d.focus then
					d.focus = false
					el:focus()
				end
			end,
			on_blur = function()
				M.flush(chat)
			end,
			on_key = function(e: any): boolean
				return enter(d, e)
			end,
		},
		ui.show(function(): boolean
			return d.problem:get() ~= nil
		end, ui.text {
			function(): string
				return d.problem:get() or ""
			end,
			id = "panel-notes-problem",
			role = "status",
			literal = true,
			wrap = true,
			w_full = true,
			px = "$space.5",
			pb = "$space.3",
			text_size = "$font.size.sm",
			color = "$danger",
		}),
		ui.show(function(): boolean
			return d.mode:get() == "local"
		end, ui.text {
			ui.t("panel.notes.local_only"),
			id = "panel-notes-local",
			role = "status",
			w_full = true,
			px = "$space.5",
			pb = "$space.3",
			text_size = "$font.size.sm",
			color = "$text.tertiary",
		}),
	}
end)

return M
