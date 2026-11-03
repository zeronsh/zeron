-- A plugin's settings (screens/plugins/settings): a page of its own in
-- Settings, under "Plugins" ahead of the "Zeron" group, while it's on;
-- drawn by Zeron from what it declares; what's set goes to the host and
-- stays (turned off and on again too); a setting never set follows its
-- default as the plugin changes it; Settings search finds them; and the
-- Plugins page links to them. On ui/plugin_settings_fixture.luau, which
-- plays the host. (The plugin hearing a change: src/plugins/examples_tests
-- `greeter_reads_its_settings_and_hears_each_change`.)

local NAV = "#settings-nav"
local PAGE = "#settings-page"
local TAB = `{NAV} >> role:tab "Greeter"`

local function open(): any
	local app = mount("plugin_settings_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:command("shell.settings")
	app:frames(20)
	return app
end

local function greeter(app: any): any
	app:command("plugin_on")
	app:frames(20)
	app:click(TAB)
	app:frames(30)
	return app
end

-- A node's identity under `scope`, by a line's text: `@id`.
local function node(app: any, scope: string, part: string): string?
	for line in app:tree(scope):gmatch("[^\n]+") do
		if line:find(part, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

local function said(app: any, entry: string): boolean
	for _, c in (app:state("plugin_settings_calls") :: any) or {} do
		if c == entry then
			return true
		end
	end
	return false
end

-- The line for `part` under the page (a control's, with its state).
local function line(app: any, part: string): string
	for l in app:tree(PAGE):gmatch("[^\n]+") do
		if l:find(part, 1, true) then
			return l
		end
	end
	return ""
end

test("a plugin's page is listed under Plugins while it's on, and goes when it's off", function()
	local app = open()
	expect(app:has(TAB)).to_be(false)
	app:command("plugin_on")
	app:frames(20)
	expect(app:has(TAB)).to_be(true)
	-- Under its own heading, with its tile, ahead of the "Zeron" group
	-- (About and updates stay last).
	local nav = app:tree(NAV)
	local heading = nav:find('text "Plugins"', 1, true)
	local tab = nav:find('tab "Greeter"', 1, true) or 0
	local zeron = nav:find('text "Zeron"', 1, true) or 0
	local about = nav:find('tab "About & updates"', 1, true) or 0
	expect(heading ~= nil and heading < tab and tab < zeron and zeron < about).to_be(true)
	expect(app:tree(TAB)).to_contain('text "G"')
	-- Drawn by Zeron from what it declares: its sections and every kind.
	app:click(TAB)
	app:frames(30)
	for _, part in { 'heading "Greeter"', 'heading "Greeting"', 'heading "More"', 'switch "Show the greeting"', 'textbox "Greeting"', 'radio "Excited"', 'button "Where it shows"', 'textbox "Greet again every"', 'button "Change Wave shortcut, now ⌃⌥G"', 'button "Choose Postcards folder"' } do
		expect(app:tree(PAGE)).to_contain(part)
	end
	-- Turned off while open: the page goes, and its row with it.
	app:command("plugin_off")
	app:frames(30)
	expect(app:has(TAB)).to_be(false)
	expect(app:tree(PAGE)).never.to_contain("Show the greeting")
	expect(app:error()).to_be_nil()
end)

test("what's set goes to the host and stays, in place, off and on again", function()
	local app = greeter(open())
	local tone_row = node(app, PAGE, "#setting-tone")
	local switch = node(app, PAGE, 'switch "Show the greeting"')
	app:click(`{PAGE} >> role:switch "Show the greeting"`)
	app:frames(10)
	expect(said(app, "set:settings-demo:show:false")).to_be(true)
	app:click(`{PAGE} >> role:radio "Excited"`)
	app:frames(10)
	expect(said(app, "set:settings-demo:tone:excited")).to_be(true)
	app:fill(`{PAGE} >> role:textbox "Greeting"`, "Hi there")
	app:key("Enter")
	app:frames(10)
	expect(said(app, "set:settings-demo:words:Hi there")).to_be(true)
	-- A number out of its bounds is said, and not set; one within is.
	app:fill(`{PAGE} >> role:textbox "Greet again every"`, "500")
	app:key("Enter")
	app:frames(10)
	expect(app:tree(PAGE)).to_contain("At most 120")
	expect(said(app, "set:settings-demo:every:500")).to_be(false)
	app:fill(`{PAGE} >> role:textbox "Greet again every"`, "15")
	app:key("Enter")
	app:frames(10)
	expect(said(app, "set:settings-demo:every:15")).to_be(true)
	expect(app:tree(PAGE)).never.to_contain("At most 120")
	-- A folder, chosen through the host.
	app:click(`{PAGE} >> role:button "Choose Postcards folder"`)
	app:frames(10)
	expect(said(app, "folder:settings-demo:album")).to_be(true)
	expect(app:tree(PAGE)).to_contain('text "Postcards"')
	-- Every change in place: the same rows, the same switch.
	expect(node(app, PAGE, "#setting-tone")).to_be(tone_row)
	expect(node(app, PAGE, 'switch "Show the greeting"')).to_be(switch)
	-- Off and on again: as it was left.
	app:command("plugin_off")
	app:frames(20)
	app:command("plugin_on")
	app:frames(20)
	app:click(TAB)
	app:frames(30)
	expect(line(app, 'switch "Show the greeting"')).to_contain("unchecked")
	expect(line(app, 'radio "Excited"')).to_contain("checked")
	expect(line(app, 'radio "Excited"')).never.to_contain("unchecked")
	expect(line(app, 'textbox "Greeting"')).to_contain('="Hi there"')
	expect(line(app, 'textbox "Greet again every"')).to_contain('="15"')
	expect(app:error()).to_be_nil()
end)

test("a setting never set follows the plugin's default as it changes", function()
	local app = greeter(open())
	expect(line(app, 'textbox "Greeting"')).to_contain('="Hello"')
	app:fill(`{PAGE} >> role:textbox "Greet again every"`, "15")
	app:key("Enter")
	app:frames(10)
	local words_row = node(app, PAGE, "#setting-words")
	-- An update changes both defaults: the one never set follows, the one
	-- set stays as set.
	app:command("defaults_changed")
	app:frames(20)
	expect(line(app, 'textbox "Greeting"')).to_contain('="Howdy"')
	expect(line(app, 'textbox "Greet again every"')).to_contain('="15"')
	expect(node(app, PAGE, "#setting-words")).to_be(words_row)
end)

test("search finds a plugin's setting and opens its page at it, while it's on", function()
	local app = open()
	app:command("plugin_on")
	app:frames(20)
	app:type(`{NAV} >> role:textbox "Search settings"`, "greet again")
	app:frames(10)
	expect(app:has('#settings-search-results >> role:option "Greet again every"')).to_be(true)
	expect(app:tree("#settings-search-results")).to_contain("Greeter · More")
	app:key("Enter")
	app:frames(30)
	expect(app:tree(PAGE)).to_contain('heading "Greeter"')
	expect(app:has("#setting-every")).to_be(true)
	-- Off: not found any more.
	app:command("plugin_off")
	app:frames(20)
	app:fill(`{NAV} >> role:textbox "Search settings"`, "postcards")
	app:frames(10)
	expect(app:has('#settings-search-results >> role:option "Postcards folder"')).to_be(false)
	expect(app:tree("#settings-search-results")).to_contain("No settings match.")
end)

test("the Plugins page links to a plugin's settings only while it has them", function()
	local app = open()
	app:click(`{NAV} >> role:tab "Plugins"`)
	app:frames(30)
	expect(app:has(`{PAGE} >> role:button "Greeter settings"`)).to_be(false)
	-- Turned on from its row: its link, which opens its page.
	app:click(`{PAGE} >> role:switch "Greeter"`)
	-- (Asked once the knob has landed: time passes in a wait.)
	app:wait("#the-knob-lands", { timeout_ms = 400 })
	app:frames(20)
	expect(said(app, "set_off:settings-demo:false")).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Greeter settings"`)).to_be(true)
	app:click(`{PAGE} >> role:button "Greeter settings"`)
	app:frames(30)
	expect(app:tree(PAGE)).to_contain('heading "Greeter"')
	expect(app:tree(PAGE)).to_contain("Show the greeting")
end)

test("a chosen option's own words stand in for the setting's", function()
	local app = greeter(open())
	expect(app:tree("#setting-place")).to_contain("After the title bar's buttons.")
	expect(app:tree("#setting-place")).never.to_contain("Its place in the title bar.")
	local row = node(app, PAGE, "#setting-place")
	app:click(`{PAGE} >> role:button "Where it shows"`)
	app:frames(20)
	app:click('role:menuitem "At the start"')
	app:frames(20)
	expect(said(app, "set:settings-demo:place:start")).to_be(true)
	expect(app:tree("#setting-place")).to_contain("Before the title bar's buttons.")
	expect(node(app, PAGE, "#setting-place")).to_be(row)
end)

test("a status the plugin reports is drawn beside the control, and cleared", function()
	local app = greeter(open())
	local row = node(app, PAGE, "#setting-keys")
	expect(app:tree("#setting-keys")).never.to_contain('"Set up"')
	app:command("status_set")
	app:frames(20)
	expect(app:tree("#setting-keys")).to_contain('"Set up"')
	expect(app:tree("#setting-keys")).to_contain("Another app has these keys. Choose others.")
	expect(app:tree("#setting-keys")).never.to_contain("Keys that wave back.")
	expect(node(app, PAGE, "#setting-keys")).to_be(row)
	app:command("status_clear")
	app:frames(20)
	expect(app:tree("#setting-keys")).never.to_contain('"Set up"')
	expect(app:tree("#setting-keys")).to_contain("Keys that wave back.")
	-- Turned off and on: none until it reports one again.
	app:command("status_set")
	app:command("plugin_off")
	app:frames(10)
	app:command("status_clear")
	app:command("plugin_on")
	app:frames(20)
	app:click(TAB)
	app:frames(30)
	expect(app:tree("#setting-keys")).never.to_contain('"Set up"')
end)

test("permission rows are Zeron's own plugins' alone, every word Zeron's", function()
	local app = greeter(open())
	-- Greeter isn't Zeron's: its declared Screen Recording row isn't there.
	expect(app:tree(PAGE)).never.to_contain("Screen Recording")
	app:command("snapper_on")
	app:frames(20)
	app:click(`{NAV} >> role:tab "Snapper"`)
	app:frames(30)
	local rec = app:tree("#setting-screen-recording")
	expect(rec).to_contain('"Screen Recording"')
	expect(rec).to_contain("Lets Snapper capture the window in front.")
	expect(rec).to_contain('"Required"')
	expect(rec).to_contain('"Allow Screen Recording"')
	expect(app:tree("#setting-accessibility")).to_contain('"Granted"')
	expect(app:tree("#setting-accessibility")).never.to_contain("Check again")
	expect(app:tree(PAGE)).to_contain("macOS asks for these for the app that opened Zeron")
	-- Asking: macOS's prompt, then System Settings.
	app:click('#setting-screen-recording >> role:button "Allow Screen Recording"')
	app:frames(10)
	expect(said(app, "request:screen-recording")).to_be(true)
	expect(app:tree("#setting-screen-recording")).to_contain('"Open System Settings"')
end)

test("Check again asks the host afresh", function()
	local app = open()
	app:command("snapper_on")
	app:frames(20)
	app:click(`{NAV} >> role:tab "Snapper"`)
	app:frames(30)
	expect(app:tree("#setting-screen-recording")).to_contain('"Required"')
	-- Granted in System Settings: nothing changes until Zeron looks again.
	app:command("grant_capture")
	app:frames(10)
	expect(app:tree("#setting-screen-recording")).to_contain('"Required"')
	local before = #((app:state("plugin_settings_calls") :: any) or {})
	app:click('#setting-screen-recording >> role:button "Check Screen Recording again"')
	app:frames(20)
	local calls = (app:state("plugin_settings_calls") :: any) or {}
	expect(#calls > before and calls[#calls] == "refresh").to_be(true)
	expect(app:tree("#setting-screen-recording")).to_contain('"Granted"')
	expect(app:tree("#setting-screen-recording")).never.to_contain("Check again")
end)

test("a page says the plugin's own intro under its title, or Zeron's line", function()
	local app = greeter(open())
	expect(app:tree(PAGE)).to_contain("Settings for the Greeter plugin.")
	app:command("snapper_on")
	app:frames(20)
	app:click(`{NAV} >> role:tab "Snapper"`)
	app:frames(30)
	expect(app:tree(PAGE)).to_contain("Set up once, one permission at a time.")
	expect(app:tree(PAGE)).never.to_contain("Settings for the Snapper plugin.")
end)

-- A plugin's wallpaper: used from its own page (Zeron's "Use as
-- wallpaper" row, its effect beside it), said on Appearance's Wallpaper row
-- with a way to its page, taken back by choosing None or Image there,
-- falling back to Appearance's kept choice when the plugin goes, and one
-- plugin's at a time.
local NEW_CHAT = "#appearance-new-chat"
local function box_of(app: any, scope: string, part: string): { x: number, y: number, w: number, h: number }?
	for l in app:tree(scope):gmatch("[^\n]+") do
		if l:find(part, 1, true) then
			local x, y, w, h = l:match("(%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
			if x then
				return { x = tonumber(x) :: number, y = tonumber(y) :: number, w = tonumber(w) :: number, h = tonumber(h) :: number }
			end
		end
	end
	return nil
end
local function appearance_page(app: any)
	app:click(`{NAV} >> role:tab "Appearance"`)
	app:frames(20)
end
local function plugin_page(app: any, name: string)
	app:click(`{NAV} >> role:tab "{name}"`)
	app:frames(20)
end

test("a plugin's wallpaper is used from its own page, said on Appearance, and taken back there", function()
	local app = open()
	app:resize(1200, 1800)
	app:command("scenes_on")
	app:frames(20)
	app:set_state("shell_wallpaper", "image")
	app:set_state("shell_wallpaper_image", { path = "assets/favicons/zeron.png", name = "zeron.png", w = 512, h = 512, x = 0.5, y = 0.5, zoom = 1 })
	-- Appearance offers None and Image alone, the plugin nowhere in them.
	appearance_page(app)
	app:click(`{PAGE} >> role:button "Wallpaper"`)
	app:frames(10)
	expect(app:has('role:menuitem "Parallax scenes"')).to_be(false)
	app:key("Escape")
	app:frames(10)
	-- Its page: Zeron's row, off; on, its effect beside it.
	plugin_page(app, "Parallax scenes")
	expect(line(app, "Use as wallpaper")).never.to_be("")
	expect(app:has(`{PAGE} >> role:button "Effect"`)).to_be(false)
	app:click(`{PAGE} >> role:switch "Use as wallpaper"`)
	app:frames(10)
	expect(app:state("shell_wallpaper_plugin")).to_be("parallax-scenes")
	expect(app:has(`{PAGE} >> role:button "Effect"`)).to_be(true)
	expect(line(app, "A new chat opens on what Parallax scenes draws")).never.to_be("")
	-- (Its switch inline at the row's end, beside the words.)
	local name, switch = box_of(app, PAGE, 'text "Use as wallpaper"'), box_of(app, PAGE, 'switch "Use as wallpaper"')
	expect((switch :: any).x > (name :: any).x + 300).to_be(true)
	-- Appearance says where it's from, in one inline row; its picture rows go.
	appearance_page(app)
	expect(line(app, "From Parallax scenes.")).never.to_be("")
	expect(app:count(`{NEW_CHAT} >> .settings-rule`)).to_be(0)
	local words, select = box_of(app, NEW_CHAT, 'text "Wallpaper"'), box_of(app, NEW_CHAT, 'button "Wallpaper"')
	expect((select :: any).x > (words :: any).x + 300).to_be(true)
	expect(math.abs((select :: any).y - (words :: any).y) < 24).to_be(true)
	expect(app:has("#new-chat-background")).to_be(true)
	-- Its page a click away.
	app:click(`{PAGE} >> role:button "Open Parallax scenes"`)
	app:frames(20)
	expect(line(app, "Use as wallpaper")).never.to_be("")
	-- Taken back from Appearance: None chosen, the plugin let go.
	appearance_page(app)
	app:click(`{PAGE} >> role:button "Wallpaper"`)
	app:frames(10)
	app:click('role:menuitem "None"')
	app:frames(10)
	expect(app:state("shell_wallpaper_plugin")).to_be("")
	expect(app:state("shell_wallpaper")).to_be("none")
	expect(line(app, "From Parallax scenes")).to_be("")
	expect(app:has("#new-chat-background")).to_be(false)
end)

test("a plugin's wallpaper going off falls back to Appearance's choice, and another plugin's takes over", function()
	local app = open()
	app:resize(1200, 1800)
	app:command("scenes_on")
	app:command("backdrops_on")
	app:frames(20)
	app:set_state("shell_wallpaper", "image")
	app:set_state("shell_wallpaper_image", { path = "assets/favicons/zeron.png", name = "zeron.png", w = 512, h = 512, x = 0.5, y = 0.5, zoom = 1 })
	plugin_page(app, "Parallax scenes")
	app:click(`{PAGE} >> role:switch "Use as wallpaper"`)
	app:frames(10)
	-- Another plugin's, from its page: it takes over, the first let go.
	plugin_page(app, "Backdrops")
	expect(line(app, "Parallax scenes is the wallpaper now")).never.to_be("")
	app:click(`{PAGE} >> role:switch "Use as wallpaper"`)
	app:frames(10)
	expect(app:state("shell_wallpaper_plugin")).to_be("backdrops")
	plugin_page(app, "Parallax scenes")
	expect(line(app, "Backdrops is the wallpaper now")).never.to_be("")
	-- Parallax scenes again, then turned off: Appearance's picture, kept.
	app:click(`{PAGE} >> role:switch "Use as wallpaper"`)
	app:frames(10)
	expect(app:state("shell_wallpaper_plugin")).to_be("parallax-scenes")
	app:command("scenes_off")
	app:frames(20)
	appearance_page(app)
	expect(line(app, "From Parallax scenes")).to_be("")
	expect(app:state("shell_wallpaper")).to_be("image")
	expect(line(app, "zeron.png")).never.to_be("")
	expect(app:has("#new-chat-background")).to_be(false)
	-- Back on: its wallpaper again, as it was left.
	app:command("scenes_on")
	app:frames(20)
	expect(line(app, "From Parallax scenes.")).never.to_be("")
end)

test("a plugin's page says Unverified at its top right when Zeron can't vouch for who made it", function()
	local app = greeter(open())
	expect(app:has(`{PAGE} >> "Unverified"`)).to_be(false)
	app:command("greeter_unverified")
	app:frames(20)
	expect(app:has(`{PAGE} >> "Unverified"`)).to_be(true)
end)
