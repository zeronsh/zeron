-- Invite your teammates keeps who was sent one as more are sent: their
-- rows stay the same nodes, the new one joining under them, and the field
-- keeps the focus. (The list was built again for every send, keyed by how
-- many it held.) On the screen in ui/invite_fixture.luau.

-- A node's identity, by its kind and label (or text): `@id`.
local function node(app: any, kind: string, label: string): string?
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(` {kind} "{label}"`, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

local function settle(app: any)
	app:frames(30)
end

test("those sent one stay as more are sent", function()
	local app = mount("invite_fixture.luau")
	app:resize(680, 500)
	settle(app)
	app:set_state("invited", { "ada@example.com" })
	settle(app)
	local ada = node(app, "text", "ada@example.com")
	expect(ada).never.to_be_nil()

	app:set_state("invited", { "ada@example.com", "grace@example.com" })
	settle(app)
	expect(`ada @{node(app, "text", "ada@example.com")}`).to_be(`ada @{ada}`)
	expect(node(app, "text", "grace@example.com")).never.to_be_nil()
	expect(app:tree():find('textbox "Email addresses" ="" [^\n]* focused') ~= nil).to_be(true)
end)
