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
--   Installed             there, and nothing to do
-- A version Zeron took down says why, and can be turned back on (it's
-- yours); one made for a later Zeron says which it needs, and doesn't
-- install. Nothing installs unless the person asks here.
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
local Lightbox = require("@ds/lightbox")
local Links = require("@ds/links")
local Markdown = require("@ds/markdown")
local locale = require("@ds/locale")
local store = require("@app/screens/plugins/store")
local state = require("@app/screens/plugins/state")
local words = require("@app/screens/plugins/words")
local asks_view = require("@app/screens/plugins/asks")
local nav = require("@app/screens/plugins/nav")
local semver = require("@app/screens/plugins/semver")
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

-- This build's Zeron (the engine's, as the About page has it), for a
-- device that doesn't say its own.
local function zeron_of(device: string?): string?
	for _, d in sessions.devices:get() or {} do
		if (device and d.id == device) or (not device and d.here) then
			return d.version
		end
	end
	return nil
end

-- A link the page may open: the web's alone.
local function web(url: any): string?
	if type(url) == "string" and #url <= 2048 and (string.match(url, "^https://[%w%-%.]+") or string.match(url, "^http://[%w%-%.]+")) then
		return url
	end
	return nil
end

-- "github.com/ada/focus", for a link's words.
local function shown_url(url: string): string
	local w = string.gsub(url, "^https?://", "")
	w = string.gsub(w, "/$", "")
	return w
end

-- A picture path of its package, as the store serves them: relative,
-- inside it. Anything else (a URL, an absolute path, a way out) isn't one.
local function package_path(src: string): string?
	if src == "" or #src > 512 or string.find(src, "^%a[%w+.-]*:") or string.sub(src, 1, 1) == "/" or string.find(src, "\\", 1, true) then
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

-- Whether it works with the Zeron of the device shown: true, false, or nil
-- when that can't be told (the device didn't say, or the range can't be
-- read: then the engine decides as it installs).
local function works(range: string?, zeron: string?): boolean?
	if range == nil or zeron == nil then
		return if range == nil then true else nil
	end
	return semver.satisfies(zeron, range)
end

local function said(node: Node): Node
	return ui.column { w_full = true, py = "$space.6", px = "$space.4", node }
end

