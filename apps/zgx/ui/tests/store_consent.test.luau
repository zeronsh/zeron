-- Before you act as yourself in the store, once a template that isn't
-- Zeron's has run, Zeron asks in a window of its own: what's asked as the
-- engine told it, why it's asked there, and nothing done until you say so. A
-- new token is shown there, once. On the sheet in
-- ui/store_consent_fixture.luau.

local function open(show: string?): any
	local app = mount("store_consent_fixture.luau")
	app:resize(460, 540)
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

local function armed(app: any)
	app:wait("#never-there", { timeout_ms = 900 })
	app:frames(4)
end

test("publishing says what, as whom, and why Zeron asks", function()
	local app = open()
	local tree = app:tree("#store-consent")
	expect(tree).to_contain("Publish Focus timer 1.3.0?")
	expect(tree).to_contain("under your username")
	expect(tree).to_contain("focus-timer")
	expect(tree).to_contain("Why Zeron asks here")
	expect(tree).to_contain("A template that isn't Zeron's has run")
	-- (Not at once: a click meant for the main window, as this one comes up.)
	app:click("#store-consent-allow")
	app:frames(4)
	expect(app:state("calls")).to_be("")
	armed(app)
	app:click("#store-consent-allow")
	app:frames(4)
	expect(app:state("calls")).to_be("decide:true")
end)

test("brought forward again, it takes a yes only after a moment again", function()
	local app = open()
	armed(app)
	app:command("raised")
	app:frames(4)
	expect(app:tree("#store-consent-allow")).to_contain("disabled")
	app:click("#store-consent-allow")
	app:frames(4)
	expect(app:state("calls")).to_be("")
	armed(app)
	app:click("#store-consent-allow")
	app:frames(4)
	expect(app:state("calls")).to_be("decide:true")
end)

test("Not now answers no", function()
	local app = open("unyank")
	expect(app:tree("#store-consent")).to_contain("Allow new installs of Focus timer 1.2.0?")
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
	app:command("token_both")
	app:frames(20)
	expect(app:tree("#store-consent")).to_contain("All your plugins and templates")
	expect(app:tree("#store-consent")).to_contain("A template can do anything Zeron can.")
end)

test("while it's being made it can be stopped and not asked again", function()
	local app = open("busy")
	expect(app:has("#store-consent-signing-in")).to_be(true)
	expect(app:tree("#store-consent-decline")).to_contain("Stop waiting")
	armed(app)
	app:click("#store-consent-allow")
	app:frames(4)
	expect(app:state("calls")).to_be("")
	app:click("#store-consent-decline")
	app:frames(4)
	expect(app:state("calls")).to_be("decide:false")
end)

-- Its status line keeps its room in every step, so nothing moves.
test("nothing in a token's window moves as it's asked, made and shown", function()
	local app = open("token")
	local status = box(app, "#store-consent-status")
	local allow = box(app, "#store-consent-allow")
	expect(status.h).to_be(16)
	expect(app:has("#store-consent-signing-in")).to_be(false)
	app:command("busy")
	app:frames(20)
	expect(box(app, "#store-consent-status").y).to_be(status.y)
	expect(box(app, "#store-consent-allow").y).to_be(allow.y)
	expect(box(app, "#store-consent-signing-in").h).to_be(16)
	app:command("made")
	app:frames(40)
	expect(box(app, "#store-consent-status").y).to_be(status.y)
	expect(box(app, "#store-consent-done").y).to_be(allow.y)
end)

test("a token's ids are under their own headings, and all of them is said where it's all", function()
	local app = open("mixed")
	local facts = app:tree("#store-consent-facts")
	expect(facts).to_contain("2 plugins, 1 template")
	expect(facts).to_contain('"Plugins"')
	expect(facts).to_contain('"Templates"')
	expect((facts:find("tidy", 1, true) :: number) < (facts:find("midnight", 1, true) :: number)).to_be(true)
	app:command("both_said")
	app:frames(20)
	expect(app:tree("#store-consent-facts")).to_contain("All your plugins")
	-- Which are templates isn't known: how many, no kind said.
	app:command("token")
	app:frames(20)
	expect(app:tree("#store-consent-facts")).to_contain("2 chosen")
end)

