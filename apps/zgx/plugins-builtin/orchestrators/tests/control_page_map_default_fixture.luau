-- The Control room page's Map view default (screens/control/page
-- `MAP_DEFAULT`), written out so the page loads without the Map, against
-- the Map's own (ds/orch_map `DEFAULT`): `map_default_same` says whether
-- they agree.
local page = require("@orchestrators/screens/control/page")
local map = require("@orchestrators/ds/orch_map")
local same = true
for k, v in map.DEFAULT do
	if page.MAP_DEFAULT[k] ~= v then
		same = false
	end
end
for k, v in page.MAP_DEFAULT do
	if map.DEFAULT[k] ~= v then
		same = false
	end
end
ui.state("map_default_same", false):set(same)
return ui.column {}
