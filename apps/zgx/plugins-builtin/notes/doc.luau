--!strict
-- A main chat's notes, in a tab of its panel: a blank page for the person
-- to write on, Markdown shown as it is typed (ds/markdown_edit, the
-- composer's). One note per main chat, synced across the person's
-- computers: the session doc's `notes` text (crates/doc notes.rs,
-- docs/session-notes.md), read and written through Zeron ("notes":
-- `zeron.notes`, only the chat the plugin is shown in), as the shipping
-- app's Notes tab does (crates/ui notes.rs), whose rules these are:
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
-- An engine from before notes keeps them here, on this computer only (the
-- plugin's store, `local`), and says "Syncs once Zeron updates"; the first
-- report from an engine that has them takes them in (both kept, if both
-- have words) and they're kept here no more.
--
-- The editor's text and the base are kept outside the view, by chat, so a
-- tab closed and opened again, another session in between, finds them as
-- they were; a write still lands after its tab has gone.
--
--   local Doc = require("./doc")(zeron)
--   Doc.Tab { chat }       the tab's body
--   Doc.focus(chat)        the keyboard to its editor as it next shows
--   Doc.flush(chat)        write what's unwritten now
--   Doc.diff, Doc.rebase, Doc.map_offset, Doc.hash, Doc.supersedes
--                          the merge's parts (merge.luau)

local markdown = require("@ds/markdown_edit")
local merge = require("./merge")

type Snapshot = merge.Snapshot

-- Quiet after the last keystroke before the notes are written, and the
-- wait before a write that failed (not refused) is tried again.
local DEBOUNCE_MS = 750
local RETRY_MS = 2000

return function(zeron: any)
	local M: any = table.clone(merge)
	M.DEBOUNCE_MS = DEBOUNCE_MS

	-- Notes kept on this computer while the engine can't sync them, by chat
	-- (the plugin's store: removed with it).
	local kept = zeron.store("local", {})
	local function kept_of(chat: string): string?
		local all = kept:peek()
		return if type(all) == "table" then all[chat] else nil
	end
	local function keep(chat: string, text: string?)
		if kept_of(chat) == text then
			return
		end
		local all = table.clone(kept:peek() or {})
		all[chat] = text
		kept:set(all)
	end

	-- -------------------------------------------------------------------------
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
			text = ui.state(`notes.text.{chat}`, ""),
			mode = ui.state(`notes.mode.{chat}`, "loading"),
			problem = ui.state(`notes.problem.{chat}`, nil :: string?),
			base = ui.state(`notes.base.{chat}`, nil :: Snapshot?),
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
		local saved = kept_of(d.chat)
		if was == "loading" and saved and d.text:peek() == "" then
			show(d, "", saved)
		elseif d.text:peek() ~= "" then
			keep(d.chat, d.text:peek())
		end
	end

	-- Another computer's notes, folded into the editor with what's typed here
	-- that the engine hasn't seen.
	local function fold(d: Doc, remote: Snapshot)
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
		fold(d, remote)
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
			local mine = if d.mode:peek() == "local" then d.text:peek() else (kept_of(d.chat) or "")
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
				keep(d.chat, nil)
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
				keep(d.chat, nil)
			end
		elseif kind == "conflict" and type(reply.current) == "table" then
			-- (Read at the write: newer than anything the watch sent first.)
			d.problem:set(nil)
			local c = reply.current
			d.held = { text = c.text or "", hash = c.hash or "", epoch = tonumber(c.epoch) or 0, seq = tonumber(c.seq) or 0 }
		elseif kind == "tooLarge" then
			local kb = math.floor((tonumber(reply.limit) or 0) / 1024)
			d.problem:set(zeron.s("tab.too_large", { kb = kb }))
			-- (The next edit tries again; the same text can't land.)
			retry = nil
		elseif err and string.find(err, "unsupported", 1, true) then
			go_local(d)
			return
		else
			d.problem:set(zeron.s("tab.save_failed"))
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
		local ok, promise = pcall(zeron.notes.set, d.chat, base.hash, text)
		if not ok then
			-- (A Zeron from before notes: no way to them.)
			written(d, text, nil, "unsupported")
			return
		end
		promise:after(function(reply: any)
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
			keep(d.chat, if text == "" then nil else text)
			return
		end
		local base = d.base:peek()
		if mode == "synced" and base and base.text ~= text then
			schedule(d, DEBOUNCE_MS)
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

	-- -------------------------------------------------------------------------
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

	local function prose_size(): number
		local ok, size = pcall(ui.theme.get, "zeron.prose.size")
		return if ok and type(size) == "number" then size else 15
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
		local report = zeron.notes.watch(chat)
		ui.effect(function()
			receive(d, report())
		end)
		ui.effect(function()
			d.text:get()
			edited(d)
		end)

		return ui.column {
			id = "notes-tab",
			grow = 1,
			w_full = true,
			h_full = true,
			min_h = 0,
			ui.textarea {
				id = "notes-editor",
				ref = editor,
				label = zeron.t("tab.label"),
				value = d.text,
				placeholder = zeron.t("tab.placeholder"),
				read_only = function(): boolean
					return d.mode:get() == "loading"
				end,
				wrap = true,
				highlight = markdown.highlight,
				size = prose_size(),
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
				id = "notes-problem",
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
				zeron.t("tab.local_only"),
				id = "notes-local",
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
end