-- Each label is on its value's first line, and what's plain to read is said
-- once: a publish's card is its id.
test("a fact's label is on its value's first line, and a publish says its id alone", function()
	local app = open("many")
	local rows: { [string]: number } = {}
	for line in app:tree("#store-consent-facts"):gmatch("[^\n]+") do
		local said, y = line:match('text "([^"]*)" %-?[%d.]+,(%-?[%d.]+)')
		if said then
			rows[said] = tonumber(y) :: number
		end
	end
	expect(rows["Can publish"]).to_be(rows["50 plugins"])
	app:command("publish")
	app:frames(20)
	expect(app:tree("#store-consent-facts")).to_contain("focus-timer")
	expect(app:tree("#store-consent-facts")).never.to_contain("1.3.0")
	expect(app:tree("#store-consent-facts")).never.to_contain("Focus timer")
	expect(app:count("role:status")).to_be(0)
end)

test("the token is shown in this window, once made, with its Copy", function()
	local app = open("made")
	expect(app:has("#store-consent-token")).to_be(true)
	expect(app:has("#key-secret")).to_be(true)
	expect(app:tree("#key-secret")).to_contain("Deploy token")
	expect(app:has("#store-consent-allow")).to_be(false)
	app:click("#key-secret-copy")
	app:frames(8)
	expect(app:state("calls")).to_contain("copy:zdt_abcdefgh0123456789")
	app:click("#store-consent-done")
	app:frames(4)
	expect(app:state("calls")).to_contain("close")
end)

-- What's asked is never cut: the facts card holds every row of it, and where
-- the window can't hold it all it scrolls, the answer staying in view.
test("every fact is whole, and the answer stays in view", function()
	for _, where in { "en", "zh-Hans" } do
		local app = open()
		app:command(`locale_{where}`)
		for _, show in { "publish", "unyank", "token", "token_both", "token_all", "hostile", "many" } do
			app:command(show)
			app:frames(20)
			local facts, said, view = box(app, "#store-consent-facts"), box(app, "#store-consent-said"), box(app, "#store-consent-scroll")
			-- (Its rows, each inside it; all of it above what's under it, so
			-- its card isn't squeezed over it; that inside what scrolls.)
			expect(facts.y + facts.h <= box(app, "#store-consent-why").y).to_be(true)
			for line in app:tree("#store-consent-facts"):gmatch("[^\n]+") do
				local y, h = line:match('text "[^"]*" %-?[%d.]+,(%-?[%d.]+) [%d.]+x([%d.]+)')
				if y then
					expect((tonumber(y) :: number) >= facts.y).to_be(true)
					expect((tonumber(y) :: number) + (tonumber(h) :: number) <= facts.y + facts.h + 0.5).to_be(true)
				end
			end
			expect(facts.y + facts.h <= said.y + said.h + 0.5).to_be(true)
			local allow = box(app, "#store-consent-allow")
			expect(view.y + view.h <= allow.y).to_be(true)
			expect(allow.y + allow.h <= 540).to_be(true)
			-- (And nothing in it wider than it: wrapped, not cut.)
			for line in app:tree("#store-consent-facts"):gmatch("[^\n]+") do
				local x, w = line:match('text "[^"]*" (%-?[%d.]+),%-?[%d.]+ ([%d.]+)x')
				if x then
					expect((tonumber(x) :: number) + (tonumber(w) :: number) <= facts.x + facts.w).to_be(true)
				end
			end
			if show == "many" then
				-- Fifty ids, each on its own line, whole: never broken.
				expect(app:count(".store-consent-one-id")).to_be(50)
				local _, whole = app:tree("#store-consent-facts"):gsub('a%-plugin%-with%-the%-longest%-id%-a%-stores%-%d%d" [%d.,-]+ [%d.]+x16', "")
				expect(whole).to_be(50)
				expect(facts.h > 400).to_be(true)
				expect(app:tree("#store-consent-facts")).to_contain(if where == "en" then "50 plugins" else "50 个插件")
			end
			local last = app:tree("#store-consent-facts")
			if show == "token" or show == "token_all" then
				expect(last).to_contain(if where == "en" then "Can publish" else "可以发布")
			end
			-- All of it can be reached.
			app:scroll("#store-consent-scroll", 5000)
			app:frames(10)
			local end_said = box(app, "#store-consent-said")
			expect(end_said.y + end_said.h <= view.y + view.h + 1).to_be(true)
		end
	end
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
