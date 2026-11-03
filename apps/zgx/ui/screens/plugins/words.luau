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
export type Line = { title: string, detail: string?, risky: boolean, icon: string }

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

local function tools(item: any): string
	local names: { string } = {}
	for _, t in item.tools or {} do
		table.insert(names, t.name)
	end
	return if #names > 0 then table.concat(names, ", ") else s("plugins.words.none_listed")
end

-- A permission's line: its words read through their messages (`title`'s
-- key, `detail`'s key or its own words), with `args` as they are then.
local function line(risky: boolean, icon: string, title: string, detail: (string | () -> string)?, args: (() -> { [string]: any })?): Line
	return setmetatable({ risky = risky, icon = icon }, {
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
		return line(false, "magic-stick", "plugins.words.permission.tools.title", function(): string
			return tools(item)
		end)
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
		local names = {}
		for _, c in item.commands or {} do
			table.insert(names, c)
		end
		local on = (item :: any).on_device
		return line(
			true,
			"terminal",
			if type(on) == "string" then "plugins.words.permission.commands.title_on" else "plugins.words.permission.commands.title",
			"plugins.words.permission.commands.detail",
			function()
				return { list = if #names > 0 then table.concat(names, ", ") else s("plugins.words.programs"), device = on }
			end
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
	return s("plugins.words.author.unknown")
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
