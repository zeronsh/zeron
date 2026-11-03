--!strict
-- What a plugin asks for, in plain words: each permission's line on the
-- consent sheet and the Plugins page, and where in Zeron a selector or a
-- screen is. One place, so the sheet and the page always say the same thing.
--
--   words.permission(item, "ui.change")  { title, detail?, risky?, icon }
--   words.author(item)                    "by you" / "by @ada" / "Unknown author"
--   words.place(".session-row")          "a sidebar row"

local M = {}

-- (Every word here is read in the locale as it's asked for: a Line's
-- `title` and `detail` are read through their messages each time, and
-- every function gives a string now, so where it's drawn in a function it
-- follows the locale.)
-- `literal`: what it names exactly as its manifest writes it (selectors,
-- modules, programs), shown under the words so a plain-words place can't
-- hide what it reaches. `items`: named things with words of their own (an
-- agent tool and its description, as agents will read it).
export type Item = { name: string, description: string? }
export type Line = { title: string, detail: string?, risky: boolean, icon: string, literal: { string }?, items: { Item }? }

local function s(key: string, args: { [string]: any }?): string
	return ui.t(key, args):get()
end

-- Where each of Zeron's named elements is, as a person would say it (the
-- host's map, src/plugins/elements.rs, says the same): its message's key.
local ELEMENTS: { [string]: string } = {
	["#shell"] = "plugins.words.place.window",
	["#sidebar"] = "plugins.words.place.sidebar",
	[".session-row"] = "plugins.words.place.sidebar_row",
	[".session-title"] = "plugins.words.place.sidebar_row",
	[".section-header"] = "plugins.words.place.sidebar",
	["#sidebar-actions"] = "plugins.words.place.sidebar",
	["#titlebar"] = "plugins.words.place.title_bar",
	["#titlebar-actions"] = "plugins.words.place.title_bar",
	["#rail"] = "plugins.words.place.rail",
	["#settings-nav"] = "plugins.words.place.settings",
	[".settings-nav-row"] = "plugins.words.place.settings",
	["#settings-page"] = "plugins.words.place.settings",
	["#chat-home"] = "plugins.words.place.new_chat",
	["#new-chat-background"] = "plugins.words.place.new_chat_background",
	["#chat-session"] = "plugins.words.place.chat",
	["#session-header"] = "plugins.words.place.chat_header",
	["#transcript"] = "plugins.words.place.chat",
	[".message"] = "plugins.words.place.message",
	[".message-actions"] = "plugins.words.place.message",
	["#chat-composer"] = "plugins.words.place.message_box",
	["#composer-toolbar"] = "plugins.words.place.message_box",
	["#files-pane"] = "plugins.words.place.files_pane",
	["#file-tree"] = "plugins.words.place.files_pane",
	["#files-header"] = "plugins.words.place.files_pane",
	[".file-row"] = "plugins.words.place.file_row",
	[".file-tab"] = "plugins.words.place.file_tab",
	["#file-viewer"] = "plugins.words.place.open_file",
	["#panel"] = "plugins.words.place.side_panel",
	["#panel-strip"] = "plugins.words.place.side_panel",
	[".panel-tab"] = "plugins.words.place.side_panel",
	["#panel-empty"] = "plugins.words.place.side_panel",
}

-- Where a screen's first part puts it.
local AREAS: { [string]: string } = {
	shell = "plugins.words.place.window",
	sidebar = "plugins.words.place.sidebar",
	rail = "plugins.words.place.rail",
	titlebar = "plugins.words.place.title_bar",
	settings = "plugins.words.place.settings",
	chat = "plugins.words.place.chats",
	files = "plugins.words.place.files_pane",
	panel = "plugins.words.place.side_panel",
}

