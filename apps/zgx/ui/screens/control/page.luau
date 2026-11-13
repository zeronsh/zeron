-- The Control room (`/control`).

local M = {}

function M.Page(): Node
	return ui.column { id = "control-room", w_full = true, h_full = true }
end

return M
