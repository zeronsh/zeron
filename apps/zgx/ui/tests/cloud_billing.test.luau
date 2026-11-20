-- Settings → Cloud and → Billing: listed only while the account has Cloud;
-- Cloud's usage over a range, GitHub and the
-- repositories Cloud can open; Billing's credits, plans and packs, Stripe's
-- pages asked for, and a purchase landing while one is open. On the shell
-- in ui/shell_fixture.luau, the host's calls answered by ui/cloud_stub.luau.

local PAGE = "#settings-page"
local NAV = "#settings-nav"

local function shows(app: any, scope: string, text: string): boolean
	return app:tree(scope):find(text, 1, true) ~= nil
end

local function asked(app: any, entry: string): boolean
	for _, c in (app:state("cloud_stub_calls") :: any) or {} do
		if c == entry then
			return true
		end
	end
	return false
end

local function asked_like(app: any, prefix: string): boolean
	for _, c in (app:state("cloud_stub_calls") :: any) or {} do
		if string.sub(c, 1, #prefix) == prefix then
			return true
		end
	end
	return false
end

local function open(page: string?, offer: boolean?): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	if offer ~= false then
		app:command("cloud_stub.offer")
	end
	app:command("shell.settings")
	app:frames(20)
	if page then
		app:click(`{NAV} >> role:tab "{page}"`)
		app:frames(30)
	end
	return app
end

test("Cloud and Billing are listed only while the account has Cloud", function()
	local app = open(nil, false)
	expect(app:has(`{NAV} >> role:tab "Cloud"`)).to_be(false)
	expect(app:has(`{NAV} >> role:tab "Billing"`)).to_be(false)
	-- (Nor found by the search.)
	app:type(`{NAV} >> #settings-search`, "credits")
	app:frames(10)
	expect(app:has(`#settings-search-results >> "Extra credits"`)).to_be(false)
	app:fill(`{NAV} >> "Search settings"`, "")
	app:command("cloud_stub.offer")
	app:frames(20)
	expect(app:has(`{NAV} >> role:tab "Cloud"`)).to_be(true)
	expect(app:has(`{NAV} >> role:tab "Billing"`)).to_be(true)
	app:type(`{NAV} >> #settings-search`, "extra credits")
	app:frames(10)
	expect(app:has(`#settings-search-results >> "Extra credits"`)).to_be(true)
	app:key("Enter")
	app:frames(30)
	expect(app:state("shell_route_path")).to_be("/settings/billing")
end)

test("Cloud shows its usage, GitHub and the repositories it can open", function()
	local app = open("Cloud")
	expect(app:state("shell_route_path")).to_be("/settings/cloud")
	expect(shows(app, PAGE, "Run agents on machines of their own")).to_be(true)
	-- (No switch: Cloud is on for every account that has it.)
	expect(app:has(`{PAGE} >> role:switch "Cloud"`)).to_be(false)
	-- Usage over the last 30 days, asked as the page opened.
	expect(asked_like(app, "cloud.usage:")).to_be(true)
	expect(shows(app, PAGE, "1,505 credits used")).to_be(true)
	expect(shows(app, PAGE, "44,095 credits left")).to_be(true)
	expect(app:has(`{PAGE} >> role:img "Oct 17, 604 credits"`)).to_be(true)
	-- GitHub connected, and the repositories, the private one said so.
	expect(shows(app, PAGE, "octo-example")).to_be(true)
	expect(shows(app, PAGE, "acme/atlas")).to_be(true)
	expect(shows(app, PAGE, "Private")).to_be(true)
	expect(shows(app, PAGE, "octo/tools")).to_be(true)
end)

test("another range asks for its days", function()
	local app = open("Cloud")
	app:command("cloud_stub.last_7_days")
	app:frames(30)
	local ranges = {}
	for _, c in (app:state("cloud_stub_calls") :: any) or {} do
		local from, to = string.match(c, "^cloud%.usage:([%d-]+):([%d-]+)$")
		if from then
			table.insert(ranges, { from = from, to = to })
		end
	end
	-- Thirty days, then seven, both to today.
	expect(#ranges).to_be(2)
	expect(ranges[1].to).to_be(ranges[2].to)
	expect(ranges[2].from > ranges[1].from).to_be(true)
	expect(app:text(`{PAGE} >> role:button "Usage range"`)).to_contain("Last 7 days")
end)

test("without GitHub, the block offers to connect and shows the code", function()
	local app = open("Cloud")
	app:command("cloud_stub.no_github")
	app:frames(30)
	expect(shows(app, PAGE, "Not connected")).to_be(true)
	expect(shows(app, PAGE, "acme/atlas")).to_be(false)
	app:click(`{PAGE} >> role:button "Connect"`)
	app:frames(30)
	expect(asked(app, "github.connect")).to_be(true)
	expect(shows(app, PAGE, "WDJB-MJHT")).to_be(true)
end)

test("Billing shows the credits, the plans and a pack", function()
	local app = open("Billing")
	expect(app:state("shell_route_path")).to_be("/settings/billing")
	expect(shows(app, PAGE, "44,099")).to_be(true)
	expect(shows(app, PAGE, "about 735 small-machine hours")).to_be(true)
	expect(shows(app, PAGE, "Of 45,000")).to_be(true)
	for _, name in { "Base", "Pro", "Max" } do
		expect(app:has(`{PAGE} >> role:group "{name} plan"`)).to_be(true)
	end
	expect(shows(app, PAGE, "$50")).to_be(true)
	expect(shows(app, PAGE, "Current plan")).to_be(true)
	expect(shows(app, PAGE, "Base · $20 a month")).to_be(true)
	expect(shows(app, PAGE, "Renews Nov 25.")).to_be(true)
	expect(shows(app, PAGE, "12,000 credits")).to_be(true)
	expect(shows(app, PAGE, "Buy for $10")).to_be(true)
end)

test("buying packs opens Stripe's page and the purchase lands", function()
	local app = open("Billing")
	app:click(`{PAGE} >> role:button ~"Buy for"`)
	app:frames(30)
	expect(asked(app, "billing.buy:1")).to_be(true)
	expect(shows(app, PAGE, "Finish in your browser")).to_be(true)
	app:command("cloud_stub.purchase_lands")
	-- (Read again every three seconds while the page is open.)
	app:wait("#never-shown", { timeout_ms = 3400 })
	app:frames(10)
	expect(shows(app, PAGE, "Finish in your browser")).to_be(false)
	expect(shows(app, PAGE, "56,099")).to_be(true)
	-- Switching plan, on one already, goes through the portal.
	app:click(`{PAGE} >> role:group "Pro plan" >> role:button "Switch"`)
	app:frames(30)
	expect(asked(app, "billing.portal")).to_be(true)
end)

test("a billing read that fails says so, and tries again", function()
	local app = open(nil)
	app:command("cloud_stub.fail_next")
	app:click(`{NAV} >> role:tab "Billing"`)
	app:frames(30)
	expect(shows(app, PAGE, "Couldn't load billing")).to_be(true)
	app:click(`{PAGE} >> role:button "Try again"`)
	app:frames(30)
	expect(shows(app, PAGE, "Couldn't load billing")).to_be(false)
	expect(shows(app, PAGE, "44,099")).to_be(true)
end)

-- A node's box in the tree: x, y, w, h.
local function box(app: any, sel: string): (number, number, number, number)
	local line = string.match(app:tree(sel), "[^\n]+") or ""
	local x, y, w, h = string.match(line, " (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)")
	return tonumber(x) or 0, tonumber(y) or 0, tonumber(w) or 0, tonumber(h) or 0
end

test("the usage bars spread across the card, wide and narrow", function()
	local app = open("Cloud")
	for _, width in { 1400, 820 } do
		app:resize(width, 900)
		app:frames(30)
		local cx, _, cw = box(app, `{PAGE} >> #setting-usage`)
		local fx, _, fw = box(app, `{PAGE} >> role:img "Oct 1, 0 credits"`)
		local lx, _, lw = box(app, `{PAGE} >> role:img "Oct 30, 0 credits"`)
		-- The first day at the card's text inset, the last at its far one,
		-- every day the same width.
		expect(fx - cx < 24).to_be(true)
		expect(cx + cw - (lx + lw) < 24).to_be(true)
		expect(math.abs(fw - lw) < 0.6).to_be(true)
		expect(fw > 5).to_be(true)
	end
end)
