-- How the Map's Topology shows an orchestrator's work on other machines, a
-- choice kept on this device (the Map's … menu, "Lines" or "Tags"):
--   "lines"  its rail runs down a column of its own to them (the default)
--   "tags"   no line across machines: each row there carries an edge in
--            its orchestrator's hue (dashed: started; solid: led) and
--            "↳ its name"
local M = {}
M.MODES = { "lines", "tags" }
M.style = ui.persist("control_map_topo_style", "lines")
function M.mode(): string
	local s = M.style:get()
	return if s == "tags" then "tags" else "lines"
end
return M
