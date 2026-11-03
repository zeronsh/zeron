--!strict
-- A session's pull request in its panel home's details, under its branch
-- (core's `slot:zeron.panel.details`, which gives `panel_details`: { session,
-- device, cwd, branch }): the panel home's own row (ds/panel_home), led by
-- the pull request glyph in the quiet ink the device, project and branch
-- rows lead with, its title, then the pull request's badge, the sidebar's
-- (ds/badge `PullRequest`: its state's glyph and its number in its state's
-- colour), and how its checks stand, a badge too (the checks badge's look,
-- a short word), when GitHub says. Both badges' corners nest in the row's. A
-- click opens it in the chat's panel. No pull request: nothing, and no room
-- taken.

local PanelHome = require("@ds/panel_home")
local Badge = require("@ds/badge")
local control = require("@ds/control")

local STATES: { [string]: boolean } = { open = true, merged = true, closed = true, draft = true }
-- GitHub's word on its checks, as the design system's checks badge says it.
local CHECKS: { [string]: string } = { passed = "passing", failed = "failing", pending = "running" }
local CHECK_ICON: { [string]: string } = { passing = "check-circle", failing = "close-circle", running = "clock-circle" }
-- A badge's height (ds/badge `md`), and its corner nested in the row's.
local BADGE_H = 20
local NESTED = control.nest(PanelHome.ROW_RADIUS, (PanelHome.ROW_H - BADGE_H) / 2)

return function(zeron: any, watch: any)
	return function(): Node
		local details = zeron.context("panel_details")
		local session = details and details.session
		if type(session) ~= "string" then
			return ui.div {}
		end
		local panel = zeron.context("panel")
		local pr = watch.pr(session)
		local function state(): string
			local now = pr()
			local s = if now then now.state else nil
			return if s and STATES[s] then s else "open"
		end
		local function open()
			local now = pr()
			if now and panel then
				panel.open_tab {
					kind = "github-pr",
					title = if now.title and now.title ~= "" then now.title else `#{now.number}`,
					icon = "pull-request",
					data = { url = now.url, number = now.number, session = session },
				}
			end
		end
		return ui.show(function(): boolean
			return pr() ~= nil
		end, ui.column {
			w_full = true,
			-- (The rows' own gap over it: the details take no room without it.)
			pt = "$zeron.list.gap",
			PanelHome.Row {
				id = "github-panel-pr",
				-- (The row's kind, in the quiet ink the device, project and
				-- branch rows lead with; its state is the badge's to say.)
				icon = "pull-request",
				label = function(): string
					local now = pr()
					return if now then now.title or "" else ""
				end,
				name = function(): string
					local now = pr()
					return if now then zeron.s("panel_row.label", { number = now.number, title = now.title or "", state = zeron.s(`panel_row.state.{state()}`) }) else ""
				end,
				on_click = open,
				trail = ui.row {
					class = "github-panel-pr-badges",
					shrink = 0,
					gap = "$space.1",
					items = "center",
					theme = { radius = { md = NESTED } },
					ui.presence(function(): string
						local now = pr()
						return if now and now.ci and CHECKS[now.ci] then CHECKS[now.ci] else ""
					end, function(ci: string): Node
						if ci == "" then
							return ui.div {}
						end
						-- (The checks badge's look, its word the short one: the
						-- row's title needs the room.)
						return Badge.Badge {
							zeron.t(`panel_row.checks.{ci}`),
							icon = CHECK_ICON[ci] :: any,
							color = `$zeron.checks.{ci}`,
							size = "md",
							label = zeron.t(`panel_row.checks_said.{ci}`),
						}
					end),
					ui.presence(function(): string
						local now = pr()
						return `{state()}:{if now then now.number else ""}`
					end, function(): Node
						local now = pr()
						return Badge.PullRequest { state = state() :: any, number = if now then now.number else nil, size = "md" }
					end),
				},
			},
		})
	end
end
