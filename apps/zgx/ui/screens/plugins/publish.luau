--!strict
-- Publishing one of yours to the plugin store, a page under Settings →
-- Plugins (from its row in Yours): what the store needs, checked on this
-- device (the engine's `PreparePluginPublish`), each thing missing said
-- with what to do about it; then Submit, which sends exactly what was
-- checked for review (`PublishPlugin` with its one-time ticket: a folder
-- changed since asks to check again). Under it, every version you've sent
-- and where its review stands, the reviewer's words when it wasn't
-- approved, and what you can still do with each (withdraw one in review,
-- stop new installs of one approved, allow them again). Then its deploy
-- keys: what a CI job publishes with instead of your session, made after a
-- fresh sign-in, its secret shown once.
--
-- Yours' rows say where each of yours stands in the store (`summary`),
-- read once a visit (`load`).
--
--   publish.Page { plugin, here }
--   publish.load()          your plugins in the store, read again
--   publish.summary(id)     "1.2.0 in the plugin store · 1.3.0 in review", or
--                           "Not in the plugin store" (nil: not known yet)

local ds = require("@ds")
local locale = require("@ds/locale")
local store = require("@app/screens/plugins/store")
local state = require("@app/screens/plugins/state")
local words = require("@app/screens/plugins/words")
local nav = require("@app/screens/plugins/nav")

local M = {}

-- Your plugins in the store, as last read: "loading", "ready", or why not
-- ("offline", "signed_out", the store's words), and the plugins by id.
local mine = ui.state("store_mine", { state = "idle", plugins = {} } :: { state: string, plugins: { [string]: any } })
local loading = false

function M.load()
	if loading then
		return
	end
	loading = true
	if mine:peek().state == "idle" then
		mine:set({ state = "loading", plugins = {} })
	end
	store.mine():after(function(r: any)
		loading = false
		local by_id: { [string]: any } = {}
		for _, p in (if type(r) == "table" and type(r.plugins) == "table" then r.plugins else {}) :: { any } do
			if type(p) == "table" and store.valid_id(p.id) then
				by_id[p.id] = p
			end
		end
		mine:set({ state = "ready", plugins = by_id })
	end, function(err: any)
		loading = false
		local m = if type(err) == "table" then err.message else err
		-- (What was read stays shown; the next visit asks again.)
		if mine:peek().state ~= "ready" then
			mine:set({ state = if type(m) == "string" and m ~= "" then m else "failed", plugins = {} })
		end
	end)
end

-- A version's state as the store keeps it: its review's, else its own.
local function status_of(v: any): string
	local s = v.status
	if type(v.review) == "table" and type(v.review.state) == "string" and (s == nil or s == "pending") then
		s = v.review.state
	end
	return if type(s) == "string" then s else "pending"
end

local STATUS_TONE: { [string]: string } = {
	pending = "accent",
	approved = "success",
	rejected = "danger",
	withdrawn = "neutral",
	-- (Approved, no longer offered: not the same as pulled before review.)
	yanked = "info",
	removed = "warning",
}

function M.summary(id: string): string?
	local now = mine:get()
	if now.state ~= "ready" then
		return nil
	end
	local p = now.plugins[id]
	if not p then
		return ui.t("plugins.store.summary.none"):get()
	end
	-- Newest first: the version people install (or, stopped, the one
	-- that was), and the newest past it that's still yours to hear about:
	-- in review, not approved, taken down. (One you withdrew isn't news.)
	local listed, stopped, news = nil :: string?, nil :: string?, nil :: { version: string, state: string }?
	for _, v in (if type(p.versions) == "table" then p.versions else {}) :: { any } do
		local st = status_of(v)
		if st == "approved" then
			listed = v.version
			break
		elseif st == "yanked" and stopped == nil then
			stopped = v.version
		elseif news == nil and stopped == nil and (st == "pending" or st == "rejected" or st == "removed") then
			news = { version = v.version, state = st }
		end
	end
	local parts: { string } = {}
	if listed then
		table.insert(parts, ui.t("plugins.store.summary.listed", { version = listed }):get())
	elseif stopped then
		table.insert(parts, ui.t("plugins.store.summary.no_installs", { version = stopped }):get())
	end
	if news then
		local key = if news.state == "pending" then "in_review" elseif news.state == "rejected" then "not_approved" else "taken_down"
		table.insert(parts, ui.t(`plugins.store.summary.{key}`, { version = news.version }):get())
	end
	if #parts == 0 then
		return ui.t("plugins.store.summary.none"):get()
	end
	return table.concat(parts, " · ")
end

-- What a check that didn't pass asks of you, in Zeron's words where it
-- knows the rule (and the field), else the engine's.
local FIELDS: { [string]: boolean } = { icon = true, categories = true, description = true, name = true, keywords = true, screenshots = true, homepage = true, repository = true, license = true }
local function fix_of(p: any): string
	local rule, field = p.rule, p.field
	if (rule == "missing_listing" or rule == "bad_listing") and type(field) == "string" and FIELDS[field] then
		return ui.t(`plugins.store.fix.{rule}.{field}`):get()
	elseif rule == "missing_readme" or rule == "not_semver" or rule == "id_reserved" or rule == "too_large" or rule == "binary_file" or rule == "no_notes" or rule == "raw_html" or rule == "footnote" or rule == "math" or rule == "not_newer" or rule == "version_exists" then
		return ui.t(`plugins.store.fix.{rule}`):get()
	end
	return if type(p.words) == "string" and p.words ~= "" then string.sub(p.words, 1, 400) else ui.t("plugins.store.fix.other"):get()
end

-- Where a problem or a warning is: its file, and its line when it's about
-- one ("README.md, line 3"); nil when it's about neither.
local function where_of(p: any): string?
	if type(p.path) ~= "string" or p.path == "" then
		return nil
	end
	if type(p.line) == "number" and p.line >= 1 and p.line % 1 == 0 then
		return ui.t("plugins.store.at_line", { path = p.path, line = p.line }):get()
	end
	return p.path
end

-- What went wrong, in Zeron's words (screens/plugins/words `failure`: the
-- engine's reason words); the engine out of reach said as the store's is.
function M.failure(message: any): string
	local m = if type(message) == "string" then message else ""
	if m == "offline" then
		return ui.t("plugins.store.offline_title"):get()
	end
	-- (However the store said it: you, signed out.)
	if store.signed_out(m) then
		return words.failure("signed_out")
	end
	return words.failure(m)
end

-- What your versions and a plugin's keys show for you signed out (no
-- session, or one that ended while the page was open), in place of what
-- couldn't be read: what to sign in for, why, and a way to look again.
-- (Publish's checks never fail on the store: the engine checks without it
-- and says so; Submit says to sign in, in its own line.)
function M.sign_in_callout(title: Message, again: () -> ()): Node
	return ds.Callout {
		tone = "neutral",
		title = title,
		body = words.failure("signed_out"),
		action = { label = ui.t("plugins.store.try_again"), on_click = again },
	}
end

-- One line of the checks, a Settings row (its own heights: 52, or 60 with
-- what to do under it): a mark, what it is, and what to do. `ok`: passed,
-- not passed, "later" (the store says as it's sent), or "note" (worth
-- knowing, nothing in the way: advice under the checks, in the secondary
-- ink).
local function check_row(ok: boolean | "later" | "note", title: string | Message, detail: (string | Message | () -> string)?): Node
	local icon = if ok == true then "check-circle" elseif ok == "later" then "clock-circle" elseif ok == "note" then "info-circle" else "danger-triangle"
	local tone = if ok == true then "$zeron.tone.success.ink" elseif ok == "later" or ok == "note" then "$text.tertiary" else "$zeron.tone.warning.ink"
	return ds.Settings.Row {
		label = title :: any,
		description = detail :: any,
		quiet = ok == "note",
		lead = ui.div { shrink = 0, color = tone, ds.Icon { name = icon :: any, size = 16 } },
	}
end

-- What the store needs, in the order a person fixes them; each passes
-- unless one of the engine's problems names it.
local CHECKS: { { id: string, rule: string, field: string? } } = {
	{ id = "readme", rule = "missing_readme" },
	{ id = "description", rule = "listing", field = "description" },
	{ id = "icon", rule = "listing", field = "icon" },
	{ id = "categories", rule = "listing", field = "categories" },
	{ id = "version", rule = "not_semver" },
}

local function matches(check: { id: string, rule: string, field: string? }, p: any): boolean
	if check.rule == "listing" then
		return (p.rule == "missing_listing" or p.rule == "bad_listing") and p.field == check.field
	end
	return p.rule == check.rule
end

M.Page = ui.component(function(props: { plugin: string, here: any }): Node
	local id = props.plugin
	-- The checks: "checking", "ready" (with `prepared`), or why not.
	local phase = ui.use_state("checking")
	local prepared = ui.use_state(nil :: any)
	local sending = ui.use_state(false)
	local sent_error = ui.use_state("")
	local acting = ui.use_state(nil :: string?)
	local confirming = ui.use_state(nil :: any)
	local confirm_open = ui.use_state(false)
	local function plugin(): state.Item?
		for _, p in state.list:get() do
			if p.id == id then
				return p
			end
		end
		return nil
	end
	local function name(): string
		local p = plugin()
		return if p then p.name else id
	end

	-- (Each check numbered: only the last one's answer, and its ticket, count.)
	local checks_asked = 0
	local function check()
		checks_asked += 1
		local mine_ = checks_asked
		phase:set("checking")
		sent_error:set("")
		store.prepare(id):after(function(r: any)
			if mine_ ~= checks_asked then
				return
			end
			prepared:set(r)
			phase:set("ready")
		end, function(err: any)
			if mine_ ~= checks_asked then
				return
			end
			local m = if type(err) == "table" then err.message else err
			phase:set(if type(m) == "string" and m ~= "" then m else "failed")
		end)
	end
	ui.on_mount(function()
		check()
		M.load()
	end)

	local function submit()
		local r = prepared:peek()
		if sending:peek() or type(r) ~= "table" or r.ok ~= true or type(r.ticket) ~= "string" then
			return
		end
		sending:set(true)
		sent_error:set("")
		store.publish(id, r.hash, r.ticket):after(function(done: any)
			sending:set(false)
			local v = if type(done) == "table" and type(done.version) == "string" then done.version else r.version
			ds.toast.show { title = ui.t("plugins.store.sent_toast", { name = name(), version = v or "" }), tone = "success" }
			-- (Its ticket is spent: checked again for the next one.)
			check()
			M.load()
		end, function(err: any)
			sending:set(false)
			local m = if type(err) == "table" then err.message else err
			sent_error:set(M.failure(m))
			if type(m) == "string" and string.match(m, "^changed") then
				check()
			end
		end)
	end

	-- What the checks found.
	local function checks(): Node
		local r = prepared:peek() or {}
		local problems: { any } = if type(r.problems) == "table" then r.problems else {}
		local rows: { Node } = {}
		local used: { [any]: boolean } = {}
		local function add(node: Node)
			if #rows > 0 then
				table.insert(rows, ds.Settings.Rule())
			end
			table.insert(rows, node)
		end
		for _, c in CHECKS do
			local found: any = nil
			for _, p in problems do
				if type(p) == "table" and matches(c, p) then
					found = p
					used[p] = true
				end
			end
			add(check_row(found == nil, ui.t(`plugins.store.check.{c.id}`), if found then fix_of(found) else nil))
		end
		-- Newer than every version sent: a check of its own when the engine
		-- could ask the store which it has (`store_checked`); else what the
		-- engine says of it, the store having the last word as it's sent.
		if r.store_checked == true then
			local found: any = nil
			for _, p in problems do
				if type(p) == "table" and (p.rule == "not_newer" or p.rule == "version_exists") then
					found = p
					used[p] = true
				end
			end
			add(check_row(found == nil, ui.t("plugins.store.check.newer"), if found then fix_of(found) else nil))
		else
			-- (Not asked, so nothing to say of it but the note: a newer-than
			-- answer can't come with it, and none is shown if one does.)
			for _, p in problems do
				if type(p) == "table" and (p.rule == "not_newer" or p.rule == "version_exists") then
					used[p] = true
				end
			end
			local note = if type(r.note) == "string" and r.note ~= "" then string.sub(r.note, 1, 300) else nil
			add(check_row("later", ui.t("plugins.store.check.newer"), function(): string
				return note or ui.t("plugins.store.check.newer_unchecked"):get()
			end))
		end
		-- Anything else in the way, in the engine's words.
		for _, pr in problems do
			if type(pr) == "table" and not used[pr] then
				add(check_row(false, fix_of(pr), where_of(pr)))
			end
		end
		-- What's worth knowing and in nobody's way, under them.
		for _, w in (if type(r.warnings) == "table" then r.warnings else {}) :: { any } do
			if type(w) == "table" then
				add(check_row("note", fix_of(w), where_of(w)))
			end
		end
		-- Its last row: what Submit sends (or what's in the way), and Submit,
		-- ending on the content edge as every row's control does.
		add(ui.row {
			id = "publish-foot",
			w_full = true,
			min_h = ui.theme.get("zeron.settings.row") :: number?,
			px = "$space.4",
			py = "$space.3",
			gap = "$space.3",
			items = "center",
			ui.text {
				function(): string
					if sent_error:get() ~= "" then
						return sent_error:get()
					end
					local now = prepared:get() or {}
					if now.ok == true then
						local pkg = now.package
						local files = if type(pkg) == "table" and type(pkg.files) == "number" then pkg.files else 0
						return ui.t("plugins.store.submit_note", { n = files }):get()
					end
					return ui.t("plugins.store.fix_first"):get()
				end,
				id = "publish-note",
				grow = 1,
				min_w = 0,
				wrap = true,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = function(): string
					return if sent_error:get() ~= "" then "$zeron.tone.danger.ink" else "$text.secondary"
				end,
			},
			ds.Button {
				id = "publish-submit",
				tone = "primary",
				size = "sm",
				disabled = function(): boolean
					local now = prepared:get() or {}
					return now.ok ~= true or type(now.ticket) ~= "string"
				end,
				busy = function(): boolean
					return sending:get()
				end,
				on_click = submit,
				function(): string
					local v = (prepared:get() or {}).version
					return if type(v) == "string" and v ~= ""
						then ui.t("plugins.store.submit_version", { version = v }):get()
						else ui.t("plugins.store.submit"):get()
				end,
			},
		})
		return ds.Settings.Group { ui.column { id = "publish-checks", w_full = true, table.unpack(rows) } }
	end

	local function checks_body(key: string): Node
		if key == "ready" then
			return checks()
		elseif key == "checking" then
			-- (The checks are known before they run: a row for each, one
			-- line high as a check that passes is, then the foot row; so the
			-- card opens at the height it has when everything passes, and
			-- only a problem's fix line moves what's under it.)
			return ds.SkeletonWaiting {
				build = function(still: boolean): Node
					local rows: { Node } = {}
					for i = 1, #CHECKS + 1 do
						table.insert(rows, ds.SkeletonRow { lead = 16, title = if i % 2 == 0 then 110 else 150, line = 0, still = still })
					end
					table.insert(rows, ui.row {
						w_full = true,
						min_h = "$zeron.settings.row",
						px = "$space.4",
						py = "$space.3",
						gap = "$space.3",
						items = "center",
						ui.div { grow = 1, ds.Skeleton { w = 240, h = 8, still = still } },
						ds.Skeleton { w = 160, h = 28, rounded = "$radius.md", still = still },
					})
					-- (The card draws the hairlines between its rows.)
					return ds.Settings.Group(rows :: any)
				end,
			}
		end
		return ds.Callout {
			tone = "danger",
			title = ui.t("plugins.store.check_failed"),
			body = M.failure(key),
			action = { label = ui.t("plugins.store.try_again"), on_click = check },
		}
	end

	-- Its versions in the store, newest first.
	local versions = ui.computed(function(): { any }
		local p = mine:get().plugins[id]
		local out = {}
		for _, v in (if p and type(p.versions) == "table" then p.versions else {}) :: { any } do
			if type(v) == "table" and type(v.version) == "string" then
				table.insert(out, v)
			end
		end
		return out
	end)
	local function act(v: any, action: string)
		acting:set(`{v.version}:{action}`)
		store.version_action(id, v.version, action):after(function()
			acting:set(nil)
			M.load()
		end, function(err: any)
			acting:set(nil)
			local m = if type(err) == "table" then err.message else err
			ds.toast.show { title = ui.t("plugins.store.action_failed"), body = M.failure(m), tone = "warning" }
		end)
	end
	local function version_row(first: any, _i: number, now: State<any>): Node
		local function action_of(): string?
			local s = status_of(now:get())
			return if s == "pending" then "withdraw" elseif s == "approved" then "yank" elseif s == "yanked" then "unyank" else nil
		end
		return ds.Settings.Row {
			label = function(): string
				return now:get().version
			end,
			-- (Each state its own badge: its tone is its state's.)
			badge = ui.switch(function(): string
				local s = status_of(now:get())
				return if STATUS_TONE[s] then s else "pending"
			end, (function(): { [string]: Node }
				local out: { [string]: Node } = {}
				for s, tone in STATUS_TONE do
					out[s] = ds.Badge { ui.t(`plugins.store.status.{s}`), tone = tone :: any, size = "sm" }
				end
				return out
			end)()),
			description = function(): string
				local v = now:get()
				local at = v.published_at
				local when = if type(at) == "number" then locale.date(at / 1000) else ""
				-- The reviewer's words, when it wasn't approved or was taken down.
				local review = v.review
				local why = if type(review) == "table" then (review.message or review.reason) else nil
				if type(why) == "string" and why ~= "" and (status_of(v) == "rejected" or status_of(v) == "removed") then
					return ui.t("plugins.store.reviewer_said", { date = when, words = string.sub(why, 1, 600) }):get()
				end
				return when
			end,
			control = ui.switch(function(): string
				return action_of() or "none"
			end, {
				none = ui.div {},
				withdraw = ds.Button {
					tone = "secondary",
					size = "sm",
					busy = function(): boolean
						return acting:get() == `{first.version}:withdraw`
					end,
					on_click = function()
						confirming:set({ version = now:peek(), action = "withdraw" })
						confirm_open:set(true)
					end,
					ui.t("plugins.store.action.withdraw"),
				},
				yank = ds.Button {
					tone = "secondary",
					size = "sm",
					busy = function(): boolean
						return acting:get() == `{first.version}:yank`
					end,
					on_click = function()
						confirming:set({ version = now:peek(), action = "yank" })
						confirm_open:set(true)
					end,
					ui.t("plugins.store.action.yank"),
				},
				unyank = ds.Button {
					tone = "secondary",
					size = "sm",
					busy = function(): boolean
						return acting:get() == `{first.version}:unyank`
					end,
					on_click = function()
						act(now:peek(), "unyank")
					end,
					ui.t("plugins.store.action.unyank"),
				},
			}),
		}
	end
	local function history(): Node
		return ds.Settings.Section {
			title = ui.t("plugins.store.your_versions"),
			ui.presence(function(): string
				local s = mine:get().state
				if s == "ready" then
					return if #versions:get() > 0 then "list" else "none"
				end
				return if store.signed_out(s) then "signed_out" else s
			end, function(key: string): Node
				if key == "list" then
					return ds.Settings.Each(versions, version_row, {
						key = function(v: any): string
							return v.version
						end,
					})
				elseif key == "none" then
					-- (None yet: one quiet line in the card, as its keys' none.)
					return ds.Settings.Group {
						ui.text {
							id = "versions-none",
							ui.t("plugins.store.no_versions"),
							w_full = true,
							px = "$space.4",
							py = "$space.3",
							wrap = true,
							text_size = "$font.size.sm",
							line_height = "$font.line.sm",
							color = "$text.secondary",
						},
					}
				elseif key == "loading" or key == "idle" then
					return ds.SkeletonWaiting {
						build = function(still: boolean): Node
							return ds.Settings.Group { ds.SkeletonRow { lead = 0, title = 60, line = 140, trail = 70, still = still } }
						end,
					}
				end
				if key == "signed_out" then
					return M.sign_in_callout(ui.t("plugins.store.sign_in_to_see"), M.load)
				end
				return ds.Callout {
					tone = "neutral",
					icon = if key == "offline" then "wifi-off" else nil,
					title = ui.t("plugins.store.versions_failed"),
					body = M.failure(key),
					action = { label = ui.t("plugins.store.try_again"), on_click = M.load },
				}
			end, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
		}
	end

	local keys_view = require("@app/screens/plugins/keys")

	return ui.column {
		w_full = true,
		ds.Settings.SubPage {
			where = ds.Settings.Back {
				label = ui.t("plugins.page.title"),
				on_click = function()
					nav.back()
				end,
			},
			ds.Settings.Page {
				title = function(): string
					return ui.t("plugins.store.publish_title", { name = name() }):get()
				end,
				description = ui.t("plugins.store.publish_body"),
				-- What the store needs: its title there from the first frame,
				-- its card the checks' (a skeleton while they run), Check
				-- again the section's own action at its title's end.
				ds.Settings.Section {
					title = ui.t("plugins.store.checks_title"),
					-- (The app's section action: a ghost, small, its words on
					-- the content edge past its own padding, as Settings'.)
					trail = ui.div {
						mr = ui.raw(-((ui.theme.get("zeron.space.snug") :: number?) or 10), "the ghost's own padding"),
						ds.Button {
							id = "publish-check-again",
							tone = "ghost",
							size = "sm",
							disabled = function(): boolean
								return phase:get() == "checking"
							end,
							on_click = check,
							ui.t("plugins.store.check_again"),
						},
					},
					ui.presence(function(): string
						local p = phase:get()
						return if p == "checking" or p == "ready" then p else `failed:{p}`
					end, function(key: string): Node
						return checks_body(if string.match(key, "^failed:") then string.sub(key, 8) else key)
					end, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
				},
				history(),
				keys_view.Section {
					plugin = id,
					here = props.here,
					published = function(): boolean
						return #versions:get() > 0
					end,
				},
			},
		},
		ds.Dialog {
			open = confirm_open,
			title = function(): string
				local c = confirming:get()
				if not c then
					return ""
				end
				return ui.t(`plugins.store.confirm.{c.action}.title`, { name = name(), version = c.version.version }):get()
			end,
			body = function(): string
				local c = confirming:get()
				return if c then ui.t(`plugins.store.confirm.{c.action}.body`):get() else ""
			end,
			confirm = {
				label = function(): string
					local c = confirming:get()
					return if c then ui.t(`plugins.store.action.{c.action}`):get() else ""
				end,
				on_click = function()
					local c = confirming:peek()
					confirming:set(nil)
					if c then
						act(c.version, c.action)
					end
				end,
			},
		},
	}
end)

return M
