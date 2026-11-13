-- The shell's sidebar (#shell-sidebar) with an orchestrator's crew threads,
-- as the host lists them: under the orchestrator's chat (`children`), not
-- among `chats`, for tests/sidebar_crew.test.luau and its screenshots.
--
-- Project "zeron" on desk holds a chat of the person's own, the
-- orchestrator's own chat (listed in Orchestrators, never as a session) and
-- three crew threads: two under way, one started in another checkout (its
-- cwd elsewhere, its project the same). The plain chat has a side chat of
-- its own, which the list doesn't show. An archived crew thread is in no
-- list (the host hides it, as an archived side chat). Project "exnomic" on
-- the macbook has a crew thread of a second orchestrator.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local sessions = require("@account/sessions")
local orchestration = require("@account/orchestration")
local route = require("@app/screens/shell/route")
route.launched:set(true)
local Sidebar = require("@app/screens/shell/sidebar")

local NOW = os.time()
local function ago(s: number): number
	return NOW - s
end

local function chat(c: { [string]: any }): any
	c.device = c.device or "dev-desk"
	c.created = c.created or ago(7200)
	c.updated = c.updated or ago(600)
	c.harness = c.harness or "claude-code"
	c.agent = c.agent or "claude"
	return c
end

sessions.now:set({
	state = "ready",
	here = "dev-desk",
	devices = {
		{ id = "dev-desk", name = "desk", platform = "linux", here = true, online = true },
		{ id = "dev-mac", name = "macbook", platform = "macos", here = false, online = true },
	},
	projects = {
		{ id = "p-zeron", name = "zeron", device = "dev-desk", path = "/home/u/Documents/zeron", git = true, created = ago(86400 * 30) },
		{ id = "p-exnomic", name = "exnomic", device = "dev-mac", path = "/Users/u/exnomic", git = true, created = ago(86400 * 20) },
	},
	chats = {
		chat({ id = "orc-fix", title = "Zeron fixes", project = "p-zeron", status = "working", updated = ago(30) }),
		chat({ id = "orc-vpn", title = "Exnomic VPN", project = "p-exnomic", device = "dev-mac", updated = ago(3600) }),
		chat({ id = "plain", title = "Linux titlebar toggle", project = "p-zeron", updated = ago(900), side_chats = 1 }),
	},
	archived = {},
	children = {
		["orc-fix"] = {
			chat({ id = "crew-qa", parent = "orc-fix", child_kind = "crew", title = "Control Room Q&A section", name = "Control Room Q&A section", project = "p-zeron", branch = "main", status = "working", updated = ago(20) }),
			chat({ id = "crew-cards", parent = "orc-fix", child_kind = "crew", title = "Board card declutter", name = "Board card declutter", project = "p-zeron", branch = "zeron/board-card-declutter", status = "done", updated = ago(1500) }),
			-- (Started in a worktree: its cwd elsewhere, its project zeron.)
			-- (In its first turn: no title of its own yet; the host says what
			-- it last said. Named by its role in the crew.)
			chat({ id = "crew-new", parent = "orc-fix", child_kind = "crew", title = "[Message from Zeron chat Zeron fixes]", project = "p-zeron", status = "working", updated = ago(10) }),
			chat({ id = "crew-hover", parent = "orc-fix", child_kind = "crew", title = "Anchored action hover cards", name = "Anchored action hover cards", project = "p-zeron", cwd = "/home/u/Documents/dev/tmp/hover-cards/wt", status = "working", updated = ago(60) }),
		},
		["orc-vpn"] = {
			chat({ id = "crew-relay", parent = "orc-vpn", child_kind = "crew", title = "Relay health check", name = "Relay health check", project = "p-exnomic", device = "dev-mac", updated = ago(5400) }),
		},
		["plain"] = {
			chat({ id = "side-1", parent = "plain", child_kind = "side", title = "Side question about tabs", name = "Side question about tabs", project = "p-zeron", updated = ago(800) }),
		},
	},
	places = { pinned = {}, sections = {} },
	places_ready = true,
} :: any)

orchestration.now:set({
	orchestrators = {
		{ id = "orc-fix", kind = "standard", name = "Zeron fixes", goal = "Fix what the person finds.", hue = 28, crew = { devices = { "dev-desk" } }, paused = false, archived = false, created = ago(86400), updated = ago(30) },
		{ id = "orc-vpn", kind = "standard", name = "Exnomic VPN", goal = "Phase 2 migration.", hue = 196, crew = { devices = { "dev-mac" } }, paused = false, archived = false, created = ago(86400 * 2), updated = ago(3600) },
	},
	crew = {
		{ chat_id = "crew-qa", orchestrator_id = "orc-fix", released = false },
		{ chat_id = "crew-cards", orchestrator_id = "orc-fix", released = false },
		{ chat_id = "crew-hover", orchestrator_id = "orc-fix", released = false },
		{ chat_id = "crew-new", orchestrator_id = "orc-fix", role = "Sidebar crew threads", released = false },
		{ chat_id = "crew-old", orchestrator_id = "orc-fix", released = true },
		{ chat_id = "crew-relay", orchestrator_id = "orc-vpn", released = false },
	},
	items = {},
	asks = {},
} :: any)

-- "Board card declutter" archived: the host takes it from under its
-- orchestrator and lists it nowhere (apps/zgx/src/sessions.rs `place_of`).
ui.command {
	id = "archive_cards",
	title = "Archive a crew thread",
	run = function()
		local now = table.clone(sessions.now:peek() :: any)
		local children = table.clone(now.children)
		local kids = {}
		for _, c in children["orc-fix"] do
			if c.id ~= "crew-cards" then
				table.insert(kids, c)
			end
		end
		children["orc-fix"] = kids
		now.children = children
		sessions.now:set(now)
	end,
}

-- For its pictures: either scheme, and the Activity view off (the list by
-- project and device, as the person's).
ui.command { id = "theme_dark", title = "Dark theme", run = function() ui.theme.follow_system(false); ui.theme.current:set("zeron-dark") end }
ui.command { id = "theme_light", title = "Light theme", run = function() ui.theme.follow_system(false); ui.theme.current:set("zeron-light") end }
ui.command { id = "crew_off", title = "Orchestrator chats off", run = function() Sidebar.show_orchestrator_chats:set(false) end }
ui.command { id = "activity_off", title = "Activity view off", run = function() require("@app/screens/shell/activity").on:set(false) end }

-- Where the route stands, for the tests to read.
local where = ui.state("crew_route", "")
ui.effect(function()
	local c = route.current:get()
	where:set(`{c.name}:{c.orchestrator or c.session or ""}`)
end)

return ui.row {
	w_full = true,
	h_full = true,
	bg = "$background",
	ui.div { h_full = true, Sidebar.Sidebar { collapsed = ui.state("crew_sidebar_collapsed", false) } },
	ds.toast.Host {},
}
