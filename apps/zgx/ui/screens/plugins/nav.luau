--!strict
-- Where Settings → Plugins is: its tabs ("catalog"), one store plugin's
-- page ("store:<id>"), or publishing one of yours ("publish:<id>"). Kept
-- apart from the page so what opens a plugin's page from elsewhere (a
-- `zeron://open/plugin/<id>` link, the shell's route) needn't load it.
--
--   nav.view:get()          "catalog" | "store:<id>" | "publish:<id>"
--   nav.tab                 the tab shown: "installed" | "yours" | "browse"
--   nav.open_listing(id)    a store plugin's page (Browse its way back)
--   nav.open_publish(id)    publishing one of yours (Yours its way back)
--   nav.back()              the tabs again
--   nav.arrive()            for a visit: the tabs, unless something opened
--                           a page as Settings came up (a link)

local M = {}

M.view = ui.state("plugins_view", "catalog")
M.tab = ui.state("plugins_tab", "installed")
-- A page opened from outside the page (a link) before the visit began.
local opened = ui.state("plugins_view_opened", false)

-- (Only an id a store id could be, as a plugin's id is: anything else is
-- no page at all.)
local function valid(id: string): boolean
	return #id >= 2 and #id <= 40 and string.match(id, "^[a-z0-9][a-z0-9%-]*$") ~= nil
end

function M.open_listing(id: string, from_outside: boolean?)
	if not valid(id) then
		return
	end
	M.tab:set("browse")
	M.view:set(`store:{id}`)
	opened:set(from_outside == true)
end

function M.open_publish(id: string)
	if not valid(id) then
		return
	end
	M.tab:set("yours")
	M.view:set(`publish:{id}`)
	opened:set(false)
end

function M.back()
	opened:set(false)
	M.view:set("catalog")
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
