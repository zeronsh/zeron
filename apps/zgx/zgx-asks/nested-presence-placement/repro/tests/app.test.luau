-- Each slot replaced by the plugin. On zgx f6506b6e6 the two that are a
-- presence branch's root stay the app's, with no error (`zgx inspect
-- contributions` lists them, matching nothing); the rest are replaced.
local function swapped(app: any): { [string]: boolean }
	local out = {}
	for _, id in { "slot-bare", "slot-root", "slot-child", "slot-nested-root", "slot-nested-child" } do
		out[id] = app:has(`#swapped-{id}`)
	end
	return out
end

test("a plugin replaces a slot that is a presence branch's root", function()
	local app = mount("app.luau")
	app:frames(10)
	local now = swapped(app)
	-- Replaced today: bare, and a child inside a branch (nested or not).
	expect(now["slot-bare"]).to_be(true)
	expect(now["slot-child"]).to_be(true)
	expect(now["slot-nested-child"]).to_be(true)
	-- The ask: a branch's root too.
	expect(now["slot-root"]).to_be(true)
	expect(now["slot-nested-root"]).to_be(true)
end)
