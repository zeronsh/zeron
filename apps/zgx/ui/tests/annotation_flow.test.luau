-- The browser's annotator, as the shipping app's flow has it (and what
-- this one adds): a session ends at its 24th pick, on another page, and on
-- another tab showing; Escape before any pick leaves the keyboard on the
-- page; a session's picks' chips wear the annotate glyph, and under the
-- pointer a card says what the element is. On the panel in
-- ui/panel_fixture.luau (its page stood in for), and the chip strip in
-- ui/cargo_fixture.luau.

local function eventually(app: any, ok: () -> boolean, ms: number?): boolean
	for _ = 1, math.ceil((ms or 5000) / 50) do
		if ok() then
			return true
		end
		app:wait('"never shown, only waited past"', { timeout_ms = 50 })
	end
	return ok()
end

local function calls(app: any): string
	return app:state("test_web_calls") or ""
end

-- The panel with a page, annotation mode on; its nonce.
local function annotating(): (any, number)
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
	app:click('role:checkbox "Annotate"')
	app:frames(3)
	return app, app:state("test_web_nonce")
end

local function pick(app: any, nonce: number, n: number)
	app:set_state("test_web_message", {
		kind = "zeron.annotation",
		nonce = nonce,
		event = "pick",
		pick = {
			url = "https://example.com/", title = "Example Domain", element = `div.card{n}`, selector = `#c{n}`,
			path = { "body" }, role = "", name = "", text = `Card {n}`, html = `<div class="card{n}">Card {n}</div>`,
			rect = { 10, 20, 120, 32 }, viewport = { 800, 600 }, styles = {}, marker = n,
		},
	})
	app:command("test.web_message")
	app:frames(2)
end

local function on(app: any): boolean
	return not string.find(app:tree('role:checkbox "Annotate"'), "unchecked", 1, true)
end

test("annotation flow: a session ends at its 24th pick, the page's picker stopped", function()
	local app, nonce = annotating()
	for n = 1, 23 do
		pick(app, nonce, n)
	end
	expect(on(app)).to_be(true)
	expect(#(app:state("chat.composer.cargo") or {})).to_be(23)
	pick(app, nonce, 24)
	app:frames(3)
	expect(#(app:state("chat.composer.cargo") or {})).to_be(24)
	expect(on(app)).to_be(false)
	expect(calls(app)).to_contain("run:stop")
	-- (A 25th the page might still post is no chip.)
	pick(app, nonce, 25)
	expect(#(app:state("chat.composer.cargo") or {})).to_be(24)
end)

test("annotation flow: another page ends the mode", function()
	local app = annotating()
	expect(on(app)).to_be(true)
	app:set_state("test_web_nav_kind", "committed")
	app:command("test.web_navigate")
	app:frames(3)
	expect(on(app)).to_be(false)
end)

test("annotation flow: another tab showing stops annotating", function()
	local app = annotating()
	expect(on(app)).to_be(true)
	app:command("panel.new_browser_tab")
	app:frames(5)
	expect(calls(app)).to_contain("run:stop")
	-- Back on it: off.
	app:click('#panel-strip >> role:tab "Page at example.com"')
	app:frames(5)
	expect(on(app)).to_be(false)
end)

test("annotation flow: Escape before any pick leaves the keyboard where it was", function()
	local app, nonce = annotating()
	local before = app:state("chat_focus") or 0
	app:set_state("test_web_message", { kind = "zeron.annotation", nonce = nonce, event = "state", state = "cancelled" })
	app:command("test.web_message")
	app:frames(3)
	expect(on(app)).to_be(false)
	expect(app:state("chat_focus") or 0).to_be(before)
end)

test("annotation flow: picks are numbered in order, chip and mark, and close up when the middle one leaves", function()
	local app, nonce = annotating()
	for n = 1, 3 do
		pick(app, nonce, 10 + n)
	end
	-- Numbered as made: each mark as its chip.
	for n = 1, 3 do
		expect(calls(app)).to_contain(`run:label({10 + n}, {n})`)
	end
	local cargo = app:state("chat.composer.cargo") or {}
	expect(cargo[2].index).to_be(2)
	-- The middle one taken off: 1, 2, no gap; the page drops mark 2 and
	-- makes 3 the new 2.
	app:set_state("test_remove_cargo", cargo[2].id)
	app:command("test.remove_cargo")
	app:frames(3)
	cargo = app:state("chat.composer.cargo") or {}
	expect(#cargo).to_be(2)
	expect(cargo[1].index).to_be(1)
	expect(cargo[2].index).to_be(2)
	expect(cargo[2].annotation.index).to_be(2)
	expect(cargo[2].element).to_be("div.card13")
	expect(calls(app)).to_contain("run:sync[[1,1],[3,2]]")
end)

test("annotation flow: a chip is the transcript's anatomy: its number and page, its words under, its card", function()
	local app = mount("cargo_fixture.luau")
	app:resize(900, 400)
	app:frames(5)
	app:set_state("test_cargo", {
		{
			id = "a1", kind = "annotation", name = "Export report", element = "button#export.btn.primary", index = 1,
			annotation = {
				url = "https://www.example.com/dashboard/revenue",
				element = "button#export.btn.primary",
				rect = { 680, 16, 101, 31 },
				html = '<button class="btn primary" id="export">\n  Export report\n</button>',
			},
		},
	})
	app:frames(10)
	-- Line one: the number its mark wears, then the page (no scheme, no
	-- www.); line two: what the element says.
	expect(app:has('"1"')).to_be(true)
	expect(app:has('"example.com/dashboard/revenue"')).to_be(true)
	expect(app:has('"Export report"')).to_be(true)
	expect(app:has('"https://www.example.com/dashboard/revenue"')).to_be(false)
	-- A note typed for it stands in for its words.
	local was = (app:state("test_cargo") :: any)[1]
	local one = table.clone(was)
	one.note = "make this the primary action"
	app:set_state("test_cargo", { one })
	app:frames(5)
	expect(app:has('"make this the primary action"')).to_be(true)
	app:hover('"example.com/dashboard/revenue"')
	expect(eventually(app, function(): boolean
		return app:has('"101 × 31"')
	end, 3000)).to_be(true)
	expect(app:has('"button"')).to_be(true)
	expect(app:has('"#export.btn.primary"')).to_be(true)
end)
