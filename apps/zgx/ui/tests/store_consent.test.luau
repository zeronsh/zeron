-- Before you act as yourself in the store, once a template that isn't
-- Zeron's has run, Zeron asks in a window of its own: what's asked as the
-- engine told it, why it's asked there, and nothing done until you say so. A
-- new token is shown there, once. On the sheet in
-- ui/store_consent_fixture.luau.

local function open(show: string?): any
	local app = mount("store_consent_fixture.luau")
	app:resize(460, 480)
	if show then
		app:command(show)
	end
	app:frames(20)
	return app
end

local function box(app: any, target: string): { x: number, y: number, w: number, h: number }
	local line = app:tree(target):match("[^\n]+") or ""
	local x, y, w, h = line:match("(%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)")
	return { x = tonumber(x) or 0, y = tonumber(y) or 0, w = tonumber(w) or 0, h = tonumber(h) or 0 }
end

test("publishing says what, as whom, and why Zeron asks", function()
	local app = open()
	local tree = app:tree("#store-consent")
	expect(tree).to_contain("Publish Focus timer 1.3.0?")
	expect(tree).to_contain("under your username")
	expect(tree).to_contain("focus-timer")
	expect(tree).to_contain("Why Zeron asks here")
	expect(tree).to_contain("A template that isn't Zeron's has run")
	app:click("#store-consent-allow")
	app:frames(4)
	expect(app:state("calls")).to_be("decide:true")
end)

test("Not now answers no", function()
	local app = open("unyank")
	expect(app:tree("#store-consent")).to_contain("Allow new installs of focus-timer 1.2.0?")
	app:click("#store-consent-decline")
	app:frames(4)
	expect(app:state("calls")).to_be("decide:false")
end)

test("a new token says its name, what it can publish and when it ends", function()
	local app = open("token")
	local tree = app:tree("#store-consent")
	expect(tree).to_contain("Make this deploy token?")
	expect(tree).to_contain("GitHub Actions")
	expect(tree).to_contain("tidy")
	expect(tree).to_contain("2026")
	app:command("token_all")
	app:frames(20)
	expect(app:tree("#store-consent")).to_contain("All your plugins")
	expect(app:tree("#store-consent")).to_contain("Never")
end)

test("while it's being made it can be stopped and not asked again", function()
	local app = open("busy")
	expect(app:has("#store-consent-signing-in")).to_be(true)
	app:click("#store-consent-allow")
	app:frames(4)
	expect(app:state("calls")).to_be("")
	app:click("#store-consent-decline")
	app:frames(4)
	expect(app:state("calls")).to_be("decide:false")
end)

test("the token is shown in this window, once made, with its Copy", function()
	local app = open("made")
	expect(app:has("#store-consent-token")).to_be(true)
	expect(app:has("#key-secret")).to_be(true)
	expect(app:has("#store-consent-allow")).to_be(false)
	app:click("#key-secret-copy")
	app:frames(8)
	expect(app:state("calls")).to_contain("copy:zdt_abcdefgh0123456789")
	app:click("#store-consent-done")
	app:frames(4)
	expect(app:state("calls")).to_contain("close")
end)

test("a thing's own words are drawn as they are and everything stays on the sheet", function()
	for _, where in { "en", "zh-Hans" } do
		local app = open()
		app:command(`locale_{where}`)
		for _, show in { "hostile", "token", "unyank", "publish" } do
			app:command(show)
			app:frames(20)
			local sheet, allow = box(app, "#store-consent"), box(app, "#store-consent-allow")
			expect(allow.y + allow.h <= sheet.y + sheet.h).to_be(true)
			expect(allow.x + allow.w <= sheet.x + sheet.w).to_be(true)
			local facts = box(app, "#store-consent-facts")
			expect(facts.x + facts.w <= sheet.x + sheet.w).to_be(true)
		end
		app:command("hostile")
		app:frames(20)
		expect(app:tree("#store-consent")).to_contain("{count} [b](https://x)")
		expect(app:tree("#store-consent")).to_contain(if where == "en" then "Not now" else "暂不")
	end
end)
