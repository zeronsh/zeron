-- Where a web link in prose goes when it is clicked: the system's browser
-- (zgx's own `href`), or whatever the app sets here (its browser tab beside
-- the chat, with "Open links in Zeron" on). ds/markdown reads it as it
-- makes a link; the shell sets it. `opener(url)` answers true when it took
-- the link, false to let it open outside.
--
--   Links.opener = function(url) return panel.open_url(session, url) end
--   Links.panel = function(url) … end   the browser tab, whatever the setting
--   Links.in_app = function() return prefs.open_links:peek() == "zeron" end
--
-- A link's context menu (ds/menu_at shows it), by its kind:
--   web       Open, Open in browser panel or in default browser (whichever
--             Open isn't), Copy link
--   mail      Open, Copy address
--   anything  Open (its document's `open`), Copy link
-- A file or a chat is its app's to say (ds/markdown's `link_menu`): the
-- entries `file` and `chat` make.
--
--   span.href = url
--   span.on_click = function() return Links.take(url) end
--   span.context_menu = Links.menu_for(url)

-- (By the alias the shell mounts its host by: one instance per path.)
local MenuAt = require("@ds/menu_at")
local locale = require("@self/locale")

local M = {}

export type Entry = MenuAt.Entry

M.opener = nil :: ((url: string) -> boolean)?
-- The app's browser tab beside what shows, whatever "Open links in" says:
-- true when it took the link (nil: the app has none).
M.panel = nil :: ((url: string) -> boolean)?
-- Whether that tab can take one now (a chat or a page open); without it,
-- whenever there's `panel`.
M.panel_ready = nil :: (() -> boolean)?
-- Whether Open goes to the app ("Open links in" Zeron). Without it, to the
-- app whenever it has an opener.
M.in_app = nil :: (() -> boolean)?
-- Whether this VM may write the clipboard (a plugin's may not without its
-- "clipboard" permission: Copy is left out of its menus then).
M.can_copy = true
-- Whether this VM opens links outside itself (`ui.invoke("open_url")`): a
-- plugin's opens them only where Zeron does (its opener).
M.system = true

-- Whether the app takes links now (a function, so a span follows it).
function M.taken(): boolean
	return M.opener ~= nil
end

-- Whether Open goes to the app now.
local function app_first(): boolean
	local in_app = M.in_app
	if in_app then
		return in_app()
	end
	return M.opener ~= nil
end

-- The last link opened, and how (a look, for devtools and tests).
M.last = ui.state("links_last", "")

-- Whether `url` is a web or mail link (one zgx's `href` opens).
function M.web(url: string): boolean
	return string.match(url, "^https?://") ~= nil or string.match(url, "^mailto:") ~= nil
end

-- A link's click (a span with `href`): the app's opener takes it (true:
-- its href stays shut), else false, and zgx opens the href outside.
function M.take(url: string): boolean
	local opener = M.opener
	if opener and opener(url) then
		M.last:set(`app:{url}`)
		return true
	end
	M.last:set(`system:{url}`)
	return false
end

-- Opens `url` outside, in the default browser (mail in the mail app).
-- (Where the host opens them: a test's has none.)
function M.outside(url: string)
	M.last:set(`system:{url}`)
	pcall(ui.invoke, "open_url", url)
end

-- Opens `url` where links go: the app's opener, else the system's
-- browser (an `https:` link; the host opens nothing else).
function M.open(url: string)
	if not M.take(url) then
		pcall(ui.invoke, "open_url", url)
	end
end

-- Puts `text` on the clipboard (where this VM may).
function M.copy(text: string)
	if M.can_copy then
		pcall(ui.clipboard.write, text)
	end
end

local function item(key: string, icon: string, on_select: () -> ()): Entry
	return { label = locale.s(key), icon = icon :: any, on_select = on_select }
end

-- A copy entry, unless this VM may not copy.
local function copy_item(into: { Entry }, key: string, text: string)
	if M.can_copy then
		table.insert(into, item(key, "copy", function()
			M.copy(text)
		end))
	end
end

-- A web link's entries: Open (where the setting says), the other place,
-- Copy link. A mail link's: Open, Copy address.
function M.web_items(url: string): { Entry }
	local out: { Entry } = { item("ds.links.open", "arrow-up-right", function()
		M.open(url)
	end) }
	if string.match(url, "^mailto:") then
		local address = string.match(url, "^mailto:([^?]*)") or url
		copy_item(out, "ds.links.copy_address", address)
		return out
	end
	local panel = M.panel
	if app_first() then
		if not M.system then
			copy_item(out, "ds.links.copy_link", url)
			return out
		end
		table.insert(out, item("ds.links.open_outside", "global", function()
			M.outside(url)
		end))
	elseif panel and (M.panel_ready == nil or M.panel_ready()) then
		table.insert(out, item("ds.links.open_in_panel", "split-columns", function()
			if panel(url) then
				M.last:set(`app:{url}`)
			else
				M.outside(url)
			end
		end))
	end
	copy_item(out, "ds.links.copy_link", url)
	return out
end

-- A web link whose click opens it outside whatever the setting (Settings'
-- own: About's, an agent's source): Open (`open`, else outside), Copy link.
function M.outside_items(url: string, open: (() -> ())?): { Entry }
	local out: { Entry } = { item("ds.links.open", "arrow-up-right", open or function()
		M.outside(url)
	end) }
	copy_item(out, "ds.links.copy_link", url)
	return out
end

-- A node's `context_menu` for a web link opening outside (`url()`: where it
-- goes now; `open`: its click).
function M.outside_menu(url: () -> string?, open: (() -> ())?): (e: PointerEvent) -> { Node }?
	return function(e: PointerEvent): { Node }?
		local now = url()
		if not now or now == "" then
			return nil
		end
		return MenuAt.show(e, M.outside_items(now, open))
	end
end

-- A file's entries: Open, Reveal in Finder (with `reveal`: on this Mac),
-- Copy path (with `path`: its whole path).
export type FileFacts = { open: (() -> ())?, reveal: (() -> ())?, path: string? }
function M.file(f: FileFacts): { Entry }
	local out: { Entry } = {}
	if f.open then
		table.insert(out, item("ds.links.open", "document", f.open))
	end
	if f.reveal then
		table.insert(out, item("ds.links.reveal", "folder", f.reveal))
	end
	if f.path then
		copy_item(out, "ds.links.copy_path", f.path)
	end
	return out
end

-- A chat's entries: Open, Copy link (`copy`: puts its link on the
-- clipboard, which may have to be asked for first).
export type ChatFacts = { open: () -> (), copy: (() -> ())? }
function M.chat(c: ChatFacts): { Entry }
	local out: { Entry } = { item("ds.links.open", "chat-round-line", c.open) }
	if c.copy and M.can_copy then
		table.insert(out, item("ds.links.copy_link", "copy", c.copy))
	end
	return out
end

-- Any other link's entries (a document's own: its `open`): Open, Copy link.
function M.other(target: string, open: (() -> ())?): { Entry }
	local out: { Entry } = {}
	if open then
		table.insert(out, item("ds.links.open", "arrow-up-right", open))
	end
	copy_item(out, "ds.links.copy_link", target)
	return out
end

-- A span's (or a node's) `context_menu` for `target`: `entries()` asked as
-- it opens, else the web's for a web link; shown by ds/menu_at.
function M.menu_for(target: string, entries: (() -> { Entry }?)?): (e: PointerEvent) -> { Node }?
	return function(e: PointerEvent): { Node }?
		local list = if entries then entries() else nil
		if list == nil and M.web(target) then
			list = M.web_items(target)
		end
		return MenuAt.show(e, list)
	end
end

return M
