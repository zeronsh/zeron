-- The top bar's update button (screens/shell/updates): only with something to
-- say, three marks at most, and its list by device with what each row can do.

local function row(device: string, name: string, harness: string, action: string?, connected: boolean?): any
	return {
		id = `{device}:{harness}`,
		device = device,
		device_name = device,
		here = device == "here",
		connected = connected ~= false,
		kind = "agent",
		harness = harness,
		name = name,
		mark = "claude",
		phase = "available",
		installed = "1.0",
		latest = "1.1",
		action = action,
		active = false,
	}
end

local function open(rows: { any }): any
	local app = mount("updates_fixture.luau")
	app:resize(900, 700)
	app:set_state("updates", { rows = rows })
	app:frames(10)
	return app
end

test("nothing to update shows nothing", function()
	local app = open({})
	expect(app:has("#updates-button")).to_be(false)
end)

test("the button wears three marks at most and says how many", function()
	local app = open({
		row("here", "Claude Code", "claude-code", "update"),
		row("here", "Codex", "codex", "update"),
		row("metal", "Claude Code", "claude-code", "update"),
		row("metal", "Grok", "grok", "update"),
		row("metal", "Pi", "pi", "update"),
	})
	expect(app:has('role:button "5 updates"')).to_be(true)
	expect(app:has("#updates-mark-3")).to_be(true)
	expect(app:has("#updates-mark-4")).to_be(false)
end)

-- A node's box, from the tree's first line: `@n … x,y WxH`.
local function box(app: any, target: string): { x: number, w: number }
	local x, w = app:tree(target):match("^%s*@%d+[^\n]- ([%-%d.]+),[%-%d.]+ ([%d.]+)x[%d.]+")
	return { x = tonumber(x) :: number, w = tonumber(w) :: number }
end

test("each mark sits in the middle of its own circle", function()
	local app = open({
		row("here", "Claude Code", "claude-code", "update"),
		row("here", "Codex", "codex", "update"),
		row("metal", "Grok", "grok", "update"),
	})
	for at = 1, 3 do
		local circle, glyph = box(app, `#updates-mark-{at}`), box(app, `#updates-mark-{at}-glyph`)
		expect(math.abs(glyph.x + glyph.w / 2 - (circle.x + circle.w / 2)) <= 0.5).to_be(true)
	end
end)

test("the same thing on two devices is one mark", function()
	local app = open({
		row("here", "Claude Code", "claude-code", "update"),
		row("metal", "Claude Code", "claude-code", "update"),
	})
	expect(app:has("#updates-mark-1")).to_be(true)
	expect(app:has("#updates-mark-2")).to_be(false)
end)

test("the list goes by device, and offers what each row can do", function()
	local app = open({
		row("here", "Claude Code", "claude-code", "update"),
		row("metal", "Codex", "codex", "steps"),
		row("metal", "Grok", "grok", "update", false),
	})
	app:click("#updates-button")
	app:frames(10)
	expect(app:has('"metal"')).to_be(true)
	expect(app:has('role:button "Update · Claude Code"')).to_be(true)
	expect(app:has('role:button "View steps · Codex"')).to_be(true)
	-- A device out of reach says so, and its action waits.
	expect(app:has('"Offline"')).to_be(true)
end)

test("another device's Zeron says when it updates there, and what stopping costs", function()
	local app = open({
		{
			id = "mac:zeron",
			device = "mac",
			device_name = "mac",
			here = false,
			connected = true,
			kind = "zeron",
			phase = "available",
			installed = "1.0",
			latest = "1.1",
			active = false,
		},
		{
			id = "metal:zeron",
			device = "metal",
			device_name = "metal",
			here = false,
			connected = true,
			kind = "zeron",
			phase = "available",
			installed = "1.0",
			latest = "1.1",
			action = "update",
			active = false,
		},
	})
	app:click("#updates-button")
	app:frames(10)
	expect(app:has('"1.0 → 1.1 · update from that device"')).to_be(true)
	expect(app:has('role:button "Update · Zeron"')).to_be(true)
end)