-- The page itself, its detail read.
local Found = ui.component(function(props: { plugin: string, here: Here }): Node
	local id, here = props.plugin, props.here
	local function d(): any
		return details:get()[id] or {}
	end
	local first: any = details:peek()[id] or {}
	local busy = ui.use_state(false)
	local updating = ui.use_state(false)
	local update_open = ui.use_state(false)
	local report_open = ui.use_state(false)
	local report_reason = ui.use_state("broken")
	local report_text = ui.use_state("")
	local unblocking = ui.use_state(false)

	local function item(): state.Item?
		for _, p in state.shown:get() do
			if p.id == id then
				return p
			end
		end
		return nil
	end
	local function range(): string?
		local e = d().engines
		return if type(e) == "table" and type(e.zeron) == "string" and e.zeron ~= "" then e.zeron else nil
	end
	local function zeron(): string?
		return zeron_of(here.device())
	end

	-- What the one button does now.
	local function standing(): string
		local it: any = item()
		if it then
			if type(it.blocked) == "table" then
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
		elseif works(range(), zeron()) == false then
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
	local function unblock()
		unblocking:set(true)
		store.unblock(id, here.device()):after(function()
			unblocking:set(false)
		end, function(err: any)
			unblocking:set(false)
			here.failed(ui.t("plugins.store.couldnt_unblock", { name = d().name or id }))(err)
		end)
	end

	-- The one button, by what it does now.
	local function action(kind: string): Node
		if kind == "install" or kind == "incompatible" or kind == "away" then
			return ds.Button {
				id = "store-install",
				tone = "primary",
				size = "md",
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
			return ds.Button {
				id = "store-update",
				tone = "primary",
				size = "md",
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
			return ds.Button {
				id = "store-review",
				tone = "accent",
				size = "md",
				on_click = function()
					state.review(id, here.device(), if here.device() then here.device_name() else nil)
				end,
				ui.t("plugins.page.review"),
			}
		elseif kind == "off" then
			return ds.Button { id = "store-turn-on", tone = "primary", size = "md", on_click = turn_on, ui.t("plugins.store.turn_on") }
		elseif kind == "settings" then
			return ds.Button {
				id = "store-settings",
				tone = "secondary",
				size = "md",
				on_click = function()
					plugin_settings.open(id)
				end,
				ui.t("plugins.store.open_settings"),
			}
		elseif kind == "blocked" then
			return ui.div {}
		end
		return ui.row {
			id = "store-installed",
			gap = "$space.1",
			items = "center",
			color = "$zeron.tone.success.ink",
			ds.Icon { name = "check-circle", size = 16 },
			ui.text {
				function(): string
					return if here.device()
						then ui.t("plugins.store.installed_on", { device = here.device_name() }):get()
						else ui.t("plugins.store.installed"):get()
				end,
				text_size = "$font.size.md",
				line_height = "$font.line.md",
				weight = "$font.weight.medium",
			},
		}
	end

	-- What's said beside the button: what installing does, why it can't,
	-- or nothing.
	local function note(): string
		local k = standing()
		if k == "install" then
			return ui.t("plugins.store.install_note"):get()
		elseif k == "incompatible" then
			local least = semver.least(range())
			local has = zeron()
			if least and has then
				return ui.t("plugins.store.needs_zeron_has", { version = least, device = here.device_name(), has = has }):get()
			elseif least then
				return ui.t("plugins.store.needs_zeron", { version = least }):get()
			end
			return ui.t("plugins.store.needs_other_zeron"):get()
		elseif k == "away" then
			return ui.t("plugins.store.device_away", { device = here.device_name() }):get()
		elseif k == "update" then
			return ui.t("plugins.store.update_note"):get()
		elseif k == "review" then
			return ui.t("plugins.store.review_note"):get()
		end
		return ""
	end

	local offer = ui.column {
		id = "store-offer",
		w_full = true,
		gap = "$space.3",
		-- A version Zeron took down: why, and the way back.
		ui.show(function(): boolean
			return standing() == "blocked"
		end, ds.Callout {
			tone = "warning",
			title = function(): string
				local it: any = item()
				local b = if it then it.blocked else nil
				local v = if type(b) == "table" and type(b.version) == "string" then b.version else nil
				return if v
					then ui.t("plugins.store.blocked_title_version", { name = d().name or id, version = v }):get()
					else ui.t("plugins.store.blocked_title", { name = d().name or id }):get()
			end,
			body = function(): string
				local it: any = item()
				local b = if it then it.blocked else nil
				local why = if type(b) == "table" and type(b.reason) == "string" and b.reason ~= "" then b.reason else nil
				return if why
					then ui.t("plugins.store.blocked_body_reason", { reason = why }):get()
					else ui.t("plugins.store.blocked_body"):get()
			end,
			action = {
				label = ui.t("plugins.store.turn_back_on"),
				on_click = function()
					if not unblocking:peek() then
						unblock()
					end
				end,
			},
		}),
		ui.row {
			w_full = true,
			gap = "$space.3",
			items = "center",
			-- (Its words on the page's text edge.)
			pl = ds.Settings.inset(),
			ui.show(function(): boolean
				return note() ~= ""
			end, ui.text {
				id = "store-note",
				note,
				grow = 1,
				min_w = 0,
				wrap = true,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = function(): string
					return if standing() == "incompatible" then "$zeron.tone.warning.ink" else "$text.tertiary"
				end,
			}),
			ui.div { grow = 1, min_w = 0 },
			ui.switch(standing, {
				install = action("install"),
				incompatible = action("incompatible"),
				away = action("away"),
				update = action("update"),
				review = action("review"),
				off = action("off"),
				settings = action("settings"),
				installed = action("installed"),
				blocked = action("blocked"),
			}),
		},
	}

	-- The facts: who, which version, how many, where from, what it needs.
	local function facts(): Node
		local items: { any } = {
			{
				key = ui.t("plugins.store.fact.by"),
				value = ui.row {
					gap = "$space.2",
					items = "center",
					ui.text {
						function(): string
							return browse.author(d())
						end,
						text_size = "$font.size.sm",
						color = "$text.primary",
					},
					browse.verified(d),
				},
			},
			{
				key = ui.t("plugins.store.fact.version"),
				value = ui.text {
					function(): string
						local v = d().version or ""
						local at = d().updated_at
						return if type(at) == "number"
							then ui.t("plugins.store.version_on", { version = v, date = locale.date(at / 1000) }):get()
							else v
					end,
					tabular = true,
					text_size = "$font.size.sm",
					color = "$text.primary",
				},
			},
			{
				key = ui.t("plugins.store.fact.installs"),
				value = ui.text {
					function(): string
						local n = d().installs
						return ui.t("plugins.store.installs", { n = if type(n) == "number" then math.max(0, math.floor(n)) else 0 }):get()
					end,
					tabular = true,
					text_size = "$font.size.sm",
					color = "$text.primary",
				},
			},
			{
				key = ui.t("plugins.store.fact.works_with"),
				value = ui.text {
					function(): string
						local r = range()
						if not r then
							return ui.t("plugins.store.works_any"):get()
						end
						local least = semver.least(r)
						return if least then ui.t("plugins.store.works_from", { version = least }):get() else r
					end,
					text_size = "$font.size.sm",
					color = "$text.primary",
				},
			},
		}
		if type(first.categories) == "table" and #first.categories > 0 then
			local names: { string } = {}
			for _, c in first.categories do
				if type(c) == "string" then
					table.insert(names, browse.category_name(c))
				end
			end
			table.insert(items, { key = ui.t("plugins.store.fact.category"), value = table.concat(names, ui.t("plugins.words.list_sep"):get()) })
		end
		if type(first.license) == "string" and first.license ~= "" then
			table.insert(items, { key = ui.t("plugins.store.fact.license"), value = first.license })
		end
		for _, link in { { "homepage", "plugins.store.fact.homepage" }, { "repository", "plugins.store.fact.repository" } } do
			local url = web(first[link[1]])
			if url then
				local u = url :: string
				table.insert(items, {
					key = ui.t(link[2]),
					value = ds.Link {
						shown_url(u),
						external = true,
						href = u,
						size = "sm",
						tone = "quiet",
						on_click = function()
							Links.open(u)
						end,
					},
				})
			end
		end
		return ds.Settings.Group {
			ui.column { w_full = true, px = "$space.4", py = "$space.3", ds.KeyValue { items = items, key_width = 96 } },
		}
	end

	-- The README: the app's Markdown, its pictures the package's own.
	local function readme(): Node
		return ds.Settings.Section {
			title = ui.t("plugins.store.about"),
			ds.Settings.Group {
				ui.column {
					id = "store-readme",
					w_full = true,
					px = "$space.4",
					py = "$space.4",
					Markdown {
						text = function(): string
							local r = d().readme
							return if type(r) == "string" then string.sub(r, 1, 200000) else ""
						end,
						-- A picture of its package, through the store; anything else
						-- is its link (`false`), never fetched from where it points.
						image = function(src: string): any
							local path = package_path(src)
							if not path then
								return false
							end
							return store.picture_src(id, d().version or "", path :: string)
						end,
					},
				},
			},
		}
	end

	-- Its screenshots, side by side, each opening the set in the lightbox.
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
		local function items(): { any }
			local out = {}
			for _, path in list do
				local src = store.picture_src(id, d().version or "", path)
				if src then
					table.insert(out, { src = src, name = string.match(path, "([^/]+)$") or path, key = path })
				end
			end
			return out
		end
		local thumbs: { Node } = {}
		for _, path in list do
			table.insert(thumbs, ui.presence(function(): string
				return store.picture_src(id, d().version or "", path) or ""
			end, function(src: string): Node
				if src == "" then
					return ds.Skeleton { w = 200, h = 125, rounded = "$radius.lg" }
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
					radius = 10,
					w = 200,
					h = 125,
					ui.image { src = src, w_full = true, h_full = true, fit = "cover", aria_hidden = true },
				}
			end))
		end
		return ds.Settings.Section {
			title = ui.t("plugins.store.pictures"),
			ui.scroll {
				id = "store-shots",
				w_full = true,
				direction = "row",
				fade = { 12, 12 },
				fade_overflow = true,
				ui.row { gap = "$space.3", px = ds.Settings.inset(1), table.unpack(thumbs) },
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
			local it = d()
			local lines = words.lines(it, it.permissions or {})
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

	-- Its versions, newest first: each one's date and what changed.
	local versions = ui.computed(function(): { store.Version }
		local out: { store.Version } = {}
		for _, v in (if type(d().versions) == "table" then d().versions else {}) :: { any } do
			if type(v) == "table" and type(v.version) == "string" and (v.status == nil or v.status == "approved" or v.status == "yanked") then
				table.insert(out, v)
			end
		end
		return out
	end)
	local function version_row(first_v: store.Version, _i: number, now: State<store.Version>): Node
		return ds.Settings.Row {
			label = first_v.version,
			badge = ui.show(function(): boolean
				return now:get().yanked == true or now:get().status == "yanked"
			end, ds.Badge { ui.t("plugins.store.yanked"), tone = "neutral", size = "sm" }),
			description = function(): string
				local at = now:get().published_at
				return if type(at) == "number" then locale.date(at / 1000) else ""
			end,
			stacked = true,
			control = ui.show(function(): boolean
				local n = now:get().notes
				return type(n) == "string" and n ~= ""
			end, ui.column {
				w_full = true,
				Markdown {
					text = function(): string
						return string.sub(now:get().notes or "", 1, 20000)
					end,
					small = true,
				},
			}),
		}
	end
	local function history(): Node
		return ds.Settings.Section {
			title = ui.t("plugins.store.versions"),
			ds.Settings.Each(versions, version_row, {
				key = function(v: store.Version): string
					return v.version
				end,
			}),
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
				k,
				size = "sm",
				on_click = function()
					browse.show_keyword(k)
					nav.back()
				end,
			})
		end
		return ui.row { id = "store-keywords", gap = "$space.2", flex_wrap = true, px = ds.Settings.inset(), table.unpack(chips) }
	end

	-- What changes with the update, read before it: the changelog and what
	-- it asks for more or no longer.
	local function update_dialog(): Node
		local function notes(): string
			local it: any = item()
			local u = if it then it.update else nil
			if type(u) == "table" and type(u.notes) == "string" and u.notes ~= "" then
				return string.sub(u.notes, 1, 20000)
			end
			-- (Else the store's, for each version since the one installed.)
			local out: { string } = {}
			local have = if it then it.version else nil
			for _, v in versions:get() do
				if have and (semver.compare(v.version, have) or 0) > 0 and type(v.notes) == "string" and v.notes ~= "" then
					table.insert(out, `**{v.version}**\n\n{v.notes}`)
				end
			end
			return table.concat(out, "\n\n")
		end
		local function changed(which: string): { words.Line }
			local it: any = item()
			local u = if it then it.update else nil
			local list = if type(u) == "table" and type(u[which]) == "table" then u[which] else {}
			return words.lines(d(), list)
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
					ui.show(function(): boolean
						return notes() ~= ""
					end, ui.column {
						w_full = true,
						gap = "$space.2",
						ui.text { ui.t("plugins.store.whats_new"), text_size = "$font.size.sm", weight = "$font.weight.medium", color = "$text.secondary" },
						Markdown { text = notes, small = true },
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

	local REASONS = { "broken", "malware", "impersonation", "spam", "other" }
	local function report_dialog(): Node
		local options = {}
		for _, r in REASONS do
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
		ds.Settings.Page {
			title = function(): string
				return d().name or id
			end,
			description = function(): string
				return d().summary or ""
			end,
			trail = browse.Tile {
				card = function(): store.Card
					return d()
				end,
				size = 48,
			},
			keywords(),
			facts(),
			offer,
			readme(),
			shots(),
			asks(),
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
	}
end)

-- A store plugin's page: read as it opens (at once from the last read),
-- or what stands in its place while it can't be.
M.Page = ui.component(function(props: { plugin: string, here: Here }): Node
	local id = props.plugin
	local phase = ui.use_state(if details:peek()[id] then "found" else "loading")
	local function load()
		store.detail(id):after(function(found: any)
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
			phase:set(if m == "not_found" then "not_found" elseif m == "offline" then "offline" else "failed")
		end)
	end
	-- (As it shows, never as it builds.)
	ui.on_mount(function()
		load()
	end)
	local function body(kind: string): Node
		if kind == "found" then
			return Found { plugin = id, here = props.here }
		elseif kind == "loading" then
			return ui.column {
				id = "store-loading",
				w_full = true,
				gap = "$space.6",
				ui.row {
					w_full = true,
					px = ds.Settings.inset(),
					gap = "$space.3",
					items = "center",
					ui.column { grow = 1, gap = "$space.2", ds.Skeleton { w = 180, h = 20 }, ds.Skeleton { w = 300 } },
					ds.Skeleton { w = 48, h = 48, rounded = "$radius.lg" },
				},
				ds.Settings.Group {
					ds.SkeletonRow { lead = 0, title = 160, line = 220 },
					ds.Settings.Rule(),
					ds.SkeletonRow { lead = 0, title = 120, line = 260 },
				},
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
		ui.presence(function(): string
			return phase:get()
		end, body, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
	}
end)

return M
