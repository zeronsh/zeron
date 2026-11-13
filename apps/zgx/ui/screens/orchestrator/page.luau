-- An orchestrator's page (`/orchestrator/:id`): a placeholder so the route
-- loads. The ui-orchestrator stream replaces it with the real page (its
-- conversation and its live state).
--
--   Page { id = function() return orchestrator_id end }

local ds = require("@ds")
local orchestration = require("@account/orchestration")

local M = {}

function M.Page(props: { id: () -> string? }): Node
	local function name(): string
		local id = props.id()
		for _, o in orchestration.now:get().orchestrators or {} do
			if o.id == id then
				return o.name
			end
		end
		return ""
	end
	return ui.column {
		id = "orchestrator-page",
		w_full = true,
		h_full = true,
		ds.EmptyState { icon = "users-group-rounded", size = "sm", fill = true, title = name },
	}
end

return M
