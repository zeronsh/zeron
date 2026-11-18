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
local Markdown = require("@ds/markdown")
local locale = require("@ds/locale")
local store = require("@app/screens/plugins/store")
local state = require("@app/screens/plugins/state")
local words = require("@app/screens/plugins/words")
local asks_view = require("@app/screens/plugins/asks")
local nav = require("@app/screens/plugins/nav")
local browse = require("@app/screens/plugins/browse")
local plugin_settings = require("@app/screens/plugins/settings")
local sessions = require("@account/sessions")

local M = {}

export type Here = {
	device: () -> string?,
	device_name: () -> string,
	away: () -> boolean,
	settled: (plain: string, versioned: string) -> (done: any) -> (),
	failed: (title: string | Message) -> (err: any) -> (),
}

-- Each plugin's page as last read, by id: one opened again draws at once,
-- and is read again behind it.
local details = ui.state("store_details", {} :: { [string]: any })

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

-- A README cut into pieces of about `PIECE` bytes, each ending where a
-- paragraph does (a blank line), never inside a fenced code block.
local PIECE = 64 * 1024
local function readme_pieces(text: string): { string }
	local out: { string } = {}
	local piece: { string } = {}
	local size = 0
	local fenced = false
	for line in string.gmatch(text .. "\n", "([^\n]*)\n") do
		if string.match(line, "^%s*```") or string.match(line, "^%s*~~~") then
			fenced = not fenced
		end
		table.insert(piece, line)
		size += #line + 1
		if size >= PIECE and not fenced and string.match(line, "^%s*$") then
			table.insert(out, table.concat(piece, "\n"))
			piece = {}
			size = 0
		end
	end
	if #piece > 0 then
		table.insert(out, table.concat(piece, "\n"))
	end
	return out
end

local function said(node: Node): Node
	return ui.column { w_full = true, py = "$space.6", px = "$space.4", node }
end

-- A version's notes (its changelog, the store takes 64 KB of it), kept to
-- `lines` lines of the small prose while folded, their foot fading, with
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

