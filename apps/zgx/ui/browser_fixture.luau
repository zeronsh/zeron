-- The session page's right panel mounted on its own with a real web page
-- (no stubs), for looking at the browser tab and its annotation mode
-- without the main window and its engine: `ZGX_CHECK=0 zgx run
-- ui/browser_fixture.luau [url]`. Its tabs are kept under a session of its
-- own; the composer's cargo (screens/chat/shared) is real, so picks show
-- in the chips strip under the panel here.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()

local Panel = require("@app/screens/panel")
local state = require("@app/screens/panel/state")
local shared = require("@app/screens/chat/shared")
local Cargo = require("@ds/composer_cargo")

-- The session the panel is for: a made-up one, or (`fixture.seed`) a real
-- chat in a project on the engine this fixture runs against, so the start
-- page's dev servers come from the engine's own discovery.
local session = ui.state("fixture_session", { id = "browser-fixture", cwd = "/" } :: any)
local key = state.key(session:peek(), nil) :: string
-- Opened once with a browser tab (on the address given, else the start
-- page); a reload finds it.
if #state.of(key).tabs == 0 then
	local url = ui.args[1]
	state.open(key, "browser", { url = url })
end

-- Picking without a pointer (from devtools): `fixture.pick` picks what
-- `fixture_pick` names in the tab's page, once annotation mode has run its
-- picker (⌘⇧I, or the bar's cursor).
local api = require("@app/screens/panel/browser_api")
local wanted = ui.state("fixture_pick", "h1")
ui.command {
	id = "fixture.pick",
	title = "Pick in Page",
	run = function()
		local tab = state.of(key).tabs[1]
		if tab then
			local web = api.web(`panel-browser-{tab.id}`)
			local sel = string.gsub(wanted:peek(), "'", "\\'")
			web:run(`window.__zeronAnnotator && window.__zeronAnnotator.pick('{sel}');`)
		end
	end,
}

-- And a look into the page: `fixture.eval` runs `fixture_js` there, its
-- answer (or error) in `fixture_result`.
local js = ui.state("fixture_js", "typeof window.__zeronAnnotator")
local result = ui.state("fixture_result", "" :: any)
ui.command {
	id = "fixture.eval",
	title = "Eval in Page",
	run = function()
		local tab = state.of(key).tabs[1]
		if tab then
			local web = api.web(`panel-browser-{tab.id}`)
			web:eval(js:peek(), function(value: any, err: string?)
				result:set(if err then `error: {err}` else value)
			end)
		end
	end,
}

-- The theme: `fixture.dark` and `fixture.light` set the scripts' (the
-- root's `theme`, as the shell's) and the window's own chrome.
local dark = ui.state("fixture_dark", true)
ui.effect(function()
	ui.invoke("appearance", dark:get())
end)
ui.command {
	id = "fixture.dark",
	title = "Dark",
	run = function()
		dark:set(true)
	end,
}
ui.command {
	id = "fixture.light",
	title = "Light",
	run = function()
		dark:set(false)
	end,
}

-- A project on this device at `fixture_seed_path`, and a chat in it, on
-- the engine; the panel then shows that chat (`fixture_seeded` says which,
-- or what went wrong).
local seed_path = ui.state("fixture_seed_path", "")
local seeded = ui.state("fixture_seeded", "")
ui.command {
	id = "fixture.seed",
	title = "Seed a Project Chat",
	run = function()
		local path = seed_path:peek()
		local here: string? = nil
		for _, d in ((ui.shared("sessions", nil :: any):peek() or {}).devices or {}) do
			if d.here then
				here = d.id
			end
		end
		if path == "" or type(here) ~= "string" then
			seeded:set(`not yet: path {path}, device {tostring(here)}`)
			return
		end
		ui.call("projects.create", { device = here, path = path }):after(function(space: any)
			ui.call("sessions.create", { project = tostring(space), device = here, cwd = path, title = "Preview scratch" }):after(function(chat: any)
				session:set({ id = tostring(chat), cwd = path, device = here })
				seeded:set(tostring(chat))
			end, function(e: any)
				seeded:set(`chat failed: {tostring(e)}`)
			end)
		end, function(e: any)
			seeded:set(`project failed: {tostring(e)}`)
		end)
	end,
}

-- The app's ⌘F, as the shell asks the panel.
ui.command {
	id = "fixture.find",
	title = "Find in Page",
	run = function()
		Panel.find_in_browser()
	end,
}

-- The window shown again on the space in use (a capture rig's: a window
-- left on another space can't be captured from this one).
ui.command {
	id = "fixture.reshow",
	title = "Show Again",
	run = function()
		ui.window.visible:set(false)
		ui.after(300, function()
			ui.window.visible:set(true)
		end)
	end,
}

-- The prompt the picks would go out as (`fixture.token` writes it to
-- `fixture_token`), for checking against the engine's parser.
local Annotation = require("@ds/annotation")
local token = ui.state("fixture_token", "")
local token_parts = ui.state("fixture_token_parts", 0)
local parts = {}
for i = 1, 40 do
	parts[i] = ui.state(`fixture_token_{i}`, "")
end
ui.command {
	id = "fixture.token",
	title = "Prompt Text",
	run = function()
		local picks = {}
		for _, item in shared.cargo:peek() do
			if item.kind == "annotation" and item.annotation then
				table.insert(picks, item)
			end
		end
		local text = Annotation.with_annotations("Make these read better.", picks)
		token:set(text)
		-- (And in pieces devtools can read whole: `fixture_token_parts` of
		-- them, `fixture_token_1`, `fixture_token_2`, …)
		local n = 0
		for i = 1, #text, 800 do
			n += 1
			parts[n]:set(string.sub(text, i, i + 799))
		end
		token_parts:set(n)
	end,
}

return ui.column {
	w_full = true,
	h_full = true,
	theme = function(): string
		return if dark:get() then "zeron-dark" else "zeron-light"
	end,
	bg = "$background",
	on_focus = function()
		Panel.focus_moved()
	end,
	Panel.Panel {
		session = session,
		project = nil,
		left = function(): number
			return 1000
		end,
		w_full = true,
		grow = 1,
		min_h = 0,
	},
	-- What the picks carry, as the composer's strip shows them.
	ui.column {
		id = "fixture-cargo",
		w_full = true,
		shrink = 0,
		px = "$space.3",
		py = "$space.2",
		border_sides = { 1, 0, 0, 0 },
		border_color = "$border.subtle",
		Cargo.Strip {
			items = shared.cargo,
			on_remove = function(id: string)
				shared.remove_cargo(id)
			end,
		},
	},
}
