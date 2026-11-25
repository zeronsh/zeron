-- What floats over the Control room (Capture's history, Needs you) floats:
-- opened or shut, the Map, Board, Notes and Chat keep their frames to the
-- point, on every tab, at the window sizes the page is used at. On
-- ui/control_page_fixture.luau (ui/control_world, whose sample has asks
-- waiting and dumps sent).

local SIZES = { { 2024, 1102 }, { 1637, 1046 }, { 1440, 900 }, { 1100, 800 } }

-- Each tab's frames: the tab itself and what it lays out.
local FRAMES = {
	map = { "#control-body", "#control-tab-map", "#control-map-tab", "#control-map-frame", "#control-map" },
	board = { "#control-body", "#control-tab-board", "#control-board-scroll", "#control-board-tab" },
	notes = { "#control-body", "#control-tab-notes", "#control-notes-scroll", "#control-notes" },
	chat = { "#control-body", "#control-tab-chat", "#control-chat" },
}

local function open(w: number, h: number): any
	local app = mount("control_page_fixture.luau")
	app:resize(w, h)
	app:frames(30)
	return app
end

-- A node's box, as the tree gives it ("x,y wxh"), or nil.
local function box(app: any, target: string): string?
	if not app:has(target) then
		return nil
	end
	local line = string.match(app:tree(target, 0), "[^\n]+") or ""
	return string.match(line, " (%-?[%d%.]+,%-?[%d%.]+ [%d%.]+x[%d%.]+)")
end

local function frames(app: any, tab: string): { [string]: string }
	local out: { [string]: string } = {}
	for _, id in FRAMES[tab] do
		out[id] = box(app, id) or "missing"
	end
	return out
end

-- History open or shut, Needs you open or minimised to its pill.
local function set(app: any, history: boolean, needs: boolean)
	app:set_state("control_history_panel", history)
	app:set_state("control_needs_min", not needs)
	-- (Past the bar's and the Map's settling, and every fade.)
	app:wait("#no-such-node", { timeout_ms = 400 })
	app:frames(20)
end

for _, size in SIZES do
	local w, h = size[1], size[2]
	test(`float frames: History and Needs you open or shut never move a tab, at {w}x{h}`, function()
		local app = open(w, h)
		for _, tab in { "map", "board", "notes", "chat" } do
			app:set_state("control_tab", tab)
			set(app, false, false)
			local shut = frames(app, tab)
			for id, b in shut do
				expect(`{tab} {id} {b}`).never.to_be(`{tab} {id} missing`)
			end
			for _, case in { { true, false, "history" }, { false, true, "needs" }, { true, true, "both" } } do
				set(app, case[1], case[2])
				-- (History shows over every tab; Needs you on all but the Board.)
				if case[1] then
					expect(`{tab} {case[3]}: history {app:has("#control-history")}`).to_be(`{tab} {case[3]}: history true`)
				end
				if case[2] and tab ~= "board" then
					expect(`{tab} {case[3]}: needs {app:has("#control-needs-float")}`).to_be(`{tab} {case[3]}: needs true`)
				end
				local now = frames(app, tab)
				for id, b in shut do
					expect(`{tab} {case[3]} {id} {now[id]}`).to_be(`{tab} {case[3]} {id} {b}`)
				end
			end
			set(app, false, false)
			for id, b in frames(app, tab) do
				expect(`{tab} shut again {id} {b}`).to_be(`{tab} shut again {id} {shut[id]}`)
			end
		end
		expect(app:error()).to_be_nil()
	end)
end
