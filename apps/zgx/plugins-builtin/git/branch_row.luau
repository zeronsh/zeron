--!strict
-- The panel home's branch row, Git's part of it: what the branch adds over
-- its base, `+N −M`, at the row's end (core's `slot:zeron.panel.branch`,
-- which provides `panel_branch`: { session, device, cwd, branch }). A
-- click opens the Changes tab at the Branch scope. Asked of the service on
-- the session's device (`branch_totals`) as the home shows the session, and
-- again as its folder, branch or work moves (a turn ending). Nothing to say
-- (no base, nothing added), or the device not answering: nothing shows,
-- and the row keeps its branch's name alone, as it does with Git off.

local PanelHome = require("@ds/panel_home")
local Tooltip = require("@ds/tooltip")
local control = require("@ds/control")

return function(zeron: any, Repo: any)
	-- Each session's totals as last heard, by session and folder (kept
	-- while the window is: the home shows them at once when it's back).
	local heard = ui.state("git.branch_totals", {} :: { [string]: any })

	return function(): Node
		local branch = zeron.live("panel_branch")
		local panel = zeron.context("panel")
		local first = branch()
		local session = first and first.session
		if type(session) ~= "string" then
			return ui.div {}
		end
		local function slot_of(b: any): string?
			return if b and b.cwd then `{b.session}|{b.cwd}` else nil
		end
		-- What to ask again on: the folder, the branch, the device and the
		-- session's status (a turn ending moves the work).
		local asked: string? = nil
		ui.effect(function()
			local b = branch()
			if type(b) ~= "table" or not b.cwd or (b.branch or "") == "" then
				return
			end
			local record = zeron.context_of("session", b.session)
			local key = `{b.session}|{b.cwd}|{b.branch}|{b.device or ""}|{record.status or ""}`
			if key == asked then
				return
			end
			asked = key
			local slot = slot_of(b) :: string
			zeron.service(b.device):call("branch_totals", { cwd = b.cwd }, { timeout = 60 }):after(function(t: any)
				if asked ~= key then
					return
				end
				local now = table.clone(heard:peek())
				now[slot] = if type(t) == "table" and t.adds then { adds = t.adds, dels = t.dels or 0 } else false
				heard:set(now)
			end, function(err: any)
				-- (Not known: the row keeps its branch alone.)
				print(`git: branch totals: {err}`)
			end)
		end)
		local function totals(): { adds: number, dels: number }?
			local slot = slot_of(branch())
			local t = if slot then heard:get()[slot] else nil
			if type(t) ~= "table" or t.adds + t.dels == 0 then
				return nil
			end
			return t
		end
		return ui.show(function(): boolean
			return totals() ~= nil
		end, ui.row {
			id = "git-branch-totals",
			role = "button",
			label = function(): string
				local t = totals() or { adds = 0, dels = 0 }
				return zeron.s("home.diff_label", { adds = t.adds, dels = t.dels })
			end,
			tooltip = Tooltip { zeron.t("home.branch_changes") },
			focusable = true,
			cursor = "pointer",
			shrink = 0,
			h = 24,
			px = "$space.1",
			gap = "$space.1",
			items = "center",
			-- (6 in from the row's edge, top and bottom.)
			rounded = PanelHome.inner(6),
			hover = { bg = "$hover" },
			focus = control.ring,
			on_click = function()
				if panel then
					Repo.open_tab(panel, session, "changes", "branch")
				end
			end,
			on_key = function(e: any): boolean?
				if (e.key == "Enter" or e.key == " ") and panel then
					Repo.open_tab(panel, session, "changes", "branch")
					return true
				end
				return nil
			end,
			ui.text {
				function(): string
					local t = totals()
					return if t then `+{t.adds}` else ""
				end,
				literal = true,
				font = "$font.mono",
				tabular = true,
				text_size = "$font.size.xs",
				color = "$zeron.diff.added",
			},
			ui.text {
				function(): string
					local t = totals()
					return if t then `−{t.dels}` else ""
				end,
				literal = true,
				font = "$font.mono",
				tabular = true,
				text_size = "$font.size.xs",
				color = "$zeron.diff.removed",
			},
		})
	end
end