-- A screen module's name, as a person would say it.
local SCREENS: { [string]: string } = {
	["@app/screens/shell"] = "plugins.words.place.main_window",
	["@app/screens/shell/sidebar"] = "plugins.words.place.sidebar",
	["@app/screens/shell/title_bar"] = "plugins.words.place.title_bar",
	["@app/screens/shell/page"] = "plugins.words.place.page_area",
	["@app/screens/chat"] = "plugins.words.place.chat_page",
	["@app/screens/chat/home"] = "plugins.words.place.new_chat_page",
	["@app/screens/chat/session"] = "plugins.words.place.chat",
	["@app/screens/chat/composer"] = "plugins.words.place.message_box",
	["@app/screens/files"] = "plugins.words.place.files_pane",
}

function M.place(target: string): string
	if SCREENS[target] then
		return s(SCREENS[target])
	end
	-- A selector's last part (`#a >> .b` is where .b is), then the element
	-- it starts with (`.message.message-user` is a message).
	local last = string.match(target, ">>%s*([^>]-)%s*$") or string.match(target, "^%s*(.-)%s*$") or target
	if ELEMENTS[last] then
		return s(ELEMENTS[last])
	end
	local head = string.match(last, "^([#.][%w_-]+)")
	if head and ELEMENTS[head] then
		return s(ELEMENTS[head])
	end
	local screen = string.match(target, "^@app/screens/([%w_]+)")
	if screen and AREAS[screen] then
		return s(AREAS[screen])
	end
	return s("plugins.words.place.zeron")
end

-- "the sidebar and the title bar": each place once, in order.
function M.places(targets: { string }?): string
	local seen: { [string]: boolean } = {}
	local out: { string } = {}
	for _, t in targets or {} do
		local p = M.place(t)
		if not seen[p] then
			seen[p] = true
			table.insert(out, p)
		end
	end
	if #out == 0 then
		return s("plugins.words.place.zeron_parts")
	elseif #out == 1 then
		return out[1]
	end
	local last = table.remove(out) :: string
	return s("plugins.words.list", { rest = table.concat(out, s("plugins.words.list_sep")), last = last })
end

