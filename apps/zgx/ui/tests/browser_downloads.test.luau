-- The browser tab's downloads: a page's download opens the save dialog
-- with its name; while it saves, a line under the bar says how far, then
-- Saved with Show in Finder, and a × puts it away; a cancelled dialog
-- leaves nothing; a failure says why. On the panel in ui/panel_fixture.luau
-- (its page and its download stood in for).

local function calls(app: any): string
	return app:state("test_web_calls") or ""
end

local function page(): any
	local app = mount("panel_fixture.luau")
	app:resize(900, 640)
	app:frames(10)
	expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
	app:click("#panel-open-browser")
	expect(app:wait('role:combobox "Address"', { timeout_ms = 15000 })).to_be(true)
	app:click("#outside")
	app:type('role:combobox "Address"', "example.com")
	app:key("Enter")
	app:frames(10)
	return app
end

local function line(app: any): string
	return app:tree() :match('"(report%.pdf, [^"]*)"') or ""
end

test("browser downloads: the save dialog, how far, Saved and Show in Finder, then put away", function()
	local app = page()
	app:set_state("test_web_download_mode", "save")
	app:command("test.web_download")
	app:frames(5)
	expect(calls(app)).to_contain("download:ask:report.pdf")
	-- (The stand-in's dialog answers at once: saving, nothing read yet.)
	expect(line(app)).to_be("report.pdf, Saving…")
	app:command("test.web_download_step")
	app:frames(5)
	expect(line(app)).to_be("report.pdf, 50%")
	app:command("test.web_download_step")
	app:frames(5)
	expect(line(app)).to_be("report.pdf, Saved")
	expect(app:has('role:button "Show in Finder"')).to_be(true)
	app:click('role:button "Dismiss report.pdf"')
	app:frames(5)
	expect(line(app)).to_be("")
	expect(app:has('role:group "Downloads"')).to_be(false)
end)

test("browser downloads: a cancelled dialog leaves nothing", function()
	local app = page()
	app:set_state("test_web_download_mode", "cancel")
	app:command("test.web_download")
	app:frames(5)
	expect(calls(app)).to_contain("download:ask:report.pdf")
	expect(line(app)).to_be("")
end)

test("browser downloads: a failure says why", function()
	local app = page()
	app:set_state("test_web_download_mode", "fail")
	app:command("test.web_download")
	app:command("test.web_download_step")
	app:command("test.web_download_step")
	app:frames(5)
	expect(line(app)).to_be("report.pdf, Couldn’t download: disk full")
	expect(app:has('role:button "Show in Finder"')).to_be(false)
end)
