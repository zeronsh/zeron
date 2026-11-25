--!strict
-- A store plugin's page, under Settings → Plugins (as an agent of the
-- registry opens under Providers): what it is and who made it, the one
-- thing to do with it on the device shown, its README, its pictures, what
-- it asks for, its versions and what changed, and where it comes from.
--
-- The one thing to do follows the device shown and what it has:
--   Install               not there (the same consent window then, as any
--                         plugin someone else made: nothing here allows it)
--   Update to <version>   a newer version found for it: what changed, and
--                         what it asks for more or no longer, read first
--   Review                waiting for you to allow it
--   Turn on               allowed, and off
--   Open settings         on, here, with settings of its own
--   (none)                there, and nothing to do
-- each the page's one primary button, its state said in one line under it
-- (installed, the version here, waiting for you, off, the device away). A
-- version Zeron took down says why, and can be turned back on (it's
-- yours); one made for another Zeron, or a plugin from elsewhere with its
-- id, says so in a callout under its summary, and doesn't install.
-- Nothing installs unless the person asks here.
--
-- The README is drawn as the app draws Markdown, from the words its author
-- signed and the store reviewed: its pictures only from its own package
-- (through the store, kept here), opening whole in the lightbox; one from
-- anywhere else is its link; raw HTML is words, never run; links open
-- where links open.
--
--   listing.Page { plugin, here }  `here`: the Plugins page's device shown
--                               (`device()`, `device_name()`, `away()`) and
--                               its outcomes (`settled`, `failed`)

local ds = require("@ds")
local FoldButton = require("@ds/fold_button").FoldButton
local Lightbox = require("@ds/lightbox")
local Links = require("@ds/links")
local locale = require("@ds/locale")
local words = require("@app/screens/plugins/words")
local client = require("@app/screens/plugins/store_client")
local kinds = require("@app/screens/plugins/store_kind")
local asks_view = require("@app/screens/plugins/asks")
local consent = require("@screens/plugins/consent")
local untrusted = require("@app/screens/plugins/untrusted")
local sessions = require("@account/sessions")

export type Here = {
	device: () -> string?,
	device_name: () -> string,
	away: () -> boolean,
	settled: (plain: string, versioned: string) -> (done: any) -> (),
	failed: (title: string | Message) -> (err: any) -> (),
}

local C = {}