-- `list`'s strings, as given, at most `most` (the rest counted).
local function literal(list: { any }?, most: number?): { string }?
	local out: { string } = {}
	local n = most or 12
	for i, v in list or {} do
		if type(v) == "string" then
			if i > n then
				table.insert(out, s("plugins.words.more", { n = #(list :: any) - n }))
				break
			end
			table.insert(out, v)
		end
	end
	return if #out > 0 then out else nil
end

-- Programs by name, those in `anything` (the host's rating; all of them
-- when it gave none) saying they can do anything you can.
local function programs_said(names: { any }?, anything: any): { list: { string }, anything: boolean }
	local list: { string } = {}
	local any = false
	for _, n in names or {} do
		if type(n) == "string" then
			local all = type(anything) ~= "table" or table.find(anything, n) ~= nil
			any = any or all
			table.insert(list, if all then s("plugins.words.can_anything", { name = n }) else n)
		end
	end
	return { list = list, anything = any }
end

-- Hosts as written ("*": any site), those in `near` (on this device or
-- your network, the host's rating) said so.
local function hosts_said(hosts: { any }?, near: any): { list: { string }, near: boolean }
	local list: { string } = {}
	local any_near = false
	for _, h in hosts or {} do
		if type(h) == "string" then
			if h == "*" then
				table.insert(list, s("plugins.words.any_site"))
			elseif type(near) == "table" and table.find(near, h) ~= nil then
				any_near = true
				table.insert(list, s("plugins.words.near_host", { host = h }))
			else
				table.insert(list, h)
			end
		end
	end
	return { list = list, near = any_near }
end

-- What of Zeron's interface it reaches, exactly as it names it: a detail
-- under its asks, never an ask of its own. `{ { title, list } }`, empty
-- when it reaches none.
function M.details(item: any): { { title: string, list: { string } } }
	local out = {}
	local lists = item.ui or {}
	local change = literal(lists.change, 40)
	if change then
		table.insert(out, { title = s("plugins.words.details.changes"), list = change })
	end
	local replace = literal(lists.replace, 40)
	if replace then
		table.insert(out, { title = s("plugins.words.details.replaces"), list = replace })
	end
	local grew = literal(type(item.digest_diff) == "table" and item.digest_diff.ui_change_added or nil, 40)
	if grew then
		table.insert(out, { title = s("plugins.words.details.changes_since"), list = grew })
	end
	return out
end

-- Its agent tools, each by name with what it tells agents.
local function tool_items(list: { any }?): { Item }?
	local out: { Item } = {}
	for _, t in list or {} do
		if type(t) == "table" and type(t.name) == "string" then
			table.insert(out, { name = t.name, description = if type(t.description) == "string" and t.description ~= "" then t.description else nil })
		end
	end
	return if #out > 0 then out else nil
end

local function tools(item: any): string
	local names: { string } = {}
	for _, t in item.tools or {} do
		table.insert(names, t.name)
	end
	return if #names > 0 then table.concat(names, ", ") else s("plugins.words.none_listed")
end

-- A permission's line: its words read through their messages (`title`'s
-- key, `detail`'s key or its own words), with `args` as they are then.
local function line(risky: boolean, icon: string, title: string, detail: (string | () -> string)?, args: (() -> { [string]: any })?, extra: { literal: { string }?, items: { Item }? }?): Line
	return setmetatable({ risky = risky, icon = icon, literal = extra and extra.literal, items = extra and extra.items }, {
		__index = function(_, field: string): string?
			if field == "title" then
				return s(title, if args then args() else nil)
			elseif field == "detail" and detail ~= nil then
				return if type(detail) == "function" then detail() else s(detail :: string, if args then args() else nil)
			end
			return nil
		end,
	}) :: any
end

function M.permission(item: any, permission: string): Line
	local ui_lists = item.ui or {}
	if permission == "ui.add" then
		return line(false, "widget", "plugins.words.permission.ui_add.title")
	elseif permission == "ui.change" then
		-- (Where, exactly, is a detail: `M.details`. No part of Zeron's
		-- interface is an ask of its own.)
		return line(false, "pen", "plugins.words.permission.ui_change.title", "plugins.words.permission.ui_change.detail", function()
			return { places = M.places(ui_lists.change) }
		end)
	elseif permission == "ui.replace" then
		return line(true, "danger-triangle", "plugins.words.permission.ui_replace.title", "plugins.words.permission.ui_replace.detail", function()
			return { places = M.places(ui_lists.replace) }
		end)
	elseif permission == "profile.read" then
		return line(false, "user-rounded", "plugins.words.permission.profile_read.title")
	elseif permission == "sessions.read" then
		return line(false, "list", "plugins.words.permission.sessions_read.title")
	elseif permission == "messages.read" then
		return line(false, "chat-round-line", "plugins.words.permission.messages_read.title")
	elseif permission == "account.email" then
		return line(false, "eye", "plugins.words.permission.account_email.title")
	elseif permission == "files.read" then
		return line(false, "folder", "plugins.words.permission.files_read.title")
	elseif permission == "panel.tabs" then
		return line(false, "split-columns", "plugins.words.permission.panel_tabs.title")
	elseif permission == "pages.add" then
		return line(false, "list", "plugins.words.permission.pages_add.title")
	elseif permission == "github" then
		return line(false, "pull-request", "plugins.words.permission.github.title", "plugins.words.permission.github.detail")
	elseif permission == "github.write" then
		return line(false, "chat-round-line", "plugins.words.permission.github_write.title")
	elseif permission == "notifications" then
		return line(false, "bell", "plugins.words.permission.notifications.title", "plugins.words.permission.notifications.detail")
	elseif permission == "links.open" then
		return line(false, "arrow-up-right", "plugins.words.permission.links_open.title", "plugins.words.permission.links_open.detail")
	elseif permission == "navigate" then
		return line(false, "map-arrow-right", "plugins.words.permission.navigate.title")
	elseif permission == "compose" then
		return line(false, "chat-round-line", "plugins.words.permission.compose.title", "plugins.words.permission.compose.detail")
	elseif permission == "network" then
		-- The places it names (any site, with "*"), those on this device or
		-- your network said so, and risky then.
		local hosts = item.network_hosts
		if type(hosts) == "table" and #hosts > 0 then
			local any = table.find(hosts, "*") ~= nil
			local listed = hosts_said(hosts, item.network_local)
			return line(listed.near, "global", if any then "plugins.words.permission.network.title_any" else "plugins.words.permission.network.title_hosts", nil, nil, { literal = listed.list })
		end
		return line(false, "global", "plugins.words.permission.network.title")
	elseif permission == "clipboard" then
		return line(false, "copy", "plugins.words.permission.clipboard.title")
	elseif permission == "files" then
		return line(false, "document", "plugins.words.permission.files.title")
	elseif permission == "web" then
		return line(false, "global", "plugins.words.permission.web.title")
	elseif permission == "windows" then
		return line(false, "monitor", "plugins.words.permission.windows.title")
	elseif permission == "tools" then
		-- Each tool by name, with what it tells your agents (their words
		-- reach every agent's list of tools).
		local items = tool_items(item.tools)
		return line(false, "magic-stick", "plugins.words.permission.tools.title", if items then nil else function(): string
			return tools(item)
		end, nil, { items = items })
	elseif permission == "devices" then
		return line(false, "smartphone", "plugins.words.permission.devices.title")
	elseif permission == "service" then
		-- Its headless half, run by the engine on the device that allowed it
		-- (another device's sheet names that one).
		local on = (item :: any).on_device
		return line(
			false,
			"action-play",
			if type(on) == "string" then "plugins.words.permission.service.title_on" else "plugins.words.permission.service.title",
			"plugins.words.permission.service.detail",
			function()
				return { device = on }
			end
		)
	elseif permission == "commands" then
		-- The programs its manifest names, as you: what they can do, it can.
		-- Each by name; one that can do anything you can (a shell, git,
		-- an interpreter, any not known to be contained) says so.
		local names = {}
		for _, c in item.commands or {} do
			table.insert(names, c)
		end
		local on = (item :: any).on_device
		local said = programs_said(names, item.commands_anything)
		return line(
			true,
			"terminal",
			if type(on) == "string" then "plugins.words.permission.commands.title_on" else "plugins.words.permission.commands.title",
			if said.anything then "plugins.words.permission.commands.detail_anything" else "plugins.words.permission.commands.detail",
			function()
				return { list = if #names > 0 then table.concat(names, ", ") else s("plugins.words.programs"), device = on }
			end,
			{ literal = if said.anything then said.list else nil }
		)
	elseif permission == "native" then
		local on = (item :: any).on_device
		return line(
			true,
			"danger-triangle",
			if type(on) == "string" then "plugins.words.permission.native.title_on" else "plugins.words.permission.native.title",
			"plugins.words.permission.native.detail",
			function()
				return { device = on }
			end
		)
	end
	return { title = permission, risky = true, icon = "danger-triangle" }
end

-- What grew since you allowed it (the engine's `digest_diff`, of what it
-- runs, reaches and tells agents beyond its permissions' names): a line
-- each, risky where it runs or reaches more. Empty when nothing did.
function M.diff(item: any): { Line }
	local d = item.digest_diff
	local out: { Line } = {}
	if type(d) ~= "table" then
		return out
	end
	local function some(list: any): boolean
		return type(list) == "table" and #list > 0
	end
	if some(d.added_commands) then
		table.insert(out, line(true, "terminal", "plugins.words.diff.commands", nil, nil, { literal = programs_said(d.added_commands, d.commands_anything).list }))
	end
	if type(d.mcp_changed) == "table" then
		local after = d.mcp_changed.after
		local said = nil
		if type(after) == "table" and type(after.command) == "string" then
			local parts = { after.command }
			for _, a in (if type(after.args) == "table" then after.args else {}) :: { any } do
				table.insert(parts, tostring(a))
			end
			said = { table.concat(parts, " ") }
			if type(after.env) == "table" then
				for k in after.env do
					table.insert(said, `{k}=…`)
				end
			end
		end
		table.insert(out, line(true, "danger-triangle", if after == nil then "plugins.words.diff.mcp_gone" else "plugins.words.diff.mcp", nil, nil, { literal = said }))
	end
	-- (What of Zeron's interface it changes is never an ask: `M.details`.
	-- A screen it replaces runs as Zeron's own code: an ask, as running
	-- a program is.)
	if some(d.ui_replace_added) then
		table.insert(out, line(true, "danger-triangle", "plugins.words.diff.ui_replace", "plugins.words.permission.ui_replace.detail", function()
			return { places = M.places(d.ui_replace_added) }
		end, { literal = literal(d.ui_replace_added) }))
	end
	if some(d.tools_added) then
		table.insert(out, line(false, "magic-stick", "plugins.words.diff.tools_added", nil, nil, { items = tool_items(d.tools_added) }))
	end
	if some(d.tools_changed) then
		local items: { Item } = {}
		for _, t in d.tools_changed do
			if type(t) == "table" and type(t.name) == "string" then
				table.insert(items, { name = t.name, description = if type(t.description_after) == "string" then t.description_after else nil })
			end
		end
		table.insert(out, line(false, "magic-stick", "plugins.words.diff.tools_changed", nil, nil, { items = items }))
	end
	if type(d.service_changed) == "table" then
		local after = d.service_changed.after
		table.insert(out, line(true, "action-play", "plugins.words.diff.service", nil, nil, { literal = if type(after) == "string" then { after } else nil }))
	end
	if type(d.entry_changed) == "table" then
		local after = d.entry_changed.after
		table.insert(out, line(false, "document", "plugins.words.diff.entry", nil, nil, { literal = if type(after) == "string" then { after } else nil }))
	end
	if some(d.network_added) then
		local listed = hosts_said(d.network_added, d.network_local)
		table.insert(out, line(listed.near, "global", "plugins.words.diff.network", nil, nil, { literal = listed.list }))
	end
	if d.settings_changed == true then
		table.insert(out, line(false, "settings", "plugins.words.diff.settings"))
	end
	local risky: { Line } = {}
	local calm: { Line } = {}
	for _, l in out do
		table.insert(if l.risky then risky else calm, l)
	end
	for _, l in risky do
		table.insert(calm, l)
	end
	return calm
end

-- `item` as it stands on another device, `name`: lines that say where
-- it runs say there (the item itself unchanged).
function M.on_device(item: any, name: string): any
	return setmetatable({ on_device = name }, { __index = item })
end

-- Each of `permissions`' lines, the risky ones last.
function M.lines(item: any, permissions: { string }?): { Line }
	local calm: { Line } = {}
	local risky: { Line } = {}
	for _, p in permissions or {} do
		local line = M.permission(item, p)
		table.insert(if line.risky then risky else calm, line)
	end
	for _, line in risky do
		table.insert(calm, line)
	end
	return calm
end

function M.author(item: any): string
	local a = item.author or {}
	if a.kind == "zeron" then
		return s("plugins.words.author.zeron")
	elseif a.kind == "you" then
		return s("plugins.words.author.you")
	elseif a.kind == "verified" and a.username then
		return s("plugins.words.author.verified", { username = a.username })
	end
	-- (Who made it can't be told: where it came from instead, beside the
	-- Unverified badge.)
	return M.from(item.source) or s("plugins.words.author.unknown")
end

-- Whether Zeron can't vouch for who made it: someone else's, signed by no
-- one Zeron knows (Zeron's own and yours never). A template's item is its
-- `imported`'s author, as a plugin's is its own.
function M.unverified(item: any): boolean
	if type(item) ~= "table" or item.builtin or item.own then
		return false
	end
	local a = item.author
	return type(a) == "table" and a.kind == "unknown"
end

-- Where someone else's came from, as a person reads a link
-- ("github.com/ada/clock"), or nil.
function M.where(source: any): string?
	if type(source) ~= "table" or type(source.url) ~= "string" or source.url == "" then
		return nil
	end
	local w = string.gsub(source.url, "^https?://", "")
	w = string.gsub(w, "%.git$", "")
	w = string.gsub(w, "/$", "")
	return w
end

-- "From github.com/ada/clock", "From a file", or nil.
function M.from(source: any): string?
	local w = M.where(source)
	if w then
		return s("plugins.words.from", { where = w })
	elseif type(source) == "table" and source.kind == "file" then
		return s("plugins.words.from_file")
	end
	return nil
end

-- What a plugin's row says under its name.
function M.about(item: any): string
	if item.failed then
		return s("plugins.words.about.failed", { reason = item.failed })
	elseif item.status == "broken" then
		return item.error or s("plugins.words.about.broken")
	elseif item.status == "tampered" then
		return s("plugins.words.about.tampered")
	elseif item.status == "pending" then
		if item.allowed and #item.allowed > 0 then
			return s("plugins.words.about.asks_more")
		end
		return s("plugins.words.about.waiting")
	end
	return if item.description and item.description ~= "" then item.description else s("plugins.words.about.no_description")
end

-- What one of Zeron's own is allowed, said under its description: they're
-- allowed what they ask without asking you, so it's never out of sight.
-- Nil for any other, or one that isn't working.
function M.grants(item: any): string?
	if not item.builtin or item.failed or (item.status ~= nil and item.status ~= "on" and item.status ~= "off") then
		return nil
	end
	local can: { string } = {}
	for _, line in M.lines(item, item.allowed or item.permissions) do
		table.insert(can, string.lower(string.sub(line.title, 1, 1)) .. string.sub(line.title, 2))
	end
	return if #can > 0 then s("plugins.words.grants", { list = table.concat(can, s("plugins.words.list_sep")) }) else nil
end

-- A newer version found where someone else's came from, said under its
-- description: its version and what it changes in what it asks (new asks
-- are allowed in the consent window after it's updated). Nil when there's
-- none.
function M.update(item: any): string?
	local update = item.update
	if type(update) ~= "table" or item.builtin then
		return nil
	end
	local version = if type(update.version) == "string" and update.version ~= "" then update.version else nil
	if type(update.refused) == "string" then
		return if version
			then s("plugins.words.update.refused", { version = version, reason = update.refused })
			else s("plugins.words.update.refused_unversioned", { reason = update.refused })
	end
	local function said(list: { string }?): string?
		local out: { string } = {}
		for _, line in M.lines(item, list or {}) do
			table.insert(out, string.lower(string.sub(line.title, 1, 1)) .. string.sub(line.title, 2))
		end
		return if #out > 0 then table.concat(out, s("plugins.words.list_sep")) else nil
	end
	local parts = {
		if version then s("plugins.words.update.available", { version = version }) else s("plugins.words.update.available_unversioned"),
	}
	local added = said(update.added)
	if added then
		table.insert(parts, s("plugins.words.update.added", { list = added }))
	end
	local removed = said(update.removed)
	if removed then
		table.insert(parts, s("plugins.words.update.removed", { list = removed }))
	end
	return table.concat(parts, " ")
end

-- A plugin's tile hue, steady per id.
local HUES = { "indigo", "rose", "sky", "emerald", "amber", "violet", "orange", "teal" }
-- A plugin's tile, `size` points: the icon it ships (an SVG, read by the
-- host: `icon_svg`) as a glyph in the theme's ink on the quiet wash, else
-- its name's initial on its hue.
function M.tile(p: any, size: number): Node
	local ds = require("@ds")
	local svg = p.icon_svg
	if type(svg) == "string" and svg ~= "" then
		local g = math.floor(size * 0.58 + 0.5)
		return ds.Tile {
			label = p.name,
			size = size,
			inside = ui.svg { code = svg, w = g, h = g, shrink = 0, tint = "$text.primary", aria_hidden = true },
		}
	end
	return ds.Tile { label = p.name, hue = M.hue(p.id), size = size }
end
function M.hue(id: string): string
	local n = 0
	for i = 1, #id do
		n = (n * 31 + string.byte(id, i)) % 9973
	end
	return HUES[n % #HUES + 1]
end

return M
