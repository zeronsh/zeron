-- The model picker's reasoning follows the model in place: moving between
-- models that both have levels keeps the slider (its groove, its level's
-- name) the same nodes, its levels the new model's. (It was built again for
-- each model, its field and thumb starting over.) And a model's options
-- (Devin Fusion's lead and sidekick) sit under it, above its reasoning, with
-- no card beside its row. On the picker in
-- ui/picker_fixture.luau.

-- A node's identity, by its kind and label (or text): `@id`.
local function node(app: any, kind: string?, label: string): string?
	local pattern = if kind then ` {kind} "{label}"` else ` "{label}"`
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(pattern, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

-- Where a node sits in the tree (its line), by what its line holds.
local function line_of(app: any, text: string): number?
	local n = 0
	for line in app:tree():gmatch("[^\n]+") do
		n += 1
		if line:find(text, 1, true) and not line:find(" hidden", 1, true) then
			return n
		end
	end
	return nil
end

local function settle(app: any)
	app:frames(30)
end

local function options(app: any): { [string]: string }
	return (app:state("test_picker_options") or {}) :: { [string]: string }
end

test("moving between models keeps the reasoning slider", function()
	local app = mount("picker_fixture.luau")
	app:resize(600, 700)
	settle(app)
	app:click('role:button "Model"')
	settle(app)
	local groove = node(app, nil, "REASONING")
	local title = node(app, "text", "REASONING")
	expect(groove).never.to_be_nil()

	-- Another model with the same levels, then one with five.
	app:set_state("test_picker_model", "opus")
	settle(app)
	expect(`groove @{node(app, nil, "REASONING")}`).to_be(`groove @{groove}`)
	expect(`title @{node(app, "text", "REASONING")}`).to_be(`title @{title}`)
	app:set_state("test_picker_model", "gpt")
	settle(app)
	expect(`groove @{node(app, nil, "REASONING")}`).to_be(`groove @{groove}`)
	expect(app:state("test_picker_effort")).to_be("medium")
	-- Its levels are the new model's: End reaches its fifth.
	app:click('"REASONING"[2]')
	app:key("End")
	settle(app)
	expect(app:state("test_picker_effort")).to_be("xhigh")
	expect(app:wait('"Extra high"', { timeout_ms = 2000 })).to_be(true)

	-- A model without levels has none; back to one with them, a slider.
	app:set_state("test_picker_model", "mini")
	settle(app)
	expect(node(app, nil, "REASONING")).to_be_nil()
	app:set_state("test_picker_model", "sonnet")
	settle(app)
	expect(node(app, nil, "REASONING")).never.to_be_nil()
end)

-- The chosen model's options sit under it, above its reasoning (Zeron's
-- tray, the lead first): each a row opening its choices beside the panel,
-- the choice made shown on it. A pick is kept by option id; the option's
-- own start is no pick.
test("a model's options sit under it, above its reasoning, and are picked beside it", function()
	local app = mount("picker_fixture.luau")
	app:resize(700, 700)
	app:set_state("test_picker_model", "fusion")
	settle(app)
	app:click('role:button "Model"')
	settle(app)
	local lead = line_of(app, 'button "Lead"')
	local sidekick = line_of(app, 'button "Sidekick"')
	local reasoning = line_of(app, '"REASONING"')
	expect(lead).never.to_be_nil()
	expect(sidekick).never.to_be_nil()
	expect(reasoning).never.to_be_nil()
	expect((lead :: number) < (sidekick :: number)).to_be(true)
	expect((sidekick :: number) < (reasoning :: number)).to_be(true)
	-- The lead's choices open beside the row, its start tagged; a pick.
	app:click('role:button "Lead"')
	settle(app)
	expect(app:has('role:option ~"Fable 5.1" >> ~"Default"')).to_be(true)
	app:click('role:option ~"GPT-6 Sol"')
	settle(app)
	expect(options(app).lead).to_be("sol")
	expect(app:has('role:button "Lead" >> ~"GPT-6 Sol"')).to_be(true)
	-- Back to its start: no pick.
	app:click('role:button "Lead"')
	settle(app)
	app:click('role:option ~"Fable 5.1"')
	settle(app)
	expect(options(app).lead).to_be(nil)
	-- A model without options has no rows.
	app:set_state("test_picker_model", "sonnet")
	settle(app)
	expect(line_of(app, 'button "Lead"')).to_be_nil()
	expect(node(app, nil, "REASONING")).never.to_be_nil()
	expect(app:error()).to_be(nil)
end)

-- Devin Fusion has no card beside its row: picked, its options are the
-- tray's rows under it, as any model's are.
test("Fusion is picked like any model, its options then under it", function()
	local app = mount("picker_fixture.luau")
	app:resize(700, 700)
	app:click('role:button "Model"')
	settle(app)
	-- Opened out (a press on the chosen model: the layer over its one
	-- row), on Devin's tab.
	app:click(".folded")
	settle(app)
	app:click('role:tab "Devin"')
	settle(app)
	-- Resting on Fusion opens nothing beside it.
	app:hover('role:option ~"Fusion"')
	app:wait("#a-moment-passes", { timeout_ms = 400 })
	settle(app)
	expect(line_of(app, '"Fusion options"')).to_be_nil()
	-- Picked, its Lead and Sidekick sit under it, above its reasoning.
	app:click('role:option ~"Fusion"')
	settle(app)
	expect(app:state("test_picker_model")).to_be("fusion")
	if not app:has('role:button "Lead"') then
		app:click('role:button "Model"')
		settle(app)
	end
	local lead = line_of(app, 'button "Lead"')
	local sidekick = line_of(app, 'button "Sidekick"')
	expect(lead).never.to_be_nil()
	expect(sidekick).never.to_be_nil()
	expect(app:error()).to_be(nil)
end)

-- Moving between tabs (opened out) keeps the panel as it is: the same
-- panel, the same size every frame, only its list changing. (The list's
-- window took each tab's height, easing to it: on Wayland a popup that
-- changes size is a new `xdg_popup`, and the compositor faded each one in
-- and the last out, so the menu flickered see-through, two of it
-- overlapping, on every switch.)
test("moving between tabs keeps the panel's size, its list changing in place", function()
	local app = mount("picker_fixture.luau")
	app:resize(600, 700)
	settle(app)
	app:click('role:button "Model"')
	settle(app)
	app:click(".folded")
	settle(app)
	-- The panel: its node and its box, as the tree has it.
	local function panel(): (string?, string?)
		for line in app:tree():gmatch("[^\n]+") do
			local id, size = line:match("@(%d+) dialog %S+ (%d+x%d+)")
			if id then
				return id, size
			end
		end
		return nil, nil
	end
	local id, size = panel()
	expect(id).never.to_be_nil()
	-- The reasoning (its title and its slider), once.
	local reasoning = app:count('"REASONING"')
	-- Claude's two models, Codex's two, the starred (none), Devin's two:
	-- frame by frame through each switch, the same panel at the same size.
	for _, tab in { "Codex", "Starred", "Devin", "Claude" } do
		app:click(`role:tab "{tab}"`)
		for _ = 1, 20 do
			app:frames(1)
			local now_id, now_size = panel()
			expect(`{tab}: @{now_id} {now_size}`).to_be(`{tab}: @{id} {size}`)
			expect(app:count('"REASONING"')).to_be(reasoning)
		end
	end
	-- Its list is the tab's: Claude's models again.
	expect(app:has('role:option ~"Sonnet"')).to_be(true)
	expect(app:has('role:option ~"GPT"')).to_be(false)
	-- A search that finds nothing keeps it too.
	app:type('role:textbox "Search"', "zz")
	for _ = 1, 20 do
		app:frames(1)
		local now_id, now_size = panel()
		expect(`search: @{now_id} {now_size}`).to_be(`search: @{id} {size}`)
	end
	expect(app:has('role:option ~"Sonnet"')).to_be(false)
	expect(app:error()).to_be(nil)
end)
