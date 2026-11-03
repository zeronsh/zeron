--!strict
-- A session's pull request as a row of its hover card, under its branch, as
-- Zeron's card has it (crates/ui/src/shell/session_hover_card.rs): drawn as
-- the card's own rows are (ds/session_card: its words, quiet, where the
-- rows' words stand), then the pull request's badge, the sidebar's own
-- (ds/badge `PullRequest`: its state's glyph and number in its colour).

local Badge = require("@ds/badge")

local GLYPH: { [string]: string } = {
	open = "pull-request",
	merged = "git-merge",
	closed = "pull-request-closed",
	draft = "pull-request-draft",
}
-- The card's own rows' glyph (ds/session_card).
local GLYPH_SIZE = 12

return function(pr: () -> any): Node
	local function state(): string
		local now = pr()
		local s = if now then now.state else nil
		return if s and GLYPH[s] then s else "open"
	end
	return ui.row {
		class = "github-pr-card-row",
		w_full = true,
		gap = "$space.2",
		items = "center",
		-- (No glyph: the badge after the title carries the pull request's;
		-- its slot kept, so the title stands where the rows' words do.)
		ui.div { w = GLYPH_SIZE, h = GLYPH_SIZE, shrink = 0 },
		ui.text {
			function(): string
				local now = pr()
				return if now then now.title or "" else ""
			end,
			grow = 1,
			min_w = 0,
			truncate = true,
			text_size = "$font.size.xs",
			color = "$text.secondary",
		},
		-- The pull request's badge, the sidebar's (ds/badge).
		ui.presence(function(): string
			local now = pr()
			return `{state()}:{if now then now.number else ""}`
		end, function(): Node
			local now = pr()
			return Badge.PullRequest { state = state() :: any, number = if now then now.number else nil, size = "sm" }
		end),
	}
end
