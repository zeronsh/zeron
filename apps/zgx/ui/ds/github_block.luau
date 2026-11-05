-- GitHub, for Cloud agents: Settings → Cloud's own GitHub block
-- (settings/cloud.rs render_github and its device flow), as setup's
-- Connect GitHub embeds it. GitHub's row, what it's doing on the row's
-- second line, and its one action at the end. Connect asks GitHub for a
-- code (Contacting GitHub…); then a second row shows the code to enter
-- there (a click copies it) and the way to GitHub, while the first waits
-- for the approval; a code that expires or is turned down says so and
-- offers another go. Connected, the account, and the repositories Cloud
-- can open (chosen on GitHub, not here).
--
-- The block keeps the room of its tallest state (the code) in every state,
-- so what's round it never moves. It only shows the state: the page round
-- it runs the device flow.
--
--   GitHubBlock { state = function() return "idle" end, copied = state, on_connect = …, … }

local Settings = require("@self/settings")
local Button = require("@self/button")
local Icon = require("@self/icon")
local progress = require("@self/progress")
local control = require("@self/control")
local locale = require("@self/locale")
local ProgressRing = progress.ProgressRing

export type Phase = "idle" | "starting" | "waiting" | "failed" | "connected"
-- A value, or a function that gives it as it stands (the page round the
-- block runs the device flow, so its code and account arrive later).
type Live<T> = T | () -> T
export type Props = {
	state: () -> Phase,
	-- The code to enter on GitHub, while it waits; the account and its
	-- repositories, once connected.
	code: Live<string?>?,
	account: Live<string?>?,
	repos: Live<{ string }?>?,
	-- What the row says when it went wrong (a code that expired, by default).
	failed: Live<string?>?,
	-- A copy's check, for a moment.
	copied: State<boolean>,
	on_connect: () -> (),
	on_cancel: () -> (),
	on_disconnect: () -> (),
	on_copy: (() -> ())?,
	on_open: (() -> ())?,
	-- Choose on GitHub: the repositories Cloud can open, chosen there.
	on_choose: (() -> ())?,
	-- As tall as its state, not its tallest (a settings page, where what's
	-- under it may move; setup's step keeps the room so nothing does).
	hug: boolean?,
}


-- A row of the block: 12 above and below its two lines (a settings row's).
local ROW = 12 + 18 + 2 + 16 + 12
-- The code's row: its two lines, then the code under them.
local CODE_ROW = 12 + 18 + 2 + 16 + 12 + 28 + 12
local BLOCK = ROW + CODE_ROW + 1 + 2 + 3
local MARK = 32

-- GitHub's mark on the marks' white tile, in the tile's one ink, as the
-- other providers' marks stand.
local function mark(): Node
	return ui.column {
		w = MARK,
		h = MARK,
		shrink = 0,
		rounded = ui.raw(MARK * 0.25, "a mark tile's radius scales with it"),
		items = "center",
		justify = "center",
		bg = "$zeron.mark.bg",
		border = 1,
		border_color = "$zeron.mark.border",
		shadow = "$shadow.sm",
		color = "$zeron.card.ink",
		ui.svg { src = "assets/icons/github-mark.svg", tint = "currentColor", w = MARK * 0.56, h = MARK * 0.56 },
	}
end

-- What GitHub's row says on its second line, by the state: a ring before
-- it while something is under way, the danger's ink when it went wrong.
local function status(p: Props): Node
	local function busy(): boolean
		local s = p.state()
		return s == "starting" or s == "waiting"
	end
	return ui.row {
		gap = "$space.2",
		items = "center",
		min_w = 0,
		ui.show(busy, ProgressRing { size = 12, tone = "neutral", label = locale.s("ds.github_block.connecting") }),
		ui.text {
			function()
				local s = p.state()
				if s == "starting" then
					return locale.s("ds.github_block.contacting")
				elseif s == "waiting" then
					return locale.s("ds.github_block.waiting")
				elseif s == "failed" then
					return (control.value(p.failed) :: string?) or locale.s("ds.github_block.expired")
				elseif s == "connected" then
					return (control.value(p.account) :: string?) or ""
				end
				return locale.s("ds.github_block.not_connected")
			end,
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			color = function()
				return if p.state() == "failed" then "$zeron.tone.danger.ink" else "$text.tertiary"
			end,
		},
	}
end

-- GitHub's one action, by the state.
local function action(p: Props): Node
	local function when(states: { string }, node: Node): Node
		return ui.show(function()
			return table.find(states, p.state()) ~= nil
		end, node)
	end
	return ui.row {
		items = "center",
		when({ "idle" }, Button {
			locale.t("ds.github_block.connect"),
			tone = "secondary",
			size = "sm",
			on_click = p.on_connect,
		}),
		when({ "failed" }, Button {
			locale.t("common.retry"),
			tone = "secondary",
			size = "sm",
			on_click = p.on_connect,
		}),
		when({ "waiting" }, Button {
			locale.t("common.cancel"),
			tone = "ghost",
			size = "sm",
			on_click = p.on_cancel,
		}),
		when({ "connected" }, Button {
			locale.t("ds.github_block.disconnect"),
			tone = "ghost",
			size = "sm",
			on_click = p.on_disconnect,
		}),
	}
end

-- The code to enter on GitHub: a click copies it (its glyph turns to a
-- check for a moment); Open GitHub copies it too.
local function code(p: Props): Node
	local copied = p.copied
	local function copy()
		copied:set(true)
		ui.after(1500, function()
			copied:set(false)
		end)
		if p.on_copy then
			p.on_copy()
		end
	end
	return ui.row {
		role = "button",
		label = function()
			return locale.s("ds.github_block.copy_code", { code = (control.value(p.code) :: string?) or "" })
		end,
		focusable = true,
		cursor = "pointer",
		h = control.H.sm,
		shrink = 0,
		px = "$space.3",
		gap = "$space.2",
		items = "center",
		rounded = control.RADIUS.sm,
		bg = "$zeron.secondary.bg",
		hover = { bg = "$pressed" },
		focus = control.ring,
		on_click = copy,
		ui.text { function()
			return (control.value(p.code) :: string?) or ""
		end, font = "$font.mono", text_size = "$font.size.md", line_height = "$font.line.md", weight = "$font.weight.medium", color = "$text.primary" },
		ui.div {
			color = function()
				return if copied:get() then "$text.primary" else "$text.tertiary"
			end,
			Icon {
				name = function()
					return if copied:get() then "check" else "copy"
				end,
				size = 14,
			},
		},
	}
end

-- The second row: the code while GitHub waits on it, the repositories once
-- it's connected. Its text lines up with GitHub's, past the mark.
local function second(p: Props): Node
	local spacer = ui.div { w = MARK, h = 1, shrink = 0 }
	-- Swapped as the phase changes, and only then: the code, the account
	-- and the repositories change in place, read as they stand.
	local function repos(): { string }
		return (control.value(p.repos) :: { string }?) or {}
	end
	return ui.presence(function(): string
		local s = p.state()
		return if s == "waiting" or s == "connected" then s else "none"
	end, function(k: string): Node
		local row: Node
		if k == "waiting" then
			-- What to do, then the code and the way to GitHub under it: two
			-- controls are more than a row's end holds beside its wor
			row = ui.row {
				w_full = true,
				px = "$space.4",
				py = "$space.3",
				gap = "$space.3",
				items = "start",
				spacer,
				ui.column {
					grow = 1,
					min_w = 0,
					gap = "$space.3",
					ui.column {
						gap = "$zeron.space.hair",
						ui.text { locale.t("ds.github_block.enter_code"), text_size = "$font.size.md", line_height = "$font.line.md", weight = "$font.weight.medium", color = "$text.primary" },
						ui.text { locale.t("ds.github_block.approve"), text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.tertiary" },
					},
					ui.row {
						gap = "$space.2",
						items = "center",
						code(p),
						Button {
							locale.t("ds.github_block.open_github"),
							tone = "secondary",
							size = "sm",
							on_click = function()
								p.copied:set(true)
								ui.after(1500, function()
									p.copied:set(false)
								end)
								if p.on_open then
									p.on_open()
								end
							end,
							Icon { name = "arrow-up-right", size = 14 },
						},
					},
				},
			}
		elseif k == "connected" then
			row = Settings.Row {
				label = function()
					return locale.s("ds.github_block.repositories", { n = #repos() })
				end,
				-- (Fading where it runs long, so Choose on GitHub keeps its place
				-- at the end.)
				description = function()
					local list = repos()
					if #list == 0 then
						return locale.s("ds.github_block.no_repositories")
					end
					return if #list > 1 then locale.s("ds.github_block.and_more", { first = list[1], n = tostring(#list - 1) }) else list[1]
				end,
				truncate = true,
				lead = spacer,
				control = Button { locale.t("ds.github_block.choose"), tone = "ghost", size = "sm", on_click = p.on_choose, Icon { name = "arrow-up-right", size = 14 } },
			}
		else
			return ui.div {}
		end
		-- The hairline over it is the row's own, so a block of one row has
		-- none of its own at the bottom, over the card's edge (a doubled
		-- line).
		return ui.column {
			w_full = true,
			ui.div { h = 1, w_full = true, shrink = 0, bg = "$zeron.settings.rule", aria_hidden = true },
			row,
		}
	end, { w_full = true, enter = { opacity = { 0, 1 }, duration = 140 }, exit = { opacity = 0, duration = 80 }, mode = "wait" })
end

return function(p: Props): Node
	return ui.column {
		w_full = true,
		h = if p.hug then nil else BLOCK,
		shrink = 0,
		-- One child, so the group draws no hairline of its own: the second
		-- row brings its own, and only when it's there.
		Settings.Group { ui.column {
			w_full = true,
			-- GitHub's row, a settings row's shape (its insets, the mark as
			-- its lead, the action at its end) with the status as its second
			-- line: a ring or the danger's ink, which a row's plain
			-- description can't carry.
			ui.row {
				w_full = true,
				min_h = "$zeron.settings.row",
				px = "$space.4",
				py = "$space.3",
				gap = "$space.4",
				items = "center",
				ui.row {
					grow = 1,
					min_w = 0,
					gap = "$space.3",
					items = "center",
					mark(),
					ui.column {
						grow = 1,
						min_w = 0,
						gap = "$zeron.space.hair",
						ui.text { "GitHub", text_size = "$font.size.md", line_height = "$font.line.md", weight = "$font.weight.medium", color = "$text.primary" },
						status(p),
					},
				},
				action(p),
			},
			second(p),
		} },
	}
end
