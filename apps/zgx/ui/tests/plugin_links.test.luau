-- A pull request's link goes to the GitHub plugin's tab beside the chat, while the
-- plugin is on and its setting is; other links, and with either off, go as links do.
-- On the shell in ui/shell_fixture.luau.

local function github(on: boolean): any
	return { { plugin = "github", name = "GitHub", settings = {}, values = { open_pr_links = on } } }
end

local function open(settings: any): any
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	app:set_state("plugins.settings", settings)
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(30)
	return app
end

local function pr_tab(app: any): boolean
	return app:has('#panel-strip >> role:tab "#12"')
end

test("a pull request's link opens the plugin's tab, once however often it's clicked", function()
	local app = open(github(true))
	app:command("click_pr_link")
	app:frames(40)
	expect(pr_tab(app)).to_be(true)
	expect(app:state("links_last")).to_be("app:https://github.com/acme/atlas/pull/12/files")
	app:command("click_pr_link")
	app:frames(40)
	local count = select(2, string.gsub(app:tree("#panel-strip"), 'tab "#12"', ""))
	expect(count).to_be(1)
	expect(app:error()).to_be_nil()
end)

test("with the setting off, or the plugin off, it goes as links do", function()
	for _, settings in { github(false), {} } do
		local app = open(settings)
		app:command("click_pr_link")
		app:frames(40)
		expect(pr_tab(app)).to_be(false)
	end
end)

test("an issue's link and another site's are left alone", function()
	local app = open(github(true))
	for _, link in { "click_issue_link", "click_other_link" } do
		app:command(link)
		app:frames(40)
		expect(pr_tab(app)).to_be(false)
	end
end)

-- A link the GitHub plugin opens itself (its viewer's Open on GitHub, a
-- comment's) is the pull request's page: it goes where links go, and never
-- back into the plugin's tab, which would only bring that tab forward.
test("a pull request's link a plugin opens itself goes where links go", function()
	local app = open(github(true))
	app:set_state("shell_fixture.plugins", true)
	app:frames(10)
	local url = "https://github.com/acme/atlas/pull/12"
	app:set_state("plugins.request", { { seq = 1, kind = "link", title = "GitHub", value = url } })
	app:frames(40)
	expect(pr_tab(app)).to_be(false)
	local last = app:state("links_last")
	expect(string.sub(last, -#url)).to_be(url)
	-- (And a link in the chat is still the plugin's, after.)
	app:command("click_pr_link")
	app:frames(40)
	expect(pr_tab(app)).to_be(true)
	expect(app:error()).to_be_nil()
end)