local Notes = ui.component(function(props: { text: () -> string, lines: number }): Node
	local open = ui.use_state(false)
	local line = (ui.theme.get("zeron.prose.small.line") :: number?) or 20
	local folded_h = props.lines * line
	local body_h = ui.use_state(0)
	local function start(): (string, boolean)
		return start_of(props.text(), NOTES_START)
	end
	-- Whether there's more than the fold shows.
	local function more(): boolean
		local _, whole = start()
		if props.lines == 1 then
			local text = props.text()
			local rest = string.gsub(text, "^%s*[^\n]*", "", 1)
			return string.match(rest, "%S") ~= nil
		end
		return not whole or body_h:get() > folded_h + 0.5
	end
	local function folded(): boolean
		return not open:get() and more()
	end
	local body: Node
	if props.lines == 1 then
		-- (An older version's: its first line's words, then the whole.)
		body = ui.presence(function(): string
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
					color = "$text.secondary",
				}
			end
			return Markdown { text = props.text, small = true, foreign = true }
		end, { w_full = true })
	else
		body = ui.column {
			w_full = true,
			overflow = "hidden",
			max_h = function(): number
				return if folded() then folded_h else 1e7
			end,
			fade = function(): { number }
				return { 0, if folded() then line * FADE_LINES else 0 }
			end,
			ui.column {
				w_full = true,
				on_resize = function(size: { w: number, h: number })
					if math.abs(size.h - body_h:peek()) >= 0.5 then
						body_h:set(size.h)
					end
				end,
				Markdown {
					text = function(): string
						if open:get() then
							return props.text()
						end
						return (start())
					end,
					small = true,
					foreign = true,
				},
			},
		}
	end
	return ui.column {
		class = "store-notes",
		w_full = true,
		gap = "$space.2",
		body,
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
	local function d(): any
		return details:get()[id] or {}
	end
	local first: any = details:peek()[id] or {}
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

	-- What the device shown has of it: this store plugin (installed from
	-- the store); another plugin that has its id (from a file, a link)
	-- is never taken for it.
	local function item(): state.Item?
		for _, p in state.shown:get() do
			local source: any = p.source
			if p.id == id and type(source) == "table" and source.kind == "store" then
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
			elseif it.status == "pending" or it.status == "tampered" or (it.status == "off" and #(it.ask or {}) > 0) then
				return "review"
			elseif it.status == "off" then
				return "off"
			elseif it.status == "on" and here.device() == nil and plugin_settings.has(id) then
				return "settings"
			end
			return "installed"
		end
		if here.away() then
			return "away"
		elseif other() then
			return "other"
		elseif not compatible() then
			return "incompatible"
		end
		return "install"
	end

	local function install()
		if busy:peek() then
			return
		end
		busy:set(true)
		local p: any = state.install({ store = id, version = d().version, device = here.device() } :: any)
		if p == nil then
			busy:set(false)
			return
		end
		p:after(function(done: any)
			busy:set(false)
			here.settled("plugins.page.installed_toast", "plugins.page.installed_toast_version")(done)
		end, function(err: any)
			busy:set(false)
			here.failed(ui.t("plugins.page.couldnt_install"))(err)
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
			here.failed(ui.t("plugins.page.couldnt_update", { name = name }))(err)
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
	local function primary(props: { [any]: any }): Node
		props.tone, props.size, props.class = "primary", "sm", "store-action"
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
				disabled = kind ~= "install",
				busy = function(): boolean
					return busy:get()
				end,
				on_click = install,
				function(): string
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
		if kind == "installed" or kind == "settings" then
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
	local function state_line(kind: string): Node?
		local said = state_words(kind)
		if not said then
			return nil
		end
		local there = kind == "installed" or kind == "settings"
		return ui.row {
			id = "store-state",
			min_w = 0,
			gap = "$space.1",
			items = "center",
			color = "$text.secondary",
			-- (There: the check, the small type's glyph.)
			if there then ds.Icon { name = "check-circle", size = 14, aria_hidden = true } else nil,
			ui.text {
				said,
				min_w = 0,
				shrink = 1,
				lines = 1,
				truncate = true,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
			},
		}
	end
	-- The slot at the head's right: the button, its centre on the name's
	-- first line, and the line under it, both ending on the content edge;
	-- the line in the button's place when there's none. No wider than a
	-- row's control, so a long device name ends in "…" there.
	local function slot(kind: string): Node
		local button = action(kind)
		local line = state_line(kind)
		local top = button or line
		if not top then
			return ui.div {}
		end
		return ui.column {
			id = "store-slot",
			shrink = 0,
			max_w = ui.theme.get("zeron.settings.control") :: number?,
			items = "end",
			gap = "$space.1",
			ui.row { min_w = 0, max_w = "100%", min_h = ui.theme.get("font.line.xl") :: number?, items = "center", top },
			if button and line then line else nil,
		}
	end

	-- Why it can't install, where it takes more than a line: made for
	-- another Zeron (in the engine's words, when it gives them), or another
	-- plugin with its id installed from elsewhere, with the way to it.
	local function cannot(): Node
		local function kind(): string
			local k = standing()
			return if k == "incompatible" or k == "other" then k else ""
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
					tone = "warning",
					title = ui.t("plugins.store.other_title"),
					body = ui.t("plugins.store.other_body"),
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
					nav.back()
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
			ui.div {
				id = "store-tile",
				shrink = 0,
				browse.Tile {
					card = function(): store.Card
						return d()
					end,
					size = 48,
				},
			},
			ui.column {
				grow = 1,
				basis = 0,
				min_w = 0,
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
					ui.text {
						function(): string
							return browse.author(d())
						end,
						text_size = "$font.size.sm",
						line_height = "$font.line.sm",
						color = "$text.secondary",
					},
					browse.badges(d),
				},
				-- Its version, the day that version came out (the Versions
				-- list's day for it) and how many have it.
				ui.text {
					function(): string
						local n = d().installs
						local count = if type(n) == "number" then math.max(0, math.floor(n)) else 0
						local version = d().version or ""
						local at: number? = nil
						for _, v in (if type(d().versions) == "table" then d().versions else {}) :: { any } do
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
			-- (The name's column ends a section's gap, 16, before it.)
			ui.switch(standing, {
				install = slot("install"),
				incompatible = slot("incompatible"),
				away = slot("away"),
				other = slot("other"),
				update = slot("update"),
				review = slot("review"),
				off = slot("off"),
				settings = slot("settings"),
				installed = slot("installed"),
				blocked = slot("blocked"),
			}),
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
					then ui.t("plugins.store.blocked_body_reason", { reason = why }):get()
					else ui.t("plugins.store.blocked_body"):get()
			end,
			action = {
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
	local prerelease = ui.show(function(): boolean
		local it: any = item()
		return it ~= nil and type(it.source) == "table" and it.source.kind == "store"
	end, ds.Settings.Group {
		ds.Settings.Row {
			label = ui.t("plugins.store.prereleases"),
			description = ui.t("plugins.store.prereleases_body"),
			control = ds.Switch {
				value = pre_on,
				label = ui.t("plugins.store.prereleases"),
				on_change = function(v: boolean)
					store.prereleases(id, v, here.device()):after(function(_: any) end, function(err: any)
						pre_on:set(not v)
						here.failed(ui.t("plugins.store.couldnt_prereleases", { name = d().name or id }))(err)
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
					table.insert(names, browse.category_name(c))
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
	-- long one (the store takes 512 KB) is drawn a piece at a time, each
	-- piece ending at a paragraph's end outside any code block: the first
	-- at once, the rest as asked (Show more), so no frame lays out half a
	-- megabyte of prose.
	local pieces = readme_pieces(if type(first.readme) == "string" then string.sub(first.readme, 1, 512 * 1024) else "")
	local shown_pieces = ui.use_state(1)
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
	local function readme(): Node
		local list = ui.computed(function(): { number }
			local out = {}
			for i = 1, math.min(shown_pieces:get(), #pieces) do
				table.insert(out, i)
			end
			return out
		end)
		return ds.Settings.Section {
			title = ui.t("plugins.store.about"),
			ds.Settings.Group {
				ui.column {
					id = "store-readme",
					w_full = true,
					px = "$space.4",
					py = "$space.4",
					gap = "$space.4",
					ui.each(list, function(i: number): Node
						return Markdown {
							-- (Its author's words: links are links, never Zeron's chips.)
							foreign = true,
							text = pieces[i],
							image = picture,
							-- (Under the page's own title: its headings a level
							-- down, none larger than the name; a `# Name` that
							-- opens it, the name again, left out; its pictures
							-- with the page's corner.)
							heading_step = 1,
							drop_title = if i == 1 then same_name else nil,
							picture_radius = "$radius.lg",
						}
					end, {
						key = function(i: number): number
							return i
						end,
					}),
					ui.show(function(): boolean
						return shown_pieces:get() < #pieces
					end, ui.row {
						id = "store-readme-more",
						-- (The page's one way to open more of a text in place,
						-- as a version's notes do.)
						FoldButton {
							label = function(): string
								return ui.t("plugins.store.readme_more"):get()
							end,
							open = function(): boolean
								return false
							end,
							on_click = function()
								shown_pieces:set(shown_pieces:peek() + 1)
							end,
						},
					}),
				},
			},
		}
	end

	-- Its screenshots, each opening the set in the lightbox: 16:10, with
	-- the cards' corner, the strip's first at the cards' edge. Decided by how
	-- many: one fills the column (no taller than a reply's picture,
	-- `zeron.prose.picture.h`, 420); two share it, 12 apart; three or more
	-- show two whole and the third at the column's end, to say there are
	-- more, as wide as two and a bit of the column as it's laid out.
	local function shots(): Node
		local list: { string } = {}
		for _, s in (if type(first.screenshots) == "table" then first.screenshots else {}) :: { any } do
			if type(s) == "string" and package_path(s) and #list < 8 then
				table.insert(list, package_path(s) :: string)
			end
		end
		if #list == 0 then
			return ui.div {}
		end
		local gap = (ui.theme.get("space.3") :: number?) or 12
		local radius = (ui.theme.get("radius.lg") :: number?) or 12
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
					return ui.div { w_full = true, h_full = true, rounded = "$radius.lg", overflow = "hidden", ds.Skeleton { fill = true } }
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
					rounded = "$radius.lg",
					ui.overlay {
						w_full = true,
						h_full = true,
						rounded = "$radius.lg",
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
		if #list == 1 then
			return ui.div {
				id = "store-shots",
				class = "store-shot",
				w_full = true,
				aspect = 1.6,
				max_h = ui.theme.get("zeron.prose.picture.h") :: number?,
				picture_in(list[1]),
			}
		elseif #list == 2 then
			local pair: { Node } = {}
			for _, path in list do
				table.insert(pair, ui.div { class = "store-shot", grow = 1, basis = 0, min_w = 0, aspect = 1.6, picture_in(path) })
			end
			return ui.row { id = "store-shots", w_full = true, gap = "$space.3", table.unpack(pair) }
		end
		-- (The column's width as it's laid out; the measure until then.)
		local column_w = ui.use_state((ui.theme.get("zeron.settings.measure") :: number?) or 640)
		local function w(): number
			return math.floor((column_w:get() - 2 * gap) / 2.4)
		end
		local function h(): number
			return math.floor(w() / 1.6)
		end
		local thumbs: { Node } = {}
		for _, path in list do
			table.insert(thumbs, ui.div { class = "store-shot", w = w, h = h, shrink = 0, picture_in(path) })
		end
		return ui.div {
			w_full = true,
			on_resize = function(size: { w: number, h: number })
				if math.abs(size.w - column_w:peek()) >= 0.5 then
					column_w:set(size.w)
				end
			end,
			ui.scroll_x {
				id = "store-shots",
				w_full = true,
				h = h,
				fade = { 12, 12 },
				fade_overflow = true,
				ui.row { gap = "$space.3", table.unpack(thumbs) },
			},
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
	local versions = ui.computed(function(): { store.Version }
		local out: { store.Version } = {}
		for _, v in (if type(d().versions) == "table" then d().versions else {}) :: { any } do
			if type(v) == "table" and type(v.version) == "string" and (v.status == nil or v.status == "approved" or v.status == "yanked") then
				table.insert(out, v)
			end
		end
		return out
	end)
	local all_versions = ui.use_state(false)
	local listed = ui.computed(function(): { store.Version }
		local all = versions:get()
		if all_versions:get() or #all <= VERSIONS_FIRST then
			return all
		end
		return table.move(all, 1, VERSIONS_FIRST, 1, {})
	end)
	local function version_row(first_v: store.Version, _i: number, now: State<store.Version>): Node
		local latest = (versions:peek()[1] or {}).version == first_v.version
		local function notes(): string
			local n = now:get().notes
			return if type(n) == "string" then string.sub(n, 1, NOTES_MOST) else ""
		end
		return ds.Settings.Row {
			label = function(): string
				return now:get().version
			end,
			badge = ui.show(function(): boolean
				return now:get().yanked == true or now:get().status == "yanked"
			end, ds.Badge { ui.t("plugins.store.status.yanked"), tone = "info", size = "sm" }),
			description = function(): string
				local at = now:get().published_at
				return if type(at) == "number" then locale.date(at / 1000) else ""
			end,
			stacked = true,
			control = ui.show(function(): boolean
				return notes() ~= ""
			end, Notes { text = notes, lines = if latest then NOTES_LINES else 1 }),
		}
	end
	local function history(): Node
		return ds.Settings.Section {
			title = ui.t("plugins.store.versions"),
			ds.Settings.Each(listed, version_row, {
				key = function(v: store.Version): string
					return v.version
				end,
			}),
			ui.show(function(): boolean
				return not all_versions:get() and #versions:get() > VERSIONS_FIRST
			end, ui.row {
				px = ds.Settings.inset(),
				FoldButton {
					label = function(): string
						return ui.t("plugins.store.all_versions", { n = #versions:get() }):get()
					end,
					open = function(): boolean
						return all_versions:get()
					end,
					on_click = function()
						all_versions:set(true)
					end,
					class = "store-all-versions",
				},
			}),
		}
	end

	-- What changes with the update, read before it: the changelog and what
	-- it asks for more or no longer.
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
			body = ui.t("plugins.store.update_body"),
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
					-- A new publisher, by name (asked again after it updates),
					-- and a version still in review (offered only to its author).
					ui.show(function(): boolean
						local it: any = item()
						local u = if it then it.update else nil
						return type(u) == "table" and (u.new_owner == true or u.in_review == true)
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
							return it ~= nil and type(it.update) == "table" and it.update.in_review == true
						end, ui.text {
							ui.t("plugins.store.update_in_review"),
							w_full = true,
							wrap = true,
							text_size = "$font.size.sm",
							line_height = "$font.line.sm",
							color = "$text.secondary",
						}),
					}),
					ui.show(function(): boolean
						return notes() ~= ""
					end, ui.column {
						w_full = true,
						gap = "$space.2",
						ui.text { ui.t("plugins.store.whats_new"), text_size = "$font.size.sm", weight = "$font.weight.medium", color = "$text.secondary" },
						Markdown { text = notes, small = true, foreign = true },
					}),
					ui.presence(function(): string
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
					end),
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
				return ui.t("plugins.store.report.title", { name = d().name or id }):get()
			end,
			body = ui.t("plugins.store.report.body"),
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
			readme(),
			asks(),
			facts(),
			prerelease,
			history(),
			ui.row {
				w_full = true,
				px = ds.Settings.inset(),
				ds.Link {
					ui.t("plugins.store.report.link"),
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
			if details:peek()[id] then
				return
			end
			local m = if type(err) == "table" then err.message else err
			local word = if type(m) == "string" then (string.match(m, "^([%l_]+)") or m) else ""
			phase:set(if word == "not_in_store" then "not_found" elseif word == "offline" or word == "store_unreachable" then "offline" else "failed")
		end)
	end
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
							ds.Skeleton { w = 48, h = 48, rounded = "$radius.md", still = still },
							ui.column { grow = 1, gap = "$space.2", ds.Skeleton { w = 180, h = 20, still = still }, ds.Skeleton { w = 120, still = still }, ds.Skeleton { w = 160, still = still } },
							ds.Skeleton { w = 64, h = 28, rounded = "$radius.lg", still = still },
						},
						ds.Settings.Group {
							ds.SkeletonRow { lead = 0, title = 160, line = 220, still = still },
							ds.Settings.Rule(),
							ds.SkeletonRow { lead = 0, title = 120, line = 260, still = still },
						},
					}
				end,
			}
		elseif kind == "not_found" then
			return said(ds.EmptyState {
				icon = "search",
				title = ui.t("plugins.store.not_found_title"),
				body = ui.t("plugins.store.not_found_body"),
				size = "sm",
				action = {
					label = ui.t("plugins.store.browse_all"),
					on_click = function()
						nav.back()
					end,
				},
			})
		end
		return ds.Callout {
			tone = if kind == "offline" then "neutral" else "danger",
			icon = if kind == "offline" then "wifi-off" else nil,
			title = if kind == "offline" then ui.t("plugins.store.offline_title") else ui.t("plugins.store.failed_title"),
			body = if kind == "offline" then ui.t("plugins.store.offline_body") else ui.t("plugins.store.failed_body"),
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
			label = ui.t("plugins.page.title"),
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