function C.make(kind: kinds.Kind, browse: any)
	local store, state, nav = kind.store, kind.state, kind.nav
	local say = kinds.say(kind)
	local template = kind.id == "template"
	local plugin_settings: any = kind.settings
	local M = {}

	-- Each plugin's page as last read, by id: one opened again draws at once,
	-- and is read again behind it.
	local details = ui.state(kind.keys .. "_details", {} :: { [string]: any })
	-- Each plugin's page asked to be read again (a number that moves), by id:
	-- when the engine refused to install it, or Zeron took a version of it
	-- down, so the page never keeps a mark or an Install the store no longer
	-- stands behind.
	local rereads = ui.state(kind.keys .. "_rereads", {} :: { [string]: number })
	local function reread(id: string)
		rereads:set_key(id, (rereads:peek()[id] or 0) + 1)
	end

	-- A link the page may open: the web's alone.
	local function web(url: any): string?
		if type(url) == "string" and #url <= 2048 and (string.match(url, "^https://[%w%-%.]+") or string.match(url, "^http://[%w%-%.]+")) then
			return url
		end
		return nil
	end

	-- "github.com/ada/focus", for a link's words: a long one kept to its
	-- start and its end, "…" between (its whole in its tooltip).
	local SHOWN_URL = 56
	local function shown_url(url: string): string
		local w = string.gsub(url, "^https?://", "")
		w = string.gsub(w, "/$", "")
		if utf8.len(w) and (utf8.len(w) :: number) > SHOWN_URL then
			local head = string.sub(w, 1, utf8.offset(w, 33) - 1)
			local tail = string.sub(w, utf8.offset(w, -20) :: number)
			w = `{head}…{tail}`
		end
		return w
	end

	-- A picture path of its package, as the store serves them: relative,
	-- inside it. Anything else (a URL, an absolute path, a way out) isn't one.
	local function package_path(src: string): string?
		if src == "" or #src > 512 or string.find(src, "^%a[%w+.-]*:") or string.sub(src, 1, 1) == "/" or string.find(src, "[\\%%:]") then
			return nil
		end
		local parts: { string } = {}
		for part in string.gmatch(src, "[^/]+") do
			if part == ".." then
				return nil
			elseif part ~= "." then
				table.insert(parts, part)
			end
		end
		if #parts == 0 then
			return nil
		end
		local path = table.concat(parts, "/")
		local ext = string.lower(string.match(path, "%.(%w+)$") or "")
		if ext ~= "png" and ext ~= "jpg" and ext ~= "jpeg" and ext ~= "gif" and ext ~= "webp" then
			return nil
		end
		return path
	end

	local function said(node: Node): Node
		return ui.column { w_full = true, py = "$space.6", px = "$space.4", node }
	end

	-- A version's notes (its changelog, the store takes 64 KB of it), kept to
	-- `lines` lines of the small prose while folded (the latest's: 10 lines of
	-- 20, a 200 pt clamp, which shows eight list items at their 26 pitch), their foot fading, with
	-- Show more under them when there's more; open, the whole of it, and Show
	-- less. Folded, only its start is drawn: enough for the lines it shows,
	-- each piece ending at a line's end, so a long one costs what shows. One
	-- line folded is its first line's words alone (an older version's).
	local VERSIONS_FIRST = 3
	local NOTES_LINES = 10
	local NOTES_MOST = 64 * 1024
	-- (About ten lines' worth of the source: what the fold shows, and more.)
	local NOTES_START = 2048
	local FADE_LINES = 1.5
	-- About, folded: 1,200 high, about a screen and a half of the pane in
	-- Zeron's own window (820 high, the pane under its bar about 780), so
	-- the page past it (Details, Versions, Report) is a scroll away, whatever
	-- the author wrote; shown whole when at most 240 taller (ten of its lines,
	-- `zeron.prose.line` 24), not folded for the sake of a few lines. Folded,
	-- its first piece ends at `ABOUT_LINES` lines that aren't blank: each is
	-- 20 high or more (a code line 20, prose and a list's 24, a heading more),
	-- so 72 reach past the fold and its slack (1,440), and it lays out about
	-- what the fold shows, never the 36,000 a whole first piece could.
	local ABOUT_FOLD = 1200
	local ABOUT_SLACK = 240
	local ABOUT_LINES = 72

	-- A text's start: up to `most` bytes, ended at a line's end where it can
	-- be; and whether that's all of it.
	local function start_of(text: string, most: number): (string, boolean)
		if #text <= most then
			return text, true
		end
		local cut = string.sub(text, 1, most)
		local at = string.match(cut, "^.*()\n")
		return if at and at > 1 then string.sub(cut, 1, at - 1) else cut, false
	end

	-- A line's words without its Markdown marks: a heading's #s, a list's
	-- marker, emphasis and code ticks, a link's target.
	local function words_of(line: string): string
		local w = string.gsub(line, "^%s*#+%s*", "")
		w = string.gsub(w, "^%s*[-*+]%s+", "")
		w = string.gsub(w, "^%s*%d+[.)]%s+", "")
		w = string.gsub(w, "!?%[([^%]]*)%]%([^)]*%)", "%1")
		w = string.gsub(w, "[*_`]+", "")
		return (string.gsub(w, "^%s+", ""))
	end
	local function first_line(text: string): string
		for line in string.gmatch(text, "[^\n]+") do
			local w = words_of(line)
			if string.match(w, "%S") then
				return w
			end
		end
		return ""
	end

	-- A text folded at a height: no taller than `height`, its end fading, with
	-- Show more under it; open, all of it, and Show less (a version's notes,
	-- About). Shown whole when it's at most `slack` taller than the fold (a
	-- few lines aren't worth a fold), unless `whole` says the folded body isn't
	-- all there is. `next` (open): whether there's a next piece to show, and
	-- the way to show it, beside Show less. Folded again, the control is
	-- brought back into view, focused.
	type FoldProps = {
		height: number,
		slack: number?,
		fade: number,
		body: (open: boolean) -> Node,
		whole: (() -> boolean)?,
		next: untrusted.Handle?,
		class: string,
		button_class: string,
		next_id: string?,
	}
	local Fold = ui.component(function(props: FoldProps): Node
		local open = ui.use_state(false)
		local body_h = ui.use_state(0)
		local slack = props.slack or 0
		local controls = ui.ref()
		local function whole(): boolean
			return if props.whole then props.whole() else true
		end
		-- Whether there's more than the fold shows, as known: a body not yet
		-- measured isn't said to be more (no fade, no Show more, for a moment).
		local function more(): boolean
			return not whole() or (body_h:get() > 0 and body_h:get() > props.height + slack + 0.5)
		end
		local function folded(): boolean
			return not open:get() and more()
		end
		-- (Held at the fold until it's measured: never drawn taller, then cut.)
		local function clipped(): boolean
			return not open:get() and (body_h:get() == 0 or more())
		end
		local function next_shown(): boolean
			return open:get() and props.next ~= nil and props.next.more:get()
		end
		return ui.column {
			class = props.class,
			w_full = true,
			gap = "$space.2",
			ui.column {
				w_full = true,
				-- (Clipped only while folded: open, what overhangs its box is
				-- drawn, as a heading's last descender is past its -6.)
				overflow = function(): string
					return if clipped() then "hidden" else "visible"
				end,
				max_h = function(): number
					return if clipped() then props.height else 1e7
				end,
				fade = function(): { number }
					return { 0, if folded() then props.fade else 0 }
				end,
				ui.column {
					w_full = true,
					on_resize = function(size: { w: number, h: number })
						if math.abs(size.h - body_h:peek()) >= 0.5 then
							body_h:set(size.h)
						end
					end,
					ui.presence(function(): string
						return if open:get() then "open" else "folded"
					end, function(key: string): Node
						return props.body(key == "open")
					end, { w_full = true }),
				},
			},
			ui.show(function(): boolean
				return open:get() or more()
			end, ui.row {
				ref = controls,
				gap = "$space.4",
				items = "center",
				self = "start",
				ui.show(next_shown, ui.row {
					id = props.next_id,
					FoldButton {
						label = function(): string
							return ui.t("plugins.store.readme_more"):get()
						end,
						open = function(): boolean
							return false
						end,
						on_click = function()
							local h = props.next
							if h and h.next then
								h.next()
							end
						end,
					},
				}),
				FoldButton {
					label = function(): string
						return if open:get() then ui.t("plugins.store.show_less"):get() else ui.t("plugins.store.readme_more"):get()
					end,
					open = function(): boolean
						return open:get()
					end,
					on_click = function()
						local folding = open:peek()
						open:set(not folding)
						if not folding then
							return
						end
						-- (Folded from far down a long one: brought back into
						-- view as it's laid out again. Focusing the row hands
						-- focus to its first control, and zgx scrolls the least
						-- that shows it.)
						ui.after(0, function()
							local el = controls:peek()
							if el then
								el:blur()
								el:focus()
							end
						end)
					end,
					class = props.button_class,
				},
			}),
		}
	end)

	local Notes = ui.component(function(props: { text: () -> string, lines: number }): Node
		local function start(): (string, boolean)
			return start_of(props.text(), NOTES_START)
		end
		if props.lines ~= 1 then
			local line = (ui.theme.get("zeron.prose.small.line") :: number?) or 20
			return Fold {
				height = props.lines * line,
				fade = line * FADE_LINES,
				whole = function(): boolean
					local _, whole = start()
					return whole
				end,
				-- (Folded, its start alone; open, the whole, a piece a frame:
				-- bounded as the store's every text, screens/plugins/untrusted.)
				body = function(open: boolean): Node
					return untrusted.View {
						text = if open then props.text() else (start()),
						what = "notes",
						small = true,
						auto = true,
					}
				end,
				class = "store-notes",
				button_class = "store-notes-more",
			}
		end
		-- (An older version's: its first line's words, then the whole.)
		local open = ui.use_state(false)
		local function more(): boolean
			local rest = string.gsub(props.text(), "^%s*[^\n]*", "", 1)
			return string.match(rest, "%S") ~= nil
		end
		return ui.column {
			class = "store-notes",
			w_full = true,
			gap = "$space.2",
			ui.presence(function(): string
				return if open:get() then "open" else "line"
			end, function(key: string): Node
				if key == "line" then
					return ui.text {
						function(): string
							return first_line(props.text())
						end,
						w_full = true,
						lines = 1,
						truncate = true,
						text_size = "$zeron.prose.small.size",
						line_height = "$zeron.prose.small.line",
						-- (The notes' own ink: the latest's are the same words.)
						color = "$text.primary",
					}
				end
				-- (Bounded as the store's every text: screens/plugins/untrusted.)
				return untrusted.View { text = props.text(), what = "notes", small = true, auto = true }
			end, { w_full = true }),
			ui.show(function(): boolean
				return open:get() or more()
			end, FoldButton {
				label = function(): string
					return if open:get() then ui.t("plugins.store.show_less"):get() else ui.t("plugins.store.readme_more"):get()
				end,
				open = function(): boolean
					return open:get()
				end,
				on_click = function()
					open:set(not open:peek())
				end,
				class = "store-notes-more",
				place = { self = "start" },
			}),
		}
	end)

	-- The page itself, its detail read.
	local Found = ui.component(function(props: { plugin: string, here: Here }): Node
		local id, here = props.plugin, props.here
		local first: any = details:peek()[id] or {}
		-- (What was read of it; once that's let go (no longer in the store),
		-- the page as it was built, while it leaves.)
		local function d(): any
			return details:get()[id] or first
		end
		-- (The version this page is of: a newer one builds the page again.)
		local version: string = if type(first.version) == "string" then first.version else ""
		local busy = ui.use_state(false)
		local updating = ui.use_state(false)
		local update_open = ui.use_state(false)
		local report_open = ui.use_state(false)
		local report_reason = ui.use_state(store.REPORT_REASONS[1])
		local report_text = ui.use_state("")
		local unblocking = ui.use_state(false)
		local unblock_open = ui.use_state(false)
		-- Each time a link (`zeron://open/plugin/<id>`) names this page, as it
		-- opens or while it's open, every install on it takes no click until it
		-- has shown a moment, the consent window's own (`consent.ARM_MS`),
		-- disabled meanwhile as Allow is there: a page that comes forward under
		-- a click mid-way never takes it. Opened from inside Zeron, it acts at
		-- once.
		local armed = ui.use_state(nav.link_seq(id) == 0)
		local arming = 0
		ui.effect(function()
			local seq = nav.link_seq(id)
			if seq == 0 then
				return
			end
			arming += 1
			local mine = arming
			armed:set(false)
			ui.after(consent.ARM_MS, function()
				-- (Only the latest link's wait ends it.)
				if mine == arming then
					armed:set(true)
				end
			end)
		end)

		-- The version a link chose (`zeron://open/plugin/<id>?version=<v>`):
		-- its record, from the versions' first page or else the store's answer
		-- for it. Install acts on it and says so; one that takes no new
		-- installs (or that the store hasn't) leaves Install off and says why.
		-- Fail closed: until its record is read, Install waits.
		local chosen = ui.use_state({ version = nil, state = "none", record = nil } :: any)
		ui.effect(function()
			local v = nav.linked_version(id)
			if v == chosen:peek().version then
				return
			end
			if v == nil then
				chosen:set({ version = nil, state = "none", record = nil })
				return
			end
			for _, rec in (if type(d().versions) == "table" then d().versions else {}) :: { any } do
				if type(rec) == "table" and rec.version == v then
					chosen:set({ version = v, state = "ok", record = rec })
					return
				end
			end
			chosen:set({ version = v, state = "loading", record = nil })
			store.version(id, v):after(function(rec: any)
				if chosen:peek().version == v then
					local found = type(rec) == "table" and rec.version == v
					chosen:set({ version = v, state = if found then "ok" else "missing", record = if found then rec else nil })
				end
			end, function(err: any)
				if chosen:peek().version == v then
					local m = if type(err) == "table" then err.message else err
					local word = if type(m) == "string" then string.match(m, "^([%l_]+)") else nil
					chosen:set({ version = v, state = if word == "not_found" or word == "not_in_store" then "missing" else "failed", record = nil })
				end
			end)
		end)
		-- The version the page speaks of and Install acts on: the chosen one
		-- while its record is coming or read, else (nil) the newest. One the
		-- store hasn't, or couldn't say, leaves the plugin's ordinary page.
		local function chosen_version(): string?
			local c = chosen:get()
			return if c.version and (c.state == "ok" or c.state == "loading") then c.version else nil
		end
		-- Whether Install may act: nothing chosen, or one the store hasn't
		-- (the ordinary page); a chosen one once read and listed. Until its
		-- record is read, or when it couldn't be, Install waits.
		local function chosen_installable(): boolean
			local c = chosen:get()
			if c.version == nil or c.state == "missing" then
				return true
			end
			return c.state == "ok" and browse.version_state(c.record) == "listed"
		end
		-- The sentence under the summary about the version a link chose, or
		-- "": where the person is (a version not the newest), why it takes no
		-- installs, or that the store hasn't it (or couldn't say).
		local function chosen_said(): string
			local c = chosen:get()
			local v = c.version
			if v == nil or c.state == "loading" then
				return ""
			elseif c.state == "missing" then
				return ui.t("plugins.store.link_version_missing", { version = v }):get()
			elseif c.state == "failed" then
				return ui.t("plugins.store.link_version_unread", { version = v }):get()
			end
			local rec = c.record
			local st = browse.version_state(rec)
			if st == "yanked" and rec.yanked_by == "superseded" and type(rec.superseded_by) == "string" and rec.superseded_by ~= "" then
				return ui.t("plugins.store.link_version_superseded", { version = v, by = rec.superseded_by }):get()
			elseif st == "yanked" then
				return ui.t("plugins.store.link_version_yanked", { version = v }):get()
			elseif st == "taken_down" then
				return ui.t("plugins.store.link_version_taken_down", { version = v }):get()
			elseif st == "removed" then
				return ui.t("plugins.store.link_version_removed", { version = v }):get()
			elseif v ~= d().version then
				return ui.t("plugins.store.link_version_for", { version = v, newest = d().version or "" }):get()
			end
			return ""
		end
		-- The version the head speaks of, and whether the store verified it
		-- (nil: nothing to say of it: a chosen one still being read, or one
		-- that takes no installs, its sentence saying why).
		local function head_version(): string
			return chosen_version() or d().version or ""
		end
		local function head_verified(): boolean?
			local c = chosen:get()
			if chosen_version() then
				if c.state ~= "ok" or browse.version_state(c.record) ~= "listed" then
					return nil
				end
				return c.record.verified == true
			end
			return d().verified
		end

		-- What the device shown has of it: this store plugin (installed from
		-- the store); another plugin that has its id (from a file, a link)
		-- is never taken for it.
		-- (A template's by the store's id alone: its folder may be named apart,
		-- and another's may have its name.)
		local function item(): any
			for _, p in state.shown:get() do
				local source: any = p.source
				if type(source) == "table" and source.kind == "store" and (if template then source.id else p.id) == id then
					return p
				end
			end
			return nil
		end
		local function other(): boolean
			for _, p in state.shown:get() do
				local source: any = p.source
				if p.id == id and not (type(source) == "table" and source.kind == "store") then
					return true
				end
			end
			return false
		end
		-- (One from elsewhere is in a plugin's way; a template installs beside it.)
		local function in_the_way(): boolean
			return not template and other()
		end
		-- Whether it works with this Zeron, as the engine judges it (its
		-- `engines.zeron` against the Zeron it runs), and why not in its words
		-- (the page is built again as either changes: its key).
		local function compatible(): boolean
			return d().compatible ~= false
		end
		local why_not: string? = if type(first.incompatible) == "string" and first.incompatible ~= "" then string.sub(first.incompatible, 1, 500) else nil
		-- Taken down by Zeron, and not turned back on.
		local function blocked(): any?
			local it: any = item()
			local b = if it then it.blocked else nil
			if template then
				return if type(b) == "table" then b else nil
			end
			return if type(b) == "table" and b.overridden ~= true then b else nil
		end

		-- What the one button does now.
		local function standing(): string
			local it: any = item()
			if it then
				if blocked() then
					return "blocked"
				elseif here.away() then
					return "away"
				elseif type(it.update) == "table" and it.update.refused == nil then
					return "update"
				elseif not template and it.status == "pending" or it.status == "tampered" or (it.status == "off" and #(it.ask or {}) > 0) then
					return "review"
				elseif template then
					return if it.active then "installed" else "use"
				elseif it.status == "off" then
					return "off"
				elseif it.status == "on" and here.device() == nil and plugin_settings.has(id) then
					return "settings"
				end
				return "installed"
			end
			if template and state.awaits(id) then
				return "review"
			elseif here.away() then
				return "away"
			elseif in_the_way() then
				return "other"
			elseif not compatible() then
				return "incompatible"
			end
			return "install"
		end

		-- (`version`: one of its versions the person chose, else the one the
		-- page offers.)
		local function install(version: string?)
			if busy:peek() then
				return
			end
			busy:set(true)
			-- (A string alone: as a click's handler it's handed the click.)
			local chosen = if type(version) == "string" then version else d().version
			local p: any = state.install({ store = id, version = chosen, device = here.device() } :: any)
			if p == nil then
				busy:set(false)
				return
			end
			p:after(function(done: any)
				busy:set(false)
				here.settled("plugins.page.installed_toast", "plugins.page.installed_toast_version")(done)
			end, function(err: any)
				busy:set(false)
				here.failed(say("plugins.page.couldnt_install"))(err)
				-- (Refused: what the store says of it now, read again.)
				reread(id)
			end)
		end
		local function update()
			if updating:peek() then
				return
			end
			updating:set(true)
			local name = d().name or id
			local p: any = state.update(id, here.device())
			if p == nil then
				updating:set(false)
				return
			end
			p:after(function(done: any)
				updating:set(false)
				here.settled("plugins.page.updated_toast", "plugins.page.updated_toast_version")(done)
			end, function(err: any)
				updating:set(false)
				here.failed(say("plugins.page.couldnt_update", { name = name }))(err)
			end)
		end
		local function turn_on()
			local p: any = state.set_off(id, false, here.device())
			if p ~= nil then
				p:after(function(_: any) end, here.failed(ui.t("plugins.page.couldnt_switch", { name = d().name or id })))
			end
		end
		-- (Yours to run: the engine keeps it on as you turned it.)
		local function unblock()
			unblocking:set(true)
			local p: any = state.set_off(id, false, here.device())
			if p == nil then
				unblocking:set(false)
				return
			end
			p:after(function(_: any)
				unblocking:set(false)
			end, function(err: any)
				unblocking:set(false)
				here.failed(ui.t("plugins.store.couldnt_unblock", { name = d().name or id }))(err)
			end)
		end

		-- (Every one of them the same button: the page's primary, small, as a
		-- row's controls are.)
		-- (And none takes a click before it's armed.)
		local function primary(props: { [any]: any }): Node
			props.tone, props.size, props.class = "primary", "sm", "store-action"
			local given, click = props.disabled, props.on_click
			props.disabled = function(): boolean
				if not armed:get() then
					return true
				end
				return if type(given) == "function" then given() == true else given == true
			end
			props.on_click = function(...)
				if armed:peek() and click then
					click(...)
				end
			end
			return ds.Button(props :: any)
		end
		-- The one button, by what it does now: the page's one primary action,
		-- its words saying which. None when there's nothing to do (installed,
		-- without settings of its own; taken down, which its callout turns back
		-- on).
		local function action(kind: string): Node?
			if kind == "install" or kind == "incompatible" or kind == "away" or kind == "other" then
				return primary {
					id = "store-install",
					disabled = function(): boolean
						return kind ~= "install" or not chosen_installable()
					end,
					busy = function(): boolean
						return busy:get()
					end,
					on_click = function()
						install(chosen_version())
					end,
					-- (A version a link chose: named, "Install 1.2.0"; never one
					-- the store hasn't.)
					function(): string
						local v = chosen_version()
						if v then
							return if here.device()
								then ui.t("plugins.store.install_version_on", { version = v, device = here.device_name() }):get()
								else ui.t("plugins.store.install_version", { version = v }):get()
						end
						return if here.device()
							then ui.t("plugins.store.install_on", { device = here.device_name() }):get()
							else ui.t("plugins.store.install"):get()
					end,
				}
			elseif kind == "update" then
				return primary {
					id = "store-update",
					busy = function(): boolean
						return updating:get()
					end,
					on_click = function()
						-- (Its offer read again as the sheet opens: whether the
						-- store has verified it, as of now, not of the last
						-- check hours ago.)
						state.check_updates(here.device(), id)
						update_open:set(true)
					end,
					function(): string
						local it: any = item()
						local v = if it and type(it.update) == "table" then it.update.version else nil
						return if type(v) == "string" and v ~= ""
							then ui.t("plugins.store.update_to", { version = v }):get()
							else ui.t("plugins.store.update"):get()
					end,
				}
			elseif kind == "review" then
				return primary {
					id = "store-review",
					on_click = function()
						state.review(id, here.device(), if here.device() then here.device_name() else nil)
					end,
					ui.t("plugins.page.review"),
				}
			elseif kind == "use" then
				return primary {
					id = "store-use",
					on_click = function()
						local it: any = item()
						local p: any = if it then state.use(it.id) else nil
						if p ~= nil then
							p:after(function(_: any) end, here.failed(say("plugins.page.couldnt_switch", { name = d().name or id })))
						end
					end,
					ui.t("templates.page.use"),
				}
			elseif kind == "off" then
				return primary { id = "store-turn-on", on_click = turn_on, ui.t("plugins.store.turn_on") }
			elseif kind == "settings" then
				return primary {
					id = "store-settings",
					on_click = function()
						plugin_settings.open(id)
					end,
					ui.t("plugins.store.open_settings"),
				}
			end
			return nil
		end

		-- Where it stands, in one line under the button (or in its place when
		-- there's none): what's here, what waits, why it can't install now.
		-- The plain Install says nothing: what it asks for says it's asked.
		local function said_here(plain: string, on: string): () -> string
			return function(): string
				return if here.device()
					then ui.t(on, { device = here.device_name() }):get()
					else ui.t(plain):get()
			end
		end
		local function state_words(kind: string): (() -> string)?
			if kind == "installed" or kind == "settings" or kind == "use" then
				return said_here("plugins.store.installed", "plugins.store.installed_on")
			elseif kind == "update" then
				return function(): string
					local it: any = item()
					local v = if it and type(it.version) == "string" then it.version else ""
					return if here.device()
						then ui.t("plugins.store.has_version_on", { device = here.device_name(), version = v }):get()
						else ui.t("plugins.store.has_version", { version = v }):get()
				end
			elseif kind == "review" then
				return function(): string
					return ui.t("plugins.store.review_note"):get()
				end
			elseif kind == "off" then
				return function(): string
					return ui.t("plugins.store.off_note"):get()
				end
			elseif kind == "away" then
				return function(): string
					return ui.t("plugins.store.away_note", { device = here.device_name() }):get()
				end
			end
			return nil
		end
		local function state_line(kind: string, align: string?): Node?
			local said = state_words(kind)
			if not said then
				return nil
			end
			local there = kind == "installed" or kind == "settings" or kind == "use"
			return ui.row {
				id = "store-state",
				min_w = 0,
				gap = "$space.1",
				items = "center",
				color = "$text.secondary",
				-- (There: the check, the small type's glyph.)
				if there then ds.Icon { name = "check-circle", size = 14, aria_hidden = true } else nil,
				-- (One line as short as its state; longer, a translation's or a
				-- device's name, it wraps inside the slot, right-aligned.)
				ui.text {
					said,
					min_w = 0,
					shrink = 1,
					wrap = true,
					align = align or "right",
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
				},
			}
		end
		-- The slot at the head's right: the button, its centre on the name's
		-- first line, and the line under it, both ending on the content edge;
		-- the line in the button's place when there's none. Its room is a row's
		-- control's, `zeron.settings.control`, in every state, so the name's
		-- column is one width for a window's width whatever the state (or the
		-- device's name in it) and never re-wraps as one changes to the next.
		local SLOT = ui.theme.get("zeron.settings.control") :: number?
		local function slot(kind: string): Node
			local button = action(kind)
			local line = state_line(kind)
			local top = button or line
			if not top then
				return ui.div { class = "store-slot-room", w = SLOT, shrink = 0 }
			end
			return ui.column {
				id = "store-slot",
				class = "store-slot-room",
				w = SLOT,
				shrink = 0,
				items = "end",
				gap = "$space.1",
				ui.row { min_w = 0, max_w = "100%", min_h = ui.theme.get("font.line.xl") :: number?, items = "center", top },
				if button and line then line else nil,
			}
		end

		-- The slot in a narrow pane (`narrow`): its own row under the meta line,
		-- on the name's text edge, the button first and the line beside it; the
		-- name then has the head's whole row.
		local function slot_under(kind: string): Node
			local button = action(kind)
			local line = state_line(kind, "left")
			if not button and not line then
				return ui.div {}
			end
			return ui.row {
				id = "store-slot",
				class = "store-slot-under",
				w_full = true,
				min_w = 0,
				gap = "$space.3",
				items = "center",
				button,
				line,
			}
		end
		-- Whether the name's column beside the slot would fall under a row's
		-- least for its words (Settings.Row's `MIN_TEXT`, 180, where its control
		-- goes under them): the head's width less its insets, the tile and its
		-- gap, and the gap and slot after the name.
		local head_w = ui.use_state(0)
		local function narrow(): boolean
			local w = head_w:get()
			if w <= 0 then
				return false
			end
			local gap = (ui.theme.get("space.4") :: number?) or 16
			local name = w - 2 * ds.Settings.TEXT_INSET - 48 - gap - gap - ((SLOT :: number?) or 240)
			return name < ds.Settings.MIN_TEXT
		end

		-- Why it can't install, where it takes more than a line: made for
		-- another Zeron (in the engine's words, when it gives them), or another
		-- plugin with its id installed from elsewhere, with the way to it.
		local function cannot(): Node
			local function kind(): string
				local k = standing()
				return if k == "incompatible" or k == "other" then k elseif template and item() == nil and other() then "other" else ""
			end
			return ui.show(function(): boolean
				return kind() ~= ""
			end, ui.div {
				id = "store-cannot",
				w_full = true,
				ui.switch(kind, {
					incompatible = ds.Callout {
						tone = "warning",
						title = ui.t("plugins.store.needs_other_zeron"),
						body = why_not,
					},
					other = ds.Callout {
						-- (In a plugin's way; a template installs beside it.)
						tone = if template then "info" else "warning",
						title = say("plugins.store.other_title"),
						body = say("plugins.store.other_body"),
						action = {
							label = ui.t("plugins.store.show_installed"),
							on_click = function()
								nav.tab:set("installed")
								nav.back()
							end,
						},
					},
					[""] = ui.div {},
				}),
			})
		end

		-- When the version the page offers isn't verified: a plain line saying
		-- so (the page informs; the sheets, where a person decides, warn), and
		-- the newest verified one, a link of its own to install it instead.
		local function verified_version(): string?
			local v = d().verified_version
			return if type(v) == "string" and v ~= "" and v ~= head_version() then v else nil
		end
		-- The sentence about the version a link chose, under the summary where
		-- the page's other sentences about a version are, and the way back to
		-- the newest (dropping the choice) wherever it isn't the newest.
		local function chosen_line(): Node
			return ui.show(function(): boolean
				return chosen_said() ~= ""
			end, ui.row {
				id = "store-chosen-line",
				w_full = true,
				px = ds.Settings.inset(),
				flex_wrap = true,
				items = "center",
				gap = "$space.2",
				ui.text {
					chosen_said,
					wrap = true,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					color = "$text.secondary",
				},
				ui.show(function(): boolean
					return chosen_version() ~= nil
				end, ds.Settings.InlineLink {
					role = "button",
					label = ui.t("plugins.store.show_newest"),
					on_click = function()
						nav.drop_linked_version()
					end,
				}),
			})
		end
		local function verified_line(): Node
			return ui.show(function(): boolean
				return head_verified() == false
			end, ui.row {
				id = "store-verified-line",
				w_full = true,
				px = ds.Settings.inset(),
				flex_wrap = true,
				items = "center",
				gap = "$space.2",
				ui.text {
					function(): string
						local v = verified_version()
						return if v
							then ui.t("plugins.store.not_verified_line", { version = head_version(), verified = v }):get()
							else say("plugins.store.none_verified"):get()
					end,
					wrap = true,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					color = "$text.secondary",
				},
				ui.show(function(): boolean
					local v = verified_version()
					local it: any = item()
					return v ~= nil and not here.away() and not in_the_way() and (it == nil or it.version ~= v)
				-- (The app's one look for a link ending a sentence, as a version
				-- row's Install: doing something here, a button.)
				end, ds.Settings.InlineLink {
					role = "button",
					label = function(): string
						return ui.t("plugins.store.install_instead", { version = verified_version() or "" }):get()
					end,
					disabled = function(): boolean
						return busy:get() or not armed:get()
					end,
					on_click = function()
						local v = verified_version()
						if v and armed:peek() then
							install(v)
						end
					end,
				}),
			})
		end

		-- What installing one is (a template): said here, beside Install.
		local function runs_as(): Node
			return ui.column {
				id = "store-runs-as",
				w_full = true,
				px = ds.Settings.inset(),
				gap = "$zeron.space.hair",
				ui.text {
					ui.t("templates.store.runs_as_zeron"),
					w_full = true,
					wrap = true,
					text_size = "$font.size.md",
					line_height = "$font.line.md",
					weight = "$font.weight.medium",
					color = "$text.primary",
				},
				ui.text {
					ui.t("templates.store.runs_as_zeron_body"),
					w_full = true,
					wrap = true,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					color = "$text.secondary",
				},
			}
		end

		-- Its keywords: each one narrows Browse to it.
		local function keywords(): Node
			local list: { string } = {}
			for _, k in (if type(first.keywords) == "table" then first.keywords else {}) :: { any } do
				if type(k) == "string" and k ~= "" and #list < 8 then
					table.insert(list, k)
				end
			end
			if #list == 0 then
				return ui.div {}
			end
			local chips: { Node } = {}
			for _, k in list do
				table.insert(chips, ds.Tag {
					function(): string
						return k
					end :: any,
					size = "sm",
					on_click = function()
						browse.show_keyword(k)
						nav.show_browse()
					end,
				})
			end
			return ui.row { id = "store-keywords", gap = "$space.2", flex_wrap = true, px = ds.Settings.inset(), table.unpack(chips) }
		end

		-- The page's head: its tile at the left of its name (as every row's
		-- is), who made it, its version and how many have it under the name,
		-- and the one thing to do at the right, ending on the content edge
		-- where the rows' controls end. Then what it says of itself, what
		-- installing it means here, and a version Zeron took down.
		local offer = ui.column {
			id = "store-offer",
			w_full = true,
			gap = "$space.3",
			ui.row {
				id = "store-header",
				w_full = true,
				px = ds.Settings.inset(),
				gap = "$space.4",
				items = "start",
				on_resize = function(size: { w: number, h: number })
					if math.abs(size.w - head_w:peek()) >= 0.5 then
						head_w:set(size.w)
					end
				end,
				-- (A template's picture is its face: no tile.)
				if kind.cards then nil else ui.div {
					id = "store-tile",
					shrink = 0,
					browse.Tile {
						card = function(): client.Card
							return d()
						end,
						size = 48,
					},
				},
				ui.column {
					grow = 1,
					basis = 0,
					min_w = 0,
					gap = "$space.3",
					ui.column {
						w_full = true,
						gap = "$space.1",
						-- (The whole name, however long: on as many lines as it takes.)
						ui.text {
							function(): string
								return d().name or id
							end,
							heading = 1,
							w_full = true,
							-- (The whole of it, on as many lines as it takes: a word
							-- longer than the line breaks to fit.)
							wrap = true,
							text_size = "$font.size.xl",
							line_height = "$font.line.xl",
							weight = "$font.weight.semibold",
							color = "$text.primary",
						},
						ui.row {
							gap = "$space.2",
							items = "center",
							flex_wrap = true,
							-- (Who made it: a link to their plugins in Browse, as a
							-- keyword narrows it to its own.)
							ui.div {
								id = "store-author",
								ds.Link {
									function(): string
										return browse.author(d())
									end,
									size = "sm",
									tone = "quiet",
									on_click = function()
										local a: any = d().author
										local name = if type(a) == "table" and type(a.username) == "string" then a.username else ""
										if name ~= "" then
											browse.show_author(name)
											nav.show_browse()
										end
									end,
								},
							},
							-- (The mark from the version it speaks of: a chosen one's
							-- own record.)
							browse.badges(function(): any
								if chosen_version() == nil then
									return d()
								end
								return { verified = head_verified() == true, author = d().author }
							end),
						},
						-- Its version, the day that version came out (the Versions
						-- list's day for it) and how many have it.
						ui.text {
							function(): string
								local n = d().installs
								local count = if type(n) == "number" then math.max(0, math.floor(n)) else 0
								-- (A version a link chose: its own number and day.)
								local version = head_version()
								local at: number? = nil
								local c = chosen:get()
								if chosen_version() and c.state == "ok" and type(c.record.published_at) == "number" then
									at = c.record.published_at
								end
								for _, v in (if at == nil and type(d().versions) == "table" then d().versions else {}) :: { any } do
									if type(v) == "table" and v.version == version and type(v.published_at) == "number" then
										at = v.published_at
									end
								end
								if at then
									return ui.t("plugins.store.version_meta", { version = version, date = locale.date(at / 1000), n = count }):get()
								end
								return ui.t("plugins.store.version_installs", { version = version, n = count }):get()
							end,
							tabular = true,
							text_size = "$font.size.sm",
							line_height = "$font.line.sm",
							color = "$text.tertiary",
						},
					},
					-- (Narrow: the slot under the meta line, 12 below it.)
					ui.show(function(): boolean
						return narrow() and standing() ~= "blocked"
					end, ui.switch(standing, {
						install = slot_under("install"),
						incompatible = slot_under("incompatible"),
						away = slot_under("away"),
						other = slot_under("other"),
						update = slot_under("update"),
						review = slot_under("review"),
						off = slot_under("off"),
					use = slot_under("use"),
						settings = slot_under("settings"),
						installed = slot_under("installed"),
						blocked = ui.div {},
					})),
				},
				-- (The name's column ends a section's gap, 16, before it.)
				ui.show(function(): boolean
					return not narrow()
				end, ui.switch(standing, {
					install = slot("install"),
					incompatible = slot("incompatible"),
					away = slot("away"),
					other = slot("other"),
					update = slot("update"),
					review = slot("review"),
					off = slot("off"),
				use = slot("use"),
					settings = slot("settings"),
					installed = slot("installed"),
					blocked = slot("blocked"),
				})),
			},
			ui.show(function(): boolean
				local summary = d().summary
				return type(summary) == "string" and summary ~= ""
			end, ui.text {
				id = "store-summary",
				function(): string
					return d().summary or ""
				end,
				px = ds.Settings.inset(),
				w_full = true,
				wrap = true,
				text_size = "$font.size.md",
				line_height = 20,
				color = "$text.secondary",
			}),
			if kind.fact then runs_as() else nil,
			chosen_line(),
			verified_line(),
			keywords(),
			cannot(),
			-- A version Zeron took down: why, and the way back.
			ui.show(function(): boolean
				return standing() == "blocked"
			end, ds.Callout {
				tone = "warning",
				title = function(): string
					local b = blocked()
					local v = if b and type(b.version) == "string" then b.version else nil
					return if v
						then ui.t("plugins.store.blocked_title_version", { name = d().name or id, version = v }):get()
						else ui.t("plugins.store.blocked_title", { name = d().name or id }):get()
				end,
				body = function(): string
					local b = blocked()
					local why = if b and type(b.reason) == "string" and b.reason ~= "" then string.sub(b.reason, 1, 500) else nil
					return if why
						then say("plugins.store.blocked_body_reason", { reason = why }):get()
						else say("plugins.store.blocked_body"):get()
				end,
				action = if template then nil else {
					label = ui.t("plugins.store.turn_back_on"),
					on_click = function()
						if not unblocking:peek() then
							unblock_open:set(true)
						end
					end,
				},
			}),
		}

		-- Whether the device shown takes its pre-releases (the engine's choice,
		-- for this plugin on that device): there once the plugin is.
		local pre_on = ui.use_state(false)
		ui.effect(function()
			local it: any = item()
			local want = it ~= nil and it.prerelease == true
			if pre_on:peek() ~= want then
				pre_on:set(want)
			end
		end)
		-- Which updates it takes on the device shown: the same as this
		-- computer's choice (Installed's), the newest version, or only versions
		-- the store has verified. This device's from what the engine keeps;
		-- another device's, its page's word for it (no "same as this computer"
		-- there: the page says what's in effect).
		local function local_prefs(): any
			return if here.device() == nil then state.store_updates:get() else nil
		end
		local function chosen_updates(): string
			local prefs = local_prefs()
			if prefs then
				local own = if type(prefs.plugins) == "table" then prefs.plugins[id] else nil
				return if own == "newest" or own == "verified" then own else "device"
			end
			return if type(first.installed) == "table" and first.installed.updates == "verified" then "verified" else "newest"
		end
		local updates = ui.use_state(chosen_updates())
		ui.effect(function()
			local now = chosen_updates()
			if updates:peek() ~= now then
				updates:set(now)
			end
		end)
		-- (A choice kept: what a template's list says of it, asked again.)
		local function chosen_kept(_: any)
			if template then
				state.check_updates()
			end
		end
		local function update_options(): { any }
			local out: { any } = {}
			local prefs = local_prefs()
			if prefs then
				table.insert(out, { value = "device", label = ui.t("plugins.store.updates_device"):get() })
			end
			table.insert(out, { value = "newest", label = ui.t("plugins.store.updates_newest"):get() })
			table.insert(out, { value = "verified", label = ui.t("plugins.store.updates_verified"):get() })
			return out
		end
		local prerelease = ui.show(function(): boolean
			local it: any = item()
			return it ~= nil and type(it.source) == "table" and it.source.kind == "store"
		end, ds.Settings.Group {
			ds.Settings.Row {
				label = ui.t("plugins.store.updates_label"),
				description = say("plugins.store.updates_body"),
				-- (What's in effect, under it: following this computer, which
				-- choice that is, named once, here. Its own choice: none.)
				detail = function(): string?
					local prefs = local_prefs()
					if prefs and updates:get() == "device" then
						return if prefs.device == "verified"
							then ui.t("plugins.store.updates_follows_verified"):get()
							else ui.t("plugins.store.updates_follows_newest"):get()
					end
					return nil
				end,
				control = ui.div {
					id = "store-updates",
					ds.Select {
						value = updates,
						size = "sm",
						w = 200,
						name = ui.t("plugins.store.updates_label"),
						options = update_options,
						on_change = function(v: any)
							local was = chosen_updates()
							store.set_updates(id, if v == "device" then nil else v, here.device()):after(chosen_kept, function(err: any)
								updates:set(was)
								here.failed(say("plugins.store.couldnt_updates", { name = d().name or id }))(err)
							end)
						end,
					},
				},
			},
			ds.Settings.Row {
					label = ui.t("plugins.store.prereleases"),
					description = say("plugins.store.prereleases_body"),
					control = ds.Switch {
						value = pre_on,
						label = ui.t("plugins.store.prereleases"),
						on_change = function(v: boolean)
							store.prereleases(id, v, here.device()):after(chosen_kept, function(err: any)
								pre_on:set(not v)
								here.failed(say("plugins.store.couldnt_prereleases", { name = d().name or id }))(err)
							end)
						end,
					},
				},
		})

		-- The facts: who, which version, how many, where from, what it needs.
		local function facts(): Node
			-- (Who, which version and how many are the head's; a version that
			-- doesn't work here is said under it, once.)
			local items: { any } = {}
			if first.compatible ~= false then
				table.insert(items, {
					key = ui.t("plugins.store.fact.works_with"),
					value = function(): string
						return ui.t("plugins.store.works_here"):get()
					end,
				})
			end
			if type(first.categories) == "table" and #first.categories > 0 then
				local names: { string } = {}
				for _, c in first.categories do
					if type(c) == "string" then
						local name: string = browse.category_name(c)
						table.insert(names, name)
					end
				end
				local said = table.concat(names, ui.t("plugins.words.list_sep"):get())
				table.insert(items, {
					key = ui.t("plugins.store.fact.category"),
					value = function(): string
						return said
					end,
				})
			end
			if type(first.license) == "string" and first.license ~= "" then
				local license = string.sub(first.license, 1, 64)
				table.insert(items, {
					key = ui.t("plugins.store.fact.license"),
					value = function(): string
						return license
					end,
				})
			end
			for _, link in { { "homepage", "plugins.store.fact.homepage" }, { "repository", "plugins.store.fact.repository" } } do
				local url = web(first[link[1]])
				if url then
					local u = url :: string
					table.insert(items, {
						key = ui.t(link[2]),
						value = ds.Link {
							function(): string
								return shown_url(u)
							end :: any,
							external = true,
							href = u,
							size = "sm",
							tone = "quiet",
							-- (A long one ends in "…" inside the card; the link and
							-- its tooltip are the whole of it.)
							truncate = true,
							tooltip = u,
							on_click = function()
								Links.open(u)
							end,
						},
					})
				end
			end
			if #items == 0 then
				return ui.div {}
			end
			return ds.Settings.Section {
				title = ui.t("plugins.store.details"),
				ds.Settings.Group {
					ui.column { id = "store-facts", w_full = true, px = "$space.4", py = "$space.3", ds.KeyValue { items = items, key_width = 96 } },
				},
			}
		end

		-- The README: the app's Markdown, its pictures the package's own. A
		-- long one (the store takes 512 KB), or one built to cost, is drawn a
		-- piece at a time (screens/plugins/untrusted: pieces by cost and size,
		-- plain text past its budget): the first at once, the rest as asked
		-- (Show more), so no frame lays out half a megabyte, and no text makes
		-- the page fail to draw.
		local readme_text = if type(first.readme) == "string" then string.sub(first.readme, 1, 512 * 1024) else ""
		local function picture(src: string): any
			-- A picture of its package, through the store; anything else is its
			-- link (`false`), never fetched from where it points.
			local path = package_path(src)
			if not path then
				return false
			end
			return store.picture_src(id, version, path :: string)
		end
		-- Whether a heading's words are the plugin's name (its case and spaces
		-- aside).
		local function plain(text: string): string
			return string.lower((string.gsub(string.gsub(text, "%s+", " "), "^ ?(.-) ?$", "%1")))
		end
		local function same_name(heading: string): boolean
			return plain(heading) == plain(d().name or id)
		end
		-- (Folded: its start alone, and whether that's all of it, known as the
		-- page is built (a scan of at most `ABOUT_LINES` lines); open: the whole
		-- first piece, the rest a piece at a time as asked.)
		local about_whole = (function(): boolean
			local c = untrusted.cutter(readme_text, ABOUT_LINES)
			c:next()
			return c.done
		end)()
		local pieces_shown = untrusted.handle()
		local function readme(): Node
			local line = (ui.theme.get("zeron.prose.line") :: number?) or 24
			return ds.Settings.Section {
				title = ui.t("plugins.store.about"),
				ds.Settings.Group {
					ui.column {
						id = "store-readme",
						w_full = true,
						px = "$space.4",
						py = "$space.4",
						Fold {
							height = ABOUT_FOLD,
							slack = ABOUT_SLACK,
							fade = line * FADE_LINES,
							whole = function(): boolean
								return about_whole
							end,
							body = function(open: boolean): Node
								return untrusted.View {
									text = readme_text,
									what = "readme",
									image = picture,
									-- (Under the page's own title: its headings a level
									-- down, none larger than the name; a `# Name` that
									-- opens it, the name again, left out; its pictures
									-- with the page's corner.)
									heading_step = 1,
									drop_title = same_name,
									picture_radius = "$radius.lg",
									first_lines = if open then nil else ABOUT_LINES,
									only_first = not open,
									handle = if open then pieces_shown else nil,
								}
							end,
							next = pieces_shown,
							class = "store-about",
							button_class = "store-about-fold",
							next_id = "store-readme-more",
						},
					},
				},
			}
		end

		-- Its screenshots, each opening the set in the lightbox: 16:10, with
		-- the cards' own corner (Settings.Group's, `$radius.xl`): they stand on
		-- the cards' edges as the cards' peers. (A README's pictures, 16 inside
		-- a card, keep the smaller corner concentric there.) A shelf
		-- (ds/shelf): one fills the column (no taller than a reply's picture,
		-- `zeron.prose.picture.h`, 420); two share it, 12 apart; three or more
		-- show two whole and the third at the column's end, to say there are
		-- more.
		local function shots(): Node
			local list: { string } = {}
			if kind.cards and type(first.cover) == "string" and package_path(first.cover) then
				table.insert(list, package_path(first.cover) :: string)
			end
			for _, s in (if type(first.screenshots) == "table" then first.screenshots else {}) :: { any } do
				if type(s) == "string" and package_path(s) and #list < 8 then
					table.insert(list, package_path(s) :: string)
				end
			end
			if #list == 0 then
				return ui.div {}
			end
			local radius = (ui.theme.get("radius.xl") :: number?) or 12
			local function items(): { any }
				local out = {}
				for _, path in list do
					local src = store.picture_src(id, version, path)
					if type(src) == "string" then
						table.insert(out, { src = src, name = string.match(path, "([^/]+)$") or path, key = path })
					end
				end
				return out
			end
			-- One picture in the box it's given: its wash while it comes, then
			-- it, clipped to the corner with the hairline a picture has.
			local function picture_in(path: string): Node
				return ui.presence(function(): string
					local src = store.picture_src(id, version, path)
					return if src == false then "\0none" else (src or "")
				end, function(src: string): Node
					if src == "" then
						return ui.div { w_full = true, h_full = true, rounded = "$radius.xl", overflow = "hidden", ds.Skeleton { fill = true } }
					elseif src == "\0none" then
						return ui.div {}
					end
					local index = 1
					for i, it in items() do
						if it.key == path then
							index = i
						end
					end
					return Lightbox.Thumb {
						group = `store:{id}`,
						index = index,
						items = items,
						radius = radius,
						w_full = true,
						h_full = true,
						rounded = "$radius.xl",
						ui.overlay {
							w_full = true,
							h_full = true,
							rounded = "$radius.xl",
							overflow = "hidden",
							border = 1,
							border_color = "$border",
							border_over = true,
							bg = "$hover",
							ui.image { src = src, w_full = true, h_full = true, fit = "cover", aria_hidden = true },
						},
					}
				end, { w_full = true, h_full = true })
			end
			-- A template's: its preview first, the content's width, and the rest
			-- under it three across (two in a narrow pane).
			if kind.cards then
				local rest: { string } = table.clone(list)
				local lead = table.remove(rest, 1) :: string
				return ui.column {
					id = "store-shots",
					w_full = true,
					gap = "$space.3",
					ui.show(function(): boolean
						return store.picture_src(id, version, lead) ~= false
					end, ui.div { class = "store-shot", w_full = true, aspect = 1.6, picture_in(lead) }),
					if #rest == 0
						then nil
						else ds.Settings.Cards {
							id = "store-thumbs",
							min_w = 160,
							least = 2,
							-- (Two or four in pairs: a fourth alone on a row looks lost.)
							most = if #rest == 2 or #rest == 4 then 2 else 3,
							build = function(w: () -> number): Node
								-- (Its height in points: an aspect's isn't one a wrapped
								-- row's picture fills.)
								local function h(): number
									return math.floor(w() / 1.6)
								end
								local thumbs: { Node } = {}
								for _, path in rest do
									table.insert(thumbs, ui.div { class = "store-shot", w = w, h = h, shrink = 0, picture_in(path) })
								end
								return ui.row { w_full = true, flex_wrap = true, items = "start", gap = "$space.3", table.unpack(thumbs) }
							end,
						},
				}
			end
			-- (The shelf's rule, as Browse's featured: one fills the column, two
			-- share it, three or more show two whole and the third begun.)
			local one = #list == 1
			return ds.Shelf {
				id = "store-shots",
				across = 2,
				count = function(): number
					return #list
				end,
				build = function(w: () -> number): any
					local thumbs: { Node } = {}
					for _, path in list do
						table.insert(thumbs, ui.div {
							class = "store-shot",
							w = w,
							shrink = 0,
							aspect = 1.6,
							-- (One alone: no taller than a reply's picture.)
							max_h = if one then ui.theme.get("zeron.prose.picture.h") :: number? else nil,
							picture_in(path),
						})
					end
					return thumbs
				end,
			}
		end

		-- What it asks for, as the consent sheet says it (the sheet asks again,
		-- as it installs: this is what to expect).
		local function asks(): Node
			return ui.presence(function(): string
				local p = d().package
				return if type(p) == "table" and type(p.hash) == "string" then p.hash else tostring(d().version)
			end, function(_: string): Node
				-- (The store's consent item: what it asks, and what each names.)
				local perm: any = d().permissions
				local asked = if type(perm) == "table" and type(perm.asks) == "table" then perm.asks else {}
				local lines = words.lines(if type(perm) == "table" then perm else {}, asked)
				local body: Node
				if #lines == 0 then
					body = ui.text { ui.t("plugins.consent.asks_nothing"), text_size = "$font.size.md", color = "$text.secondary", wrap = true }
				else
					local list: { Node } = {}
					for _, l in lines do
						table.insert(list, asks_view.line(l))
					end
					body = ui.column { w_full = true, gap = "$space.3", table.unpack(list) }
				end
				return ds.Settings.Section {
					title = ui.t("plugins.store.asks"),
					description = ui.t("plugins.store.asks_note"),
					ds.Settings.Group {
						ui.column { id = "store-asks", w_full = true, px = "$space.4", py = "$space.4", body },
					},
				}
			end)
		end

		-- Its versions, newest first: each one's date and what changed, kept to
		-- a bound so Report is always near. The latest's notes to about ten
		-- lines, Show more opening the rest in place; each older one's first
		-- line, opening whole in place; the latest few, Show all the rest.
		-- (The detail's first page, then the rest by cursor once asked for.)
		local more_versions = ui.use_state({} :: { any })
		local versions_cursor = ui.use_state((first.versions_next_cursor :: string?))
		local versions = ui.computed(function(): { client.Version }
			local out: { client.Version } = {}
			local seen: { [string]: boolean } = {}
			local function add(list: any)
				for _, v in (if type(list) == "table" then list else {}) :: { any } do
					if type(v) == "table" and type(v.version) == "string" and not seen[v.version] and (v.status == nil or v.status == "listed" or v.status == "yanked") then
						seen[v.version] = true
						table.insert(out, v)
					end
				end
			end
			add(d().versions)
			add(more_versions:get())
			return out
		end)
		local function fetch_versions(cursor: string)
			store.versions(id, cursor, 50):after(function(r: any)
				local now = table.clone(more_versions:peek())
				for _, v in (if type(r) == "table" and type(r.versions) == "table" then r.versions else {}) :: { any } do
					table.insert(now, v)
				end
				more_versions:set(now)
				local next = if type(r) == "table" and type(r.next_cursor) == "string" and r.next_cursor ~= "" then r.next_cursor else nil
				versions_cursor:set(next)
				if next then
					fetch_versions(next)
				end
			end, function(err: any)
				here.failed(ui.t("plugins.store.versions_failed_toast"))(err)
			end)
		end
		local all_versions = ui.use_state(false)
		local listed = ui.computed(function(): { client.Version }
			local all = versions:get()
			if all_versions:get() or #all <= VERSIONS_FIRST then
				return all
			end
			return table.move(all, 1, VERSIONS_FIRST, 1, {})
		end)
		-- What a version's row offers at its line's end: "installed" (the one
		-- this device has), "install" (one it can take here), else "none".
		local function version_action(v: any): string
			local it: any = item()
			if it ~= nil and not in_the_way() and it.version == v.version then
				return "installed"
			end
			return if browse.version_state(v) == "listed" and not here.away() and not in_the_way() and compatible() then "install" else "none"
		end
		local function version_row(first_v: client.Version, _i: number, now: State<client.Version>): Node
			local latest = (versions:peek()[1] or {}).version == first_v.version
			local function notes(): string
				local n = now:get().notes
				return if type(n) == "string" then string.sub(n, 1, NOTES_MOST) else ""
			end
			return ui.column { class = "store-version", w_full = true, ds.Settings.Row {
				label = function(): string
					return now:get().version
				end,
				-- (As every list of versions has it, Publish's too: the public's
				-- view, a version nobody verified "Not verified yet".)
				badge = browse.version_badge(function(): any
					return now:get()
				end),
				description = function(): string
					local at = now:get().published_at
					return if type(at) == "number" then locale.date(at / 1000) else ""
				end,
				-- Why it takes no new installs when it was replaced, when the
				-- store verified it (its badge says whether), and whether it's
				-- the one this device has; any other listed one, installed from
				-- here.
				detail = function(): string?
					local v: any = now:get()
					local parts: { string } = {}
					-- (Replaced before the store verified it, as the store says.)
					if browse.version_state(v) == "yanked" and v.yanked_by == "superseded" and type(v.superseded_by) == "string" and v.superseded_by ~= "" then
						table.insert(parts, ui.t("plugins.store.superseded", { version = v.superseded_by }):get())
					end
					if v.verified == true and type(v.verified_at) == "number" then
						table.insert(parts, ui.t("plugins.store.verified_on", { date = locale.date(v.verified_at / 1000) }):get())
					end
					return if #parts > 0 then table.concat(parts, " · ") else nil
				end,
				-- At its first line's end: Install on every row that can be
				-- installed, verified or not (the newest's too, beside the head's
				-- button); "Installed" there on the one this device has.
				name_action = ui.switch(function(): string
					return version_action(now:get())
				end, {
					none = ui.div {},
					install = ds.Settings.InlineLink {
						-- (Doing something here: a button, as Installed's Update.)
						role = "button",
						label = ui.t("plugins.store.install"),
						disabled = function(): boolean
							return busy:get() or not armed:get()
						end,
						on_click = function()
							if armed:peek() then
								install(now:peek().version)
							end
						end,
					},
					installed = ui.text {
						ui.t("plugins.store.installed"),
						class = "store-version-installed",
						text_size = "$font.size.sm",
						line_height = "$font.line.sm",
						weight = "$font.weight.medium",
						color = "$text.secondary",
					},
				}),
				stacked = true,
				control = ui.show(function(): boolean
					return notes() ~= ""
				-- (Folded from far down a long one, its own control is brought
				-- back into view, focused: the fold's way.)
				end, Notes { text = notes, lines = if latest then NOTES_LINES else 1 }),
			} }
		end
		local function history(): Node
			return ds.Settings.Section {
				title = ui.t("plugins.store.versions"),
				ds.Settings.Each(listed, version_row, {
					key = function(v: client.Version): string
						return v.version
					end,
				}),
				ui.show(function(): boolean
					return not all_versions:get() and (#versions:get() > VERSIONS_FIRST or versions_cursor:get() ~= nil)
				end, ui.row {
					px = ds.Settings.inset(),
					FoldButton {
						-- (How many, once the store has said: more pages to come,
						-- "All versions" alone.)
						label = function(): string
							return if versions_cursor:get() ~= nil
								then ui.t("plugins.store.all_versions_plain"):get()
								else ui.t("plugins.store.all_versions", { n = #versions:get() }):get()
						end,
						open = function(): boolean
							return all_versions:get()
						end,
						on_click = function()
							all_versions:set(true)
							local c = versions_cursor:peek()
							if c then
								fetch_versions(c)
							end
						end,
						class = "store-all-versions",
					},
				}),
			}
		end

		-- What changes with the update, read before it: the changelog and what
		-- it asks for more or no longer.
		-- The newest verified version an unverified update offers beside it
		-- (the offer's own `verified_alternative`), else nil.
		local function verified_alternative(): string?
			local it: any = item()
			local u = if it then it.update else nil
			local alt = if type(u) == "table" and type(u.verified_alternative) == "table" then u.verified_alternative.version else nil
			return if type(alt) == "string" and alt ~= "" then alt else nil
		end
		local function update_dialog(): Node
			local function notes(): string
				local it: any = item()
				local u = if it then it.update else nil
				return if type(u) == "table" and type(u.notes) == "string" then string.sub(u.notes, 1, 65536) else ""
			end
			local function changed(which: string): { words.Line }
				local it: any = item()
				local u = if it then it.update else nil
				local list = if type(u) == "table" and type(u[which]) == "table" then u[which] else {}
				local perm: any = d().permissions
				return words.lines(if type(perm) == "table" then perm else {}, list)
			end
			return ds.Dialog {
				open = update_open,
				title = function(): string
					local it: any = item()
					local v = if it and type(it.update) == "table" then it.update.version else nil
					return if type(v) == "string"
						then ui.t("plugins.store.update_title", { name = d().name or id, version = v }):get()
						else ui.t("plugins.store.update_title_plain", { name = d().name or id }):get()
				end,
				body = say("plugins.store.update_body"),
				confirm = {
					label = ui.t("plugins.store.update"),
					on_click = update,
				},
				ui.scroll {
					id = "store-update-scroll",
					w_full = true,
					max_h = 320,
					fade = { 12, 12 },
					fade_overflow = true,
					ui.column {
						w_full = true,
						gap = "$space.4",
						-- A new publisher, by name (asked again after it updates), a
						-- version signed with another key than yours (said, never in
						-- the way: the store took it), and a version the store
						-- hasn't verified (a warning, here where the person
						-- decides: from the offer's own record, with the verified
						-- one to take instead where there is one).
						ui.show(function(): boolean
							local it: any = item()
							local u = if it then it.update else nil
							return type(u) == "table" and (u.new_owner == true or u.new_key == true or u.verified == false)
						end, ui.column {
							w_full = true,
							gap = "$space.2",
							ui.show(function(): boolean
								local it: any = item()
								return it ~= nil and type(it.update) == "table" and it.update.new_owner == true
							end, ds.Callout {
								tone = "warning",
								title = function(): string
									local a = d().author
									local name = if type(a) == "table" and type(a.username) == "string" and a.username ~= "" then a.username else nil
									return if name
										then ui.t("plugins.store.update_new_owner", { username = name }):get()
										else ui.t("plugins.store.update_new_owner_unnamed"):get()
								end,
							}),
							ui.show(function(): boolean
								local it: any = item()
								return it ~= nil and type(it.update) == "table" and it.update.new_key == true
							end, ds.Callout {
								tone = "warning",
								title = ui.t("plugins.store.update_new_key"),
							}),
							-- (Keyed by what it says: its words and its action are
							-- the offer's.)
							ui.presence(function(): string
								local it: any = item()
								local u = if it then it.update else nil
								if type(u) ~= "table" or u.verified ~= false then
									return ""
								end
								local had = if u.installed_verified == true and type(it.version) == "string" then it.version else ""
								return `{u.version or ""}|{had}|{verified_alternative() or ""}`
							end, function(key: string): Node
								if key == "" then
									return ui.div {}
								end
								local v, had, alt = string.match(key, "^([^|]*)|([^|]*)|([^|]*)$")
								return ui.div {
									class = "store-update-unverified",
									w_full = true,
									ds.Callout {
										tone = "warning",
										title = ui.t("plugins.store.not_verified_yet", { version = v or "" }),
										-- (What follows Update, said before it: the consent
										-- window asks, as the sheet's other callouts say.)
										body = function(): string
											local asks = say("plugins.store.update_asks_first"):get()
											return if had and had ~= ""
												then ui.t("plugins.store.installed_was_verified", { installed = had }):get() .. " " .. asks
												else asks
										end,
										action = if alt and alt ~= ""
											then {
												label = ui.t("plugins.store.update_instead", { version = alt }),
												on_click = function()
													update_open:set(false)
													install(alt)
												end,
											}
											else nil,
									},
								}
							end, { w_full = true }),
						}),
						ui.show(function(): boolean
							return notes() ~= ""
						end, ui.column {
							w_full = true,
							gap = "$space.2",
							ui.text { ui.t("plugins.store.whats_new"), text_size = "$font.size.sm", weight = "$font.weight.medium", color = "$text.secondary" },
							-- (Bounded as the store's every text: screens/plugins/untrusted.)
							ui.presence(notes, function(text: string): Node
								return untrusted.View { text = text, what = "notes", small = true, auto = true }
							end, { w_full = true }),
						}),
						(if template
							then ui.column {
								w_full = true,
								gap = "$space.2",
								ds.Callout { tone = "warning", title = ui.t("templates.store.runs_as_zeron"), body = ui.t("templates.store.runs_as_zeron_body") },
								ui.show(function(): boolean
									local it: any = item()
									return it ~= nil and type(it.source) == "table" and (it.edited == true or it.source.edited == true)
								end, ds.Callout { id = "store-update-edits", tone = "warning", title = ui.t("templates.store.update_replaces_edits") }),
							}
							else ui.presence(function(): string
								local a, r = changed("added"), changed("removed")
								local key = {}
								for _, l in a do
									table.insert(key, "+" .. l.title)
								end
								for _, l in r do
									table.insert(key, "-" .. l.title)
								end
								return table.concat(key, "\n")
							end, function(_: string): Node
								local a, r = changed("added"), changed("removed")
								return ui.column {
									w_full = true,
									gap = "$space.4",
									if #a > 0 then asks_view.section(ui.t("plugins.store.asks_more"), a) else nil,
									if #r > 0 then asks_view.section(ui.t("plugins.store.asks_less"), r) else nil,
									if #a == 0 and #r == 0
										then ui.text { ui.t("plugins.store.asks_same"), text_size = "$font.size.sm", color = "$text.secondary", wrap = true }
										else nil,
								}
							end)) :: any,
					},
				},
			}
		end

		local function report_dialog(): Node
			local options = {}
			for _, r in store.REPORT_REASONS do
				table.insert(options, { value = r, label = ui.t(`plugins.store.report.reason.{r}`) })
			end
			return ds.Dialog {
				open = report_open,
				title = function(): string
					return say("plugins.store.report.title", { name = d().name or id }):get()
				end,
				body = say("plugins.store.report.body"),
				size = "sm",
				confirm = {
					label = ui.t("plugins.store.report.send"),
					on_click = function()
						local reason, text = report_reason:peek(), string.sub(report_text:peek(), 1, 2000)
						store.report(id, reason, text):after(function()
							report_text:set("")
							ds.toast.show { title = ui.t("plugins.store.report.sent"), tone = "success" }
						end, here.failed(ui.t("plugins.store.report.failed")))
					end,
				},
				ds.Select { value = report_reason, label = ui.t("plugins.store.report.reason_label"), options = options, w_full = true },
				ds.TextArea {
					value = report_text,
					name = ui.t("plugins.store.report.text_label"),
					placeholder = ui.t("plugins.store.report.text_placeholder"),
					rows = 3,
					max_rows = 6,
					w_full = true,
				},
			}
		end

		return ui.column {
			w_full = true,
			gap = "$space.6",
			ui.column {
				w_full = true,
				gap = "$space.6",
				offer,
				shots(),
				-- (What it asks for before its author's own words: what a person
				-- weighs first, never pushed down by a README.)
				if template then nil else asks(),
				readme(),
				facts(),
				prerelease,
				history(),
				ui.row {
					w_full = true,
					px = ds.Settings.inset(),
					ds.Link {
						say("plugins.store.report.link"),
						size = "sm",
						tone = "quiet",
						on_click = function()
							report_open:set(true)
						end,
					},
				},
			},
			update_dialog(),
			report_dialog(),
			-- Back on, once more asked: it was taken down for a reason.
			ds.Dialog {
				open = unblock_open,
				title = function(): string
					return ui.t("plugins.store.unblock_title", { name = d().name or id }):get()
				end,
				body = function(): string
					local b = blocked()
					local why = if b and type(b.reason) == "string" and b.reason ~= "" then string.sub(b.reason, 1, 500) else nil
					return if why then ui.t("plugins.store.unblock_body_reason", { reason = why }):get() else ui.t("plugins.store.unblock_body"):get()
				end,
				tone = "danger",
				confirm = {
					label = ui.t("plugins.store.turn_back_on"),
					on_click = unblock,
				},
			},
		}
	end)

	-- A store plugin's page: read as it opens (at once from the last read),
	-- or what stands in its place while it can't be.
	M.Page = ui.component(function(props: { plugin: string, here: Here }): Node
		local id = props.plugin
		local phase = ui.use_state(if details:peek()[id] then "found" else "loading")
		-- Another device's Zeron, as its presence says it, for the engine to
		-- judge it by (this one's it knows).
		local function for_zeron(): string?
			local device = props.here.device()
			if not device then
				return nil
			end
			for _, d in sessions.devices:peek() or {} do
				if d.id == device and type(d.version) == "string" and d.version ~= "" then
					return d.version
				end
			end
			return nil
		end
		local function load()
			store.detail(id, for_zeron()):after(function(found: any)
				if type(found) == "table" and found.id == id then
					details:set_key(id, found)
					phase:set("found")
				else
					phase:set("not_found")
				end
			end, function(err: any)
				local m = if type(err) == "table" then err.message else err
				local word = if type(m) == "string" then (string.match(m, "^([%l_]+)") or m) else ""
				-- (Not in the store any more: what was kept of it goes too.)
				if word == "not_in_store" then
					phase:set("not_found")
					details:set_key(id, nil)
					return
				end
				if details:peek()[id] then
					return
				end
				phase:set(if word == "offline" or word == "store_unreachable" then "offline" else "failed")
			end)
		end
		-- Asked to be read again (an install refused, a version taken down).
		local asked = rereads:peek()[id] or 0
		ui.effect(function()
			local n = rereads:get()[id] or 0
			if n ~= asked then
				asked = n
				load()
			end
		end)
		-- A version of it Zeron took down, as this device's list says: read
		-- again, never the mark it had.
		local was_blocked = false
		ui.effect(function()
			local blocked = false
			for _, p in state.shown:get() do
				local source: any = p.source
				if type(source) == "table" and source.kind == "store" and source.id == id and type((p :: any).blocked) == "table" then
					blocked = true
				end
			end
			if blocked and not was_blocked then
				reread(id)
			end
			was_blocked = blocked
		end)
		-- (As it shows, never as it builds.) Another device chosen: whether it
		-- works there is asked again.
		ui.on_mount(function()
			load()
		end)
		local shown_device: string? = props.here.device()
		ui.effect(function()
			local now = props.here.device()
			if now ~= shown_device then
				shown_device = now
				load()
			end
		end)
		local function body(key: string): Node
			local kind = if string.match(key, "^found:") then "found" else key
			if kind == "found" then
				return Found { plugin = id, here = props.here }
			elseif kind == "loading" then
				-- (Its head's shape, then a card's: as the page that comes.)
				return ds.SkeletonWaiting {
					build = function(still: boolean): Node
						return ui.column {
							id = if still then "store-loading-still" else "store-loading",
							w_full = true,
							gap = "$space.6",
							ui.row {
								w_full = true,
								px = ds.Settings.inset(),
								gap = "$space.4",
								items = "start",
								if kind.cards then nil else ds.Skeleton { w = 48, h = 48, rounded = "$radius.md", still = still },
								ui.column { grow = 1, gap = "$space.2", ds.Skeleton { w = 180, h = 20, still = still }, ds.Skeleton { w = 120, still = still }, ds.Skeleton { w = 160, still = still } },
								ds.Skeleton { w = 64, h = 28, rounded = "$radius.lg", still = still },
							},
							-- (The card draws the hairline between its rows.)
							ds.Settings.Group {
								ds.SkeletonRow { lead = 0, title = 160, line = 220, still = still },
								ds.SkeletonRow { lead = 0, title = 120, line = 260, still = still },
							},
						}
					end,
				}
			elseif kind == "not_found" then
				return said(ds.EmptyState {
					icon = "search",
					title = say("plugins.store.not_found_title"),
					body = say("plugins.store.not_found_body"),
					size = "sm",
					action = {
						label = say("plugins.store.browse_all"),
						on_click = function()
							nav.back()
						end,
					},
				})
			end
			return ds.Callout {
				tone = if kind == "offline" then "neutral" else "danger",
				icon = if kind == "offline" then "wifi-off" else nil,
				title = if kind == "offline" then say("plugins.store.offline_title") else say("plugins.store.failed_title"),
				body = if kind == "offline" then say("plugins.store.offline_body") else say("plugins.store.failed_body"),
				action = {
					label = ui.t("plugins.store.try_again"),
					on_click = function()
						phase:set("loading")
						load()
					end,
				},
			}
		end
		return ds.Settings.SubPage {
			where = ds.Settings.Back {
				label = ui.t(if template then "templates.page.title" else "plugins.page.title"),
				on_click = function()
					nav.back()
				end,
			},
			-- (Found, by the version it's of: a new version is another page,
			-- its README, pictures and asks all its own.)
			ui.presence(function(): string
				local p = phase:get()
				if p == "found" then
					local found = details:get()[id] or {}
					local v = found.version
					-- (And whether it works here: another device chosen may say
					-- otherwise, which the head and the facts say.)
					local why = if type(found.incompatible) == "string" then found.incompatible else ""
					return `found:{if type(v) == "string" then v else ""}:{tostring(found.compatible ~= false)}:{why}`
				end
				return p
			end, body, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
		}
	end)

	return M
end

return C