test("Update all says what it will do first, and what it can't reach", function()
	local app = open({
		row("here", "Claude Code", "claude-code", "update"),
		row("metal", "Grok", "grok", "update"),
		row("metal", "Codex", "codex", "steps"),
		row("away", "Pi", "pi", "update", false),
	})
	app:click("#updates-button")
	app:frames(10)
	app:click("#updates-all")
	app:frames(20)
	-- A dialog: what goes, and what can't, and why.
	expect(app:has("#updates-review")).to_be(true)
	expect(app:has('"Update all?"')).to_be(true)
	expect(app:has('"2 updates across 2 devices"')).to_be(true)
	expect(app:has('"Not included"')).to_be(true)
	expect(app:has('"away · Offline"')).to_be(true)
	expect(app:has('"Agents wait for what\'s running to finish"')).to_be(true)
	-- Nothing happens until it's said yes to.
	app:click('role:button "Cancel"')
	app:wait('"never shown, only waited past"', { timeout_ms = 200 })
	app:frames(10)
	-- (A closed dialog's content is kept, hidden.)
	expect(not app:has("#updates-review") or string.find(app:tree("#updates-review"), " hidden", 1, true) ~= nil).to_be(true)
end)

test("a device that says it can't update from here says why, and offers what to run", function()
	local app = open({
		{
			id = "box:zeron",
			device = "box",
			device_name = "box",
			here = false,
			connected = true,
			kind = "zeron",
			phase = "available",
			installed = "1.0",
			latest = "1.1",
			why_not = "It was started by hand",
			manual_command = "zeron update",
			action = "copy",
			active = false,
		},
	})
	app:click("#updates-button")
	app:frames(10)
	expect(app:has('"It was started by hand"')).to_be(true)
	expect(app:has('role:button "Copy command · Zeron"')).to_be(true)
end)

test("an update waiting says why in the engine's words, or its own", function()
	local waiting = row("metal", "Codex", "codex", "cancel")
	waiting.phase = "waiting"
	waiting.active = true
	waiting.message = "Waiting for running chats to finish."
	local plain = row("metal", "Claude Code", "claude-code", "cancel")
	plain.phase = "waiting"
	plain.active = true
	local app = open({ waiting, plain })
	app:click("role:button ~\"update\"")
	app:frames(20)
	expect(app:has('"Waiting for running chats to finish."')).to_be(true)
	expect(app:has('"Waiting for the current run"')).to_be(true)
end)

test("a Mac another device updated offers to reopen on the new version", function()
	local app = mount("updates_fixture.luau")
	app:resize(900, 700)
	app:set_state("about", { app_version = "0.2.0", moved_on_to = "0.3.0", flow = "idle" })
	app:frames(10)
	app:click("#updates-button")
	app:frames(20)
	expect(app:has('role:button "Restart · Zeron"')).to_be(true)
end)

test("the list scrolls under the head, which stays and takes its own presses", function()
	local many = {}
	for i = 1, 12 do
		table.insert(many, row("here", `Agent {i}`, `agent-{i}`, "update"))
	end
	local app = open(many)
	app:click("#updates-button")
	app:frames(20)
	local function y_of(sel: string): number
		return tonumber(string.match(string.match(app:tree(sel), "[^\n]+") or "", " %-?%d+,(%-?%d+) ")) or 0
	end
	local head = y_of("#updates-all")
	local first = y_of('role:button "Update · Agent 1"')
	expect(first > head).to_be(true)
	app:scroll('role:button "Update · Agent 3"', 200)
	app:frames(20)
	expect(y_of("#updates-all")).to_be(head)
	expect(y_of('role:button "Update · Agent 1"') < first).to_be(true)
	-- Nothing's pressed through the head.
	expect(app:has('role:button "Update · Agent 12"')).to_be(true)
end)

local function some(n: number): { any }
	local out = {}
	for i = 1, n do
		table.insert(out, row("here", `Agent {i}`, `agent-{i}`, "update"))
	end
	return out
end

test("skipping a row hides it until a newer version, and Undo brings it back", function()
	local list = some(2)
	local app = open(list)
	app:click("#updates-button")
	app:frames(10)
	app:click("#update-skip-here:agent-1")
	app:frames(10)
	expect(app:has("#update-row-here:agent-1")).to_be(false)
	expect(app:has("#update-row-here:agent-2")).to_be(true)
	expect(app:tree("#updates-button")).to_contain("1 update")
	-- Undo.
	app:click('role:button "Undo"')
	app:frames(10)
	expect(app:tree("#updates-button")).to_contain("2 updates")
	-- (Pressing Undo was a press outside the panel.)
	if not app:has("#update-row-here:agent-1") or string.find(app:tree("#update-row-here:agent-1"), " hidden", 1, true) then
		app:click("#updates-button")
		app:frames(10)
	end
	app:click("#update-skip-here:agent-1")
	app:frames(10)
	-- The same version again stays skipped; a newer one shows.
	local newer = some(2)
	app:set_state("updates", { rows = newer })
	app:frames(10)
	expect(app:tree("#updates-button")).to_contain("1 update")
	newer[1].latest = "1.2"
	app:set_state("updates", { rows = newer })
	app:frames(10)
	expect(app:tree("#updates-button")).to_contain("2 updates")
end)

test("skipping them all from the dialog puts the button away", function()
	local app = open(some(3))
	app:click("#updates-button")
	app:frames(10)
	app:click("#updates-all")
	app:frames(20)
	app:click('role:button "Skip all"')
	app:wait('"never shown, only waited past"', { timeout_ms = 300 })
	app:frames(10)
	expect(app:has("#updates-button")).to_be(false)
	expect(app:has("#updates-review")).to_be(false)
end)

test("what's under way can't be skipped", function()
	local going = row("here", "Codex", "codex", "cancel")
	going.phase = "downloading"
	going.active = true
	local app = open({ going })
	app:click("#updates-button")
	app:frames(10)
	expect(app:has("#update-skip-here:codex")).to_be(false)
end)

test("a long line fades at its end, beside Skip and its action", function()
	local long = row("here", "Muse Code", "muse", "retry")
	long.phase = "failed"
	long.message = "verification returned something much longer than this row has any room for"
	local app = open({ long })
	app:click("#updates-button")
	app:frames(10)
	expect(app:has("#update-skip-here:muse")).to_be(true)
	expect(app:has("#update-action-here:muse")).to_be(true)
	-- At its own width, past the row's room: faded, not cut to an ellipsis.
	local width = string.match(app:tree("#update-row-here:muse"), 'text "verification.-" %-?%d+,%-?%d+ (%d+)x')
	expect((tonumber(width) or 0) > 340).to_be(true)
end)

test("Skip sits before a row's action, and actions keep the row's end", function()
	local long = row("here", "Muse Code", "muse", "retry")
	long.phase = "failed"
	long.message = "it failed"
	local going = row("here", "Grok", "grok", "cancel")
	going.phase = "downloading"
	going.active = true
	local app = open({ row("here", "Codex", "codex", "update"), long, going })
	app:click("#updates-button")
	app:frames(10)
	local function span(sel: string): (number, number)
		local x, w = string.match(string.match(app:tree(sel), "[^\n]+") or "", " (%-?%d+),%-?%d+ (%d+)x")
		return tonumber(x) or 0, (tonumber(x) or 0) + (tonumber(w) or 0)
	end
	local _, skip_end = span("#update-skip-here:codex")
	local action_start, action_end = span("#update-action-here:codex")
	expect(skip_end <= action_start).to_be(true)
	local _, muse_end = span("#update-action-here:muse")
	local _, grok_end = span("#update-action-here:grok")
	expect(muse_end).to_be(action_end)
	expect(grok_end).to_be(action_end)
	-- Nothing to skip mid-update.
	expect(app:has("#update-skip-here:grok")).to_be(false)
end)

test("no title: Update all alone over the list, and with nothing to run together, no room for it", function()
	local app = open({ row("here", "Codex", "codex", "update"), row("here", "Grok", "grok", "update") })
	app:click("#updates-button")
	app:frames(10)
	-- (Its name's there for what reads it aloud, not as words on show.)
	expect(string.find(app:tree(), 'text "Updates"', 1, true)).to_be(nil)
	expect(app:has("#updates-all")).to_be(true)
	local function top(sel: string): number
		return tonumber(string.match(string.match(app:tree(sel), "[^\n]+") or "", " %-?%d+,(%-?%d+) ")) or 0
	end
	local with_head = top("#update-row-here:codex")
	app:set_state("updates", { rows = { row("here", "Codex", "codex", "update") } })
	app:frames(20)
	expect(app:has("#updates-all")).to_be(false)
	expect(top("#update-row-here:codex") < with_head).to_be(true)
end)

test("the panel is as tall as its rows, up to its most", function()
	local function list(n: number): { any }
		local out = {}
		for i = 1, n do
			table.insert(out, row("here", `Agent {i}`, `agent-{i}`, "update"))
		end
		return out
	end
	local app = open(list(1))
	app:click("#updates-button")
	app:frames(10)
	local function height(): number
		return tonumber(string.match(string.match(app:tree(".updates-panel"), "[^\n]+") or "", " %d+x(%d+)")) or 0
	end
	local one = height()
	expect(one < 90).to_be(true)
	app:set_state("updates", { rows = list(3) })
	app:frames(20)
	local three = height()
	expect(three > one).to_be(true)
	app:set_state("updates", { rows = list(20) })
	app:frames(20)
	local most = height()
	expect(most > three).to_be(true)
	expect(most).to_be(242)
	-- And back.
	app:set_state("updates", { rows = list(1) })
	app:frames(20)
	expect(height()).to_be(one)
end)

test("with nothing over them, the rows sit in the middle of the panel", function()
	local app = open({ row("here", "Codex", "codex", "update") })
	app:click("#updates-button")
	app:frames(10)
	local function box(sel: string): (number, number)
		local y, h = string.match(string.match(app:tree(sel), "[^\n]+") or "", " %-?%d+,(%-?%d+) %d+x(%d+)")
		return tonumber(y) or 0, tonumber(h) or 0
	end
	local panel_y, panel_h = box(".updates-panel")
	local row_y, row_h = box("#update-row-here:codex")
	expect(row_y - panel_y).to_be((panel_y + panel_h) - (row_y + row_h))
end)
