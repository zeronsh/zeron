-- Your agents updates in place: flipping an agent's switch, a sign-in
-- starting and being cancelled, an account's usage moving, and an account
-- arriving and going leave every other card, its switch and its buttons the
-- same nodes, and the page's scroll where it was. (Built again, a card is a
-- new node: the button just pressed goes, and the list leaves the flow and
-- comes back, which scrolled the page.) The step runs on agents made up in
-- ui/providers_fixture.luau, whose stubs answer as the engine would.

type Seen = { id: string, y: number }

-- A node of the tree, by its kind and its label or text: its identity and
-- where it shows (`@id kind "label" x,y wxh`).
local function seen(tree: string, kind: string, label: string): Seen?
	for line in tree:gmatch("[^\n]+") do
		if line:find(` {kind} "{label}"`, 1, true) then
			local id = line:match("@(%d+)")
			local y = line:match(" %-?%d+,(%-?%d+) %d+x%d+")
			if id and y then
				return { id = id, y = tonumber(y) :: number }
			end
		end
	end
	return nil
end

-- What must stay put, read from the tree as it stands.
local WATCHED = {
	{ "text", "Codex" },
	{ "switch", "Codex on" },
	{ "button", "Actions for ada@example.com" },
	{ "button", "Add account" },
	{ "text", "Cursor" },
	{ "switch", "Cursor on" },
	{ "text", "Grok" },
	{ "switch", "Grok on" },
	{ "switch", "Devin on" },
	{ "button", "Connect a Devin account" },
}

local function watch(app: any): { [string]: Seen }
	local tree = app:tree()
	local out: { [string]: Seen } = {}
	for _, w in WATCHED do
		local s = seen(tree, w[1], w[2])
		if s then
			out[`{w[1]} {w[2]}`] = s
		end
	end
	return out
end

-- The Codex card, above every change but its own, hasn't moved (the page
-- didn't scroll), and every node seen before is the same node now, but the
-- foot a sign-in swaps (`swapped`).
local function unmoved(before: { [string]: Seen }, after: { [string]: Seen }, step: string, swapped: { string }?)
	local b, a = before["text Codex"], after["text Codex"]
	expect(`{step}: Codex at {a and a.y}`).to_be(`{step}: Codex at {b and b.y}`)
	for name, was in before do
		local now = if table.find(swapped or {}, name) then was else after[name]
		expect(`{step}: {name} is {if now then "@" .. now.id else "gone"}`).to_be(`{step}: {name} is @{was.id}`)
	end
end

local function settle(app: any)
	app:frames(30)
end

test("Your agents changes in place, the page holding its scroll", function()
	local app = mount("providers_fixture.luau")
	app:resize(680, 500)
	settle(app)
	app:scroll("#scroll_step", 240)
	settle(app)
	local first = watch(app)
	expect(first["switch Codex on"]).never.to_be_nil()
	expect(first["switch Devin on"]).never.to_be_nil()

	-- An agent turned off, then on.
	app:click('role:switch "Codex on"')
	settle(app)
	expect(app:state("calls")).to_contain("set_enabled:codex:false")
	unmoved(first, watch(app), "switch off")
	app:click('role:switch "Codex on"')
	settle(app)
	unmoved(first, watch(app), "switch on")

	-- A sign-in started on Codex: its foot alone changes, the focus going
	-- on to the foot's Cancel (zgx moves focus on to what replaces a
	-- focused node; a scroll view reveals it, here already in view). Then
	-- cancelled.
	app:click('#scroll_step >> "Add account"')
	settle(app)
	expect(app:has('"Signing in to Codex"')).to_be(true)
	expect(app:tree():find('button "Cancel" [^\n]*focused') ~= nil).to_be(true)
	local signing = watch(app)
	unmoved(first, signing, "sign-in started", { "button Add account" })
	app:click('#scroll_step >> "Cancel"')
	settle(app)
	expect(app:state("calls")).to_contain("cancel:codex")
	expect(app:has('#scroll_step >> "Add account"')).to_be(true)
	unmoved(first, watch(app), "sign-in cancelled", { "button Add account" })
	-- (The foot's Add account is a new node after the swap and back; from
	-- here on it, too, must stay the node it is.)
	first["button Add account"] = watch(app)["button Add account"]

	-- Codex's usage and version move.
	app:command("usage")
	settle(app)
	expect(app:has('"90%"')).to_be(true)
	expect(app:has('"v1.0.1"')).to_be(true)
	unmoved(first, watch(app), "usage moved")

	-- An account arrives on Grok, then goes: Grok's foot says so in place.
	app:command("add_account")
	settle(app)
	expect(app:has('"bo@example.com"')).to_be(true)
	local with = watch(app)
	unmoved(first, with, "account added")
	expect(with["switch Grok on"] and with["switch Grok on"].id).to_be(first["switch Grok on"].id)
	app:command("remove_account")
	settle(app)
	expect(app:has('"bo@example.com"')).to_be(false)
	unmoved(first, watch(app), "account removed")
end)

test("Connect GitHub changes in place as its account and repositories do", function()
	local app = mount("github_fixture.luau")
	app:resize(680, 500)
	settle(app)
	local tree = app:tree()
	local disconnect = seen(tree, "button", "Disconnect")
	local choose = seen(tree, "button", "Choose on GitHub")
	local count = seen(tree, "text", "2 repositories")
	expect(disconnect).never.to_be_nil()
	expect(choose).never.to_be_nil()
	expect(count).never.to_be_nil()

	app:command("more_repos")
	settle(app)
	tree = app:tree()
	local three = seen(tree, "text", "3 repositories")
	expect(three and three.id).to_be(count and count.id)
	expect(app:has('"example/one and 2 more"')).to_be(true)
	local still = seen(tree, "button", "Choose on GitHub")
	expect(still and still.id).to_be(choose and choose.id)

	app:command("renamed")
	settle(app)
	tree = app:tree()
	expect(app:has('"octo-renamed"')).to_be(true)
	local after = seen(tree, "button", "Disconnect")
	expect(after and after.id).to_be(disconnect and disconnect.id)
	local kept = seen(tree, "text", "3 repositories")
	expect(kept and kept.id).to_be(count and count.id)
end)
