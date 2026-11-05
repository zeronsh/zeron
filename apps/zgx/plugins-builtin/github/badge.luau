--!strict
-- A session's pull request beside its title in the sidebar: the design
-- system's pull request badge (ds/badge, the sidebar's own badge
-- generalised: its state's glyph and number in its state's colour,
-- `$zeron.pr.*`, on a wash of it), lit under the pointer as Zeron's sidebar
-- badge brightens (crates/ui/src/change_requests.rs `render_badge_model`).
-- A click opens its session and the pull request in that chat's panel.
--
-- No tooltip, on purpose: the session's hover card says its state and title
-- (card_row.luau), as Zeron's sidebar has it, rather than a second card
-- over the first.

local Badge = require("@ds/badge")

export type Pr = { number: number, title: string?, url: string?, state: string? }

local STATES: { [string]: boolean } = { open = true, merged = true, closed = true, draft = true }

return ui.component(function(props: { pr: () -> Pr?, on_open: () -> (), words: any }): Node
	local hovered = ui.use_state(false)
	local function state(): string
		local now = props.pr()
		return if now and STATES[now.state or ""] then now.state :: string else "open"
	end
	return ui.row {
		class = "github-pr-badge",
		shrink = 0,
		cursor = "pointer",
		role = "link",
		label = function(): string
			local now = props.pr()
			return if now then props.words.s("badge.open", { number = now.number, title = now.title or "" }) else props.words.s("badge.plain")
		end,
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
		on_click = props.on_open,
		-- Its state and number followed: the badge drawn again as they move,
		-- the link round it kept.
		ui.presence(function(): string
			local now = props.pr()
			return `{state()}:{if now then now.number else ""}`
		end, function(): Node
			local now = props.pr()
			return Badge.PullRequest {
				state = state() :: any,
				number = if now then now.number else nil,
				size = "sm",
				lit = function(): boolean
					return hovered:get()
				end,
			}
		end),
	}
end)
