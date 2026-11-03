-- Settings → Templates, mounted on its own with templates made up here (no
-- engine in a test host): the page tests/templates_in_place.test.luau
-- drives. What the host would publish is set here; what the page asks is
-- stubbed to change this data as the host would, and `ui.command`s change it
-- as an agent's work would (a template made, one breaking).

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local state = require("@app/screens/templates/state")
local page = require("@app/screens/templates/page")
-- The command palette too, whose Templates rows use them (shell/palette).
-- (No window chrome to restyle in a test host, as shell_fixture says.)
local actions = require("@app/screens/onboarding/actions");
(actions :: any).apply_scheme = function() end
local Palette = require("@app/screens/shell/palette")
local view = require("@app/screens/shell/view")

local palettes = require("@ds/palettes")

-- An official template's look (src/plugins/looks.rs): its built-in theme's
-- palettes, light and dark.
local function official(light: string, dark: string): state.Look
	return { stamp = `{light}|{dark}`, light = { palette = palettes.find(light) }, dark = { palette = palettes.find(dark) } }
end
-- A template's own theme files (the hot pink one's, both; the night one's,
-- only dark).
local function theme(scheme: string, accent: string, window: string, text: string): state.Scheme
	return { theme = { type = scheme, extends = `zgx-{scheme}`, tokens = { accent = accent, text = { primary = text }, zeron = { window = window } } } }
end

local model: { state.Template } = {
	{ id = "zeron", name = "Zeron", description = "Zeron's own interface.", author = "Zeron", version = "0.2.131", zeron_version = "0.2.131", builtin = true, active = true, status = "ok" },
	-- Zeron's official templates (its built-in themes, kept by each launch:
	-- src/plugins/theme_templates.rs), each with its look, light then dark.
	{ id = "theme-tokyo-night", name = "Tokyo Night", description = "Tokyo Night's colours on Zeron's interface.", author = "Zeron", version = "0.2.131", zeron_version = "0.2.131", builtin = true, active = false, status = "ok", theme = "tokyo-night", look = official("tokyo-night-light", "tokyo-night") },
	{ id = "theme-github", name = "GitHub", description = "GitHub's colours on Zeron's interface.", author = "Zeron", version = "0.2.131", zeron_version = "0.2.131", builtin = true, active = false, status = "ok", theme = "github", look = official("github-light", "github-dark") },
	{ id = "hot-pink", name = "Hot Pink", description = "Everything pink.", author = "you", version = "1.0.0", builtin = false, active = false, status = "ok", look = { stamp = "p1", light = theme("light", "#d6247a", "#fff0f6", "#3d0a22"), dark = theme("dark", "#ff3e9a", "#1a0010", "#ffe0ef") } },
	{ id = "night", name = "Night", description = "A darker dark.", author = "you", version = "1.0.0", builtin = false, active = false, status = "ok", look = { stamp = "n1", dark = theme("dark", "#22ccaa", "#000000", "#e8fff9") } },
	{ id = "airy", name = "Airy", description = "More room between things.", author = "you", version = "1.0.0", zeron_version = "0.2.131", builtin = false, active = false, status = "ok" },
}

local function find(id: string): state.Template
	for _, t in model do
		if t.id == id then
			return t
		end
	end
	error(`no template {id} in the fixture`)
end

-- Packages waiting for you (someone else's, brought in).
local waiting: { state.Pending } = {}

local function publish()
	local list: { state.Template } = {}
	local active = "zeron"
	for _, t in model do
		table.insert(list, table.clone(t))
		if t.active then
			active = t.id
		end
	end
	state.now:set({ state = "ready", active = active, dir = "/tmp/templates", list = list, pending = table.clone(waiting) })
end

local said: { string } = {}
local calls = ui.state("calls", "")
local function note(what: string)
	table.insert(said, what)
	calls:set(table.concat(said, ","))
end
local stub = state :: any
stub.use = function(id: string)
	note(`use:{id}`)
	for _, t in model do
		t.active = t.id == id
	end
	publish()
end
stub.duplicate = function(id: string)
	note(`duplicate:{id}`)
	local from = find(id)
	table.insert(model, { id = `{id}-copy`, name = `{from.name} copy`, author = "you", builtin = false, active = false, status = "ok" })
	publish()
end
stub.delete = function(id: string)
	note(`delete:{id}`)
	for i, t in model do
		if t.id == id then
			table.remove(model, i)
			break
		end
	end
	publish()
end
stub.create = function(name: string)
	note(`create:{name}`)
	local id = string.lower((string.gsub(name, "%W+", "-")))
	table.insert(model, { id = id, name = name, author = "you", version = "1.0.0", builtin = false, active = false, status = "ok" })
	publish()
end
stub.open_folder = function(id: string)
	note(`folder:{id}`)
end
stub.review = function(name: string)
	note(`review:{name}`)
end

ui.command {
	id = "made",
	title = "An agent makes a template",
	run = function()
		table.insert(model, { id = "dusk", name = "Dusk", author = "you", builtin = false, active = false, status = "ok" })
		publish()
	end,
}
ui.command {
	id = "updated",
	title = "Zeron updates past Airy",
	run = function()
		local t = find("airy")
		t.status = "out_of_date"
		t.zeron_version = "0.2.120"
		publish()
	end,
}
ui.command {
	id = "broke",
	title = "Airy's root stops running",
	run = function()
		local t = find("airy")
		t.status = "broken"
		t.error = "app.luau:3: attempt to call a nil value"
		publish()
	end,
}

ui.command {
	id = "imported",
	title = "Ada's template is installed",
	run = function()
		table.insert(model, {
			id = "ledger",
			name = "Ledger",
			version = "2.0.0",
			builtin = false,
			active = false,
			status = "ok",
			imported = { key = "k-ada", author = { kind = "verified", username = "ada" } },
		})
		table.insert(model, {
			id = "mystery",
			name = "Mystery",
			builtin = false,
			active = false,
			status = "ok",
			imported = { key = "k-who", author = { kind = "unknown" } },
		})
		table.insert(model, {
			id = "mine-again",
			name = "Mine again",
			builtin = false,
			active = false,
			status = "ok",
			imported = { key = "k-me", author = { kind = "you" } },
		})
		publish()
	end,
}
ui.command {
	id = "waiting",
	title = "Someone's template waits for you",
	run = function()
		table.insert(waiting, { pending = "ledger-1700000000", name = "Ledger", source = "ledger.zeron-template" })
		publish()
	end,
}
ui.command {
	id = "answered",
	title = "The waiting template is answered",
	run = function()
		table.clear(waiting)
		publish()
	end,
}

publish()

ui.command {
	id = "palette",
	title = "Open the command palette",
	run = function()
		view.palette:set(true)
	end,
}

-- The themes registered (the previews' among them), for the tests.
local theme_names = ui.state("theme_names", "")
ui.command { id = "theme_names", title = "List the themes", run = function() theme_names:set(table.concat(ui.theme.names(), ",")) end }

-- Light, for pictures of it (src/audit_shots.rs: `!theme_light`).
ui.command { id = "theme_light", title = "Light theme", run = function() ui.theme.follow_system(false); ui.theme.current:set("zeron-light") end }

return ui.div {
	w_full = true,
	h_full = true,
	ui.scroll {
		w_full = true,
		h_full = true,
		ui.column {
			w_full = true,
			px = "$space.6",
			pt = "$space.4",
			pb = "$space.8",
			items = "center",
			page.View(),
		},
	},
	Palette(),
}
