-- A session's card, as Zeron's hover card (crates/ui shell/session_hover_card.rs):
-- its title (three lines at most), how long ago it moved, where it runs
-- (this device's glyph, or a remote one), its project and its branch. One
-- surface, the menus' glass (ds/glass): what floats over the page. It takes
-- no pointer; whoever places it shows and hides it (the sidebar, on a rest).
--
--   SessionCard { title = fn, time = fn, device = fn, here = fn,
--                 project = fn, branch = fn, session = id }
--
-- Plugins add rows under its details (the GitHub plugin's pull request, as
-- Zeron's card has it): `zeron.place { at = ".session-card-end", after = … }`,
-- reading `zeron.need("session")`, the card's session (`session`).
--
-- Each prop a value or a function of state: `time` a time (`os.time()`
-- seconds) or text; `device`, `project`, `branch` nil when there's none
-- (the project then reads "No project").

local Glass = require("@self/glass")
local Icon = require("@self/icon")
local status = require("@self/status")
local locale = require("@self/locale")

export type Props = {
	title: string | () -> string,
	time: (number | string)? | () -> (number | string)?,
	device: string? | () -> string?,
	here: boolean? | () -> boolean?,
	project: string? | () -> string?,
	branch: string? | () -> string?,
	-- The session it's of, for what plugins add to it.
	session: string?,
}

local W = 284

local function read<T>(v: T | () -> T): T
	return if type(v) == "function" then (v :: any)() else v :: T
end

local function line(icon: Icon.Name | () -> Icon.Name, text: () -> string): Node
	return ui.row {
		w_full = true,
		gap = "$space.2",
		items = "center",
		ui.div {
			shrink = 0,
			color = "$text.tertiary",
			if type(icon) == "function"
				then ui.presence(icon :: any, function(name: string): Node
					return Icon { name = name :: Icon.Name, size = 12 }
				end)
				else Icon { name = icon :: Icon.Name, size = 12 },
		},
		ui.text { text, grow = 1, min_w = 0, truncate = true, text_size = "$font.size.xs", color = "$text.secondary" },
	}
end

return function(props: Props): Node
	return Glass.Panel {
		kind = "menu",
		class = "session-card",
		provide = if props.session then { session = props.session } else nil,
		w = W,
		p = "$space.3",
		gap = "$space.2",
		pointer_events = "none",
		ui.text {
			function(): string
				return read(props.title)
			end,
			w_full = true,
			wrap = true,
			lines = 3,
			text_size = "$font.size.md",
			weight = "$font.weight.medium",
			color = "$text.primary",
		},
		ui.column {
			w_full = true,
			gap = "$space.1",
			line("clock-circle", function(): string
				local t = read(props.time)
				if type(t) ~= "number" then
					return t or ""
				end
				local secs = status.since(t)
				return if secs < 60 then locale.s("time.just_now") else locale.s("ds.session_card.ago", { when = status.ago(secs) })
			end),
			ui.show(function(): boolean
				return read(props.device) ~= nil
			end, line(function(): Icon.Name
				return if read(props.here) then "laptop" else "remote-server"
			end, function(): string
				return read(props.device) or ""
			end)),
			line("folder", function(): string
				return read(props.project) or locale.s("ds.session_card.no_project")
			end),
			ui.show(function(): boolean
				return read(props.branch) ~= nil
			end, line("git-branch", function(): string
				return read(props.branch) or ""
			end)),
			-- Where plugins' rows go, after the details (no size of its own).
			ui.div { class = "session-card-end", w_full = true, h = 0 },
		},
	}
end
