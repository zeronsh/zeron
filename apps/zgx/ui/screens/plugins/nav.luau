--!strict
-- Where Settings → Plugins is: its tabs ("catalog"), one store plugin's
-- page ("store:<id>"), or publishing one of yours ("publish:<id>"). Kept
-- apart from the page so what opens a plugin's page from elsewhere (a
-- `zeron://open/plugin/<id>` link, the shell's route) needn't load it.
--
--   nav.view:get()          "catalog" | "store:<id>" | "publish:<id>"
--   nav.tab                 the tab shown: "installed" | "yours" | "browse"
--   nav.open_listing(id)    a store plugin's page (Browse its way back);
--                           `by_link`: a link opened it (`nav.linked()`),
--                           `version` the one the link chose
--                           (`nav.linked_version(id)`, tracked;
--                           `nav.drop_linked_version()` lets it go)
--   nav.open_publish(id)    publishing one of yours (Yours its way back)
--   nav.back()              the tabs again
--   nav.show_browse()       Browse, wherever a page was opened from (what
--                           a plugin's keyword or publisher narrowed it to)
--   nav.show_tab(tab)       a tab, whatever page was open (a notice's
--                           "Deploy tokens": Yours)
--   nav.arrive()            for a visit: the tabs, unless something opened
--                           a page as Settings came up (a link)

local M = {}

M.view = ui.state("plugins_view", "catalog")
M.tab = ui.state("plugins_tab", "installed")
-- A page opened from outside the page (a link) before the visit began.
local opened = ui.state("plugins_view_opened", false)
-- The store plugin whose page a link opened, until the person goes
-- anywhere else from it: its page's action waits a moment before it takes
-- a click (screens/plugins/listing), as the consent window's Allow does.
local linked = ui.state("plugins_view_linked", nil :: string?)
-- The version a link chose on that page (`?version=`): its Install acts
-- on it and the page says its state. Gone with the link's page.
local linked_version = ui.state("plugins_view_linked_version", nil :: string?)
-- How many links have named a page: each one arms its actions again, even
-- on a page already open (`nav.link_seq(id)`).
local link_seq = ui.state("plugins_view_link_seq", 0)

-- (Only an id a store id could be, as a plugin's id is: anything else is
-- no page at all.)
local function valid(id: string): boolean
	return #id >= 2 and #id <= 40 and string.match(id, "^[a-z0-9][a-z0-9%-]*$") ~= nil
end

-- (A version as a version reads anywhere: the host's check, again.)
local function version_reads(v: string): boolean
	return #v >= 1 and #v <= 32 and string.match(v, "^[%w%.%+%-]+$") ~= nil
end

function M.open_listing(id: string, from_outside: boolean?, by_link: boolean?, version: string?)
	if not valid(id) then
		return
	end
	M.tab:set("browse")
	M.view:set(`store:{id}`)
	opened:set(from_outside == true)
	linked:set(if by_link then id else nil)
	linked_version:set(if by_link and type(version) == "string" and version_reads(version) then version else nil)
	if by_link then
		link_seq:set(link_seq:peek() + 1)
	end
end

-- For plugin `id`'s page: a number that moves with each link naming it (0
-- when none has), tracked: the page arms its actions again as it moves.
function M.link_seq(id: string): number
	return if linked:get() == id then link_seq:get() else 0
end

-- Whether a link opened plugin `id`'s page (read as the page shows).
function M.linked(id: string): boolean
	return linked:peek() == id
end

-- The page of the newest again: the link's choice of a version dropped.
function M.drop_linked_version()
	linked_version:set(nil)
end

-- The version a link chose on plugin `id`'s page, or nil (read as the
-- page shows: a later link to it may choose another).
function M.linked_version(id: string): string?
	return if linked:get() == id then linked_version:get() else nil
end

function M.open_publish(id: string)
	if not valid(id) then
		return
	end
	M.tab:set("yours")
	M.view:set(`publish:{id}`)
	opened:set(false)
	linked:set(nil)
	linked_version:set(nil)
end

function M.back()
	opened:set(false)
	linked:set(nil)
	linked_version:set(nil)
	M.view:set("catalog")
end

local TABS = { installed = true, yours = true, browse = true }

function M.show_tab(tab: string)
	if not TABS[tab] then
		return
	end
	opened:set(false)
	linked:set(nil)
	linked_version:set(nil)
	M.tab:set(tab)
	M.view:set("catalog")
end

function M.show_browse()
	M.show_tab("browse")
end

function M.arrive()
	if opened:peek() then
		opened:set(false)
		return
	end
	if M.view:peek() ~= "catalog" then
		M.view:set("catalog")
	end
end

-- The store plugin shown, or nil.
function M.listing(): string?
	return string.match(M.view:get(), "^store:(.+)$")
end

return M
