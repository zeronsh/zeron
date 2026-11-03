-- ds.PrivateText: hidden, an address is a blurred stand-in that is never
-- the address (no text anywhere holds it), the same stand-in every time,
-- its separators kept; a click shows that one, another hides it; the
-- setting switches every address at once. On ui/private_fixture.luau, and
-- the settings pages on ui/shell_fixture.luau.

local EMAILS = { "mara@halcyon.dev", "theo.lindqvist@halcyon.dev", "ines_duarte+zeron@north-wind.io" }

local function shows(app: any, text: string): boolean
	return app:tree():find(text, 1, true) ~= nil
end

local function open(): any
	local app = mount("private_fixture.luau")
	app:resize(700, 500)
	app:frames(10)
	return app
end

test("a stand-in is the same every time, as long as the address, its separators in place", function()
	local first = open():state("private_stand_ins") :: { string }
	local again = open():state("private_stand_ins") :: { string }
	expect(first).to_equal(again)
	for i, e in EMAILS do
		local s = first[i]
		expect(#s).to_be(#e)
		expect(s).never.to_be(e)
		for at = 1, #e do
			local ch = string.sub(e, at, at)
			if string.find(ch, "[@%.%-_+]") then
				expect(string.sub(s, at, at)).to_be(ch)
			else
				expect(string.find("abcdefghjkmnpqrstuvwxyz23456789", string.sub(s, at, at), 1, true) ~= nil).to_be(true)
			end
		end
	end
	-- Different addresses, different stand-ins.
	expect(first[1]).never.to_be(first[2])
end)

test("hidden, no text anywhere holds an address; shown again, they're plain", function()
	local app = open()
	for _, e in EMAILS do
		expect(shows(app, e)).to_be(true)
	end
	app:command("private.hide")
	app:frames(10)
	for _, e in EMAILS do
		expect(shows(app, e)).to_be(false)
	end
	local stand_ins = app:state("private_stand_ins") :: { string }
	expect(shows(app, stand_ins[1])).to_be(true)
	app:command("private.show")
	app:frames(10)
	for _, e in EMAILS do
		expect(shows(app, e)).to_be(true)
	end
end)

test("a click shows that one address, another hides it, the rest staying hidden", function()
	local app = open()
	app:command("private.hide")
	app:frames(10)
	app:click('#private-line >> role:switch "Show email"')
	app:frames(10)
	expect(app:tree("#private-line"):find("mara@halcyon.dev", 1, true) ~= nil).to_be(true)
	-- (Said as a switch, on while the address shows.)
	expect(app:tree('#private-line >> role:switch "Show email"')).never.to_contain("unchecked")
	-- (The rows' copies of it stay hidden: the reveal is that instance's.)
	local _, count = app:tree("#private"):gsub("mara@halcyon%.dev", "")
	expect(count).to_be(1)
	app:click('#private-line >> role:switch "Show email"')
	app:frames(10)
	expect(shows(app, "mara@halcyon.dev")).to_be(false)
end)

test("the toggle never moves the line: its height and baseline are the text's", function()
	local app = open()
	local function top_and_height(): (number, number)
		local tree: string = app:tree("#private-line")
		local line = string.match(tree, "[^\n]+") or ""
		local y, h = line:match(" %-?%d+,(%-?%d+) %d+x(%d+)")
		return tonumber(y) or -1, tonumber(h) or -1
	end
	local y1, h1 = top_and_height()
	app:command("private.hide")
	app:frames(10)
	local y2, h2 = top_and_height()
	expect(y2).to_be(y1)
	expect(h2).to_be(h1)
end)

test("Settings' switch hides every address in the app at once", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:command("shell.settings")
	app:frames(20)
	app:click('#settings-page >> role:switch "Hide email addresses"')
	app:frames(10)
	app:click('#settings-nav >> "Members"')
	app:frames(30)
	expect(shows(app, "Grace Hopper")).to_be(true)
	expect(shows(app, "grace@example.net")).to_be(false)
	expect(shows(app, "edsger@example.net")).to_be(false)
	app:click('#settings-nav >> "Account"')
	app:frames(30)
	expect(shows(app, "ada@example.net")).to_be(false)
	-- Off again: every one plain, in place.
	app:click('#settings-nav >> "General"')
	app:frames(20)
	app:click('#settings-page >> role:switch "Hide email addresses"')
	app:frames(10)
	app:click('#settings-nav >> "Members"')
	app:frames(30)
	expect(shows(app, "grace@example.net")).to_be(true)
end)
