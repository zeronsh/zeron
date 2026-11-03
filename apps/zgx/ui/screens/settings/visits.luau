-- Each time a settings page opens, a visit (screens/visits, as setup's
-- steps have them): what a page asks Zeron as it opens (your profile, the
-- members, this Mac's git settings) is asked once a visit, not again as a
-- hot reload builds the page again.
--
--   visits.once("members", load)   -- the first time this visit
--   visits.again("members", load)  -- asked again now (Try again)

local route = require("@app/screens/shell/route")
local Visits = require("@app/screens/visits")

local v = Visits.new("settings", function(): string?
	local r = route.current:get()
	return if r.name == "settings" then r.page else nil
end)

return {
	visit = v.visit,
	once = v.once,
	again = v.again,
}
