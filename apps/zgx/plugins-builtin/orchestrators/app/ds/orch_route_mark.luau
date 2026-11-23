-- How a dump's part stands, as one glyph: Zeron's status where there is
-- one. On its way, the quiet ring; sent (delivered), a quiet check; its
-- place at work on it and saying how it goes (answered), the working ring;
-- finished (done), the done check; failed, the failed glyph; moved, the
-- refresh glyph; unsure, nothing (its chip says so). Held 300 ms at least
-- (ds/orch_hold). The receipt's chips and a part's hover card alike.
--
--   RouteMark { state = function() return route.state end, size = 13 }
--   RouteMark.kind("delivered")   -- its word's key: "pending", "sent", …

local Icon = require("@ds/icon")
local status = require("@ds/status")
local hold = require("@self/orch_hold")

local M = {}

-- What each state shows: a status kind, or one of the plain glyphs.
local SHOWN: { [string]: string } = {
	pending = "pending",
	delivered = "sent",
	answered = "working",
	done = "done",
	failed = "failed",
	undone = "undone",
	unsure = "unsure",
}

function M.kind(state: string?): string
	return SHOWN[state or "pending"] or "pending"
end

function M.Mark(props: { state: () -> string, size: number? }): Node
	local size = props.size or 13
	local shown = hold.value(function(): string
		return M.kind(props.state())
	end)
	return ui.switch(shown, {
		pending = ui.div { class = "orch-route-mark", status.Spinner { size = size - 1, ink = "$text.tertiary" } },
		sent = ui.div { class = "orch-route-mark", color = "$text.tertiary", Icon { name = "check", size = size - 1 } },
		working = ui.div { class = "orch-route-mark", status.StatusIcon { kind = "working", size = size } },
		done = ui.div { class = "orch-route-mark", status.StatusIcon { kind = "done", size = size } },
		failed = ui.div { class = "orch-route-mark", status.StatusIcon { kind = "failed", size = size } },
		undone = ui.div { class = "orch-route-mark", color = "$text.tertiary", Icon { name = "refresh", size = size - 1 } },
		unsure = ui.div {},
	}, ui.div {})
end

return M
