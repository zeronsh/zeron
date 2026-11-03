-- Your theme library (screens/settings/theme_library) on its own, for
-- tests/theme_library_in_place.test.luau: the Light and Dark theme choices
-- as Settings → Appearance has them, the import section, and what the
-- window wears now (`#applied`, the palette's text ink, or "none"). No host
-- in a test: its import, rename and remove change the library here as
-- src/themes.rs does (its Rust tests import the real files).

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local palettes = require("@ds/palettes")
local library = require("@app/screens/settings/theme_library")

ui.theme.follow_system(false)
ui.theme.current:set("zeron-dark")
ds.look.palette.dark:set("zeron-dark")
ds.look.palette.light:set("zeron-light")

-- What src/themes.rs makes of tests/fixtures/themes/Harbor Night.json.
local HARBOR = {
	id = "custom-harbor-night-harbor-night",
	name = "Harbor Night",
	kind = "dark",
	entry = "custom-harbor-night",
	background = "#14181f",
	raised = "#1d232c",
	text = "#d8dee9",
	muted = "#a3acb9",
	faint = "#7c8594",
	accent = "#7aa2f7",
	danger = "#f7768e",
	warning = "#e0af68",
	success = "#9ece6a",
	syntax = { "#5c6773", "#c792ea", "#c3e88d", "#f78c6c", "#ffcb6b", "#82aaff", "#d8dee9", "#d8dee9", "#d8dee9", "#d8dee9", "#d8dee9", "#f7768e" },
}

local model = { list = {} :: { any }, entries = {} :: { any } }
local function publish()
	palettes.library():set({ state = "ready", list = table.clone(model.list), entries = table.clone(model.entries) })
end

-- A promise settled at once, as a host's does later.
local function settled(ok: boolean, value: any): any
	return {
		after = function(_self: any, done: (any) -> (), failed: ((any) -> ())?)
			if ok then
				done(value)
			elseif failed then
				failed(value)
			end
		end,
	}
end

local said = ui.state("calls", "")
local function note(what: string)
	said:set(if said:peek() == "" then what else `{said:peek()},{what}`)
end

library.host.import = function(path: string): any
	note(`import:{path}`)
	if string.find(path, "Broken", 1, true) then
		return settled(false, { message = `could not parse JSONC theme {path}` })
	end
	table.insert(model.list, table.clone(HARBOR))
	table.insert(model.entries, { id = HARBOR.entry, name = HARBOR.name, status = { ok = true }, variants = { HARBOR.id } })
	publish()
	return settled(true, { entry = HARBOR.entry, variants = { HARBOR.id }, skipped = {} })
end
library.host.remove = function(id: string)
	note(`remove:{id}`)
	model.list, model.entries = {}, {}
	publish()
end
library.host.rename = function(id: string, name: string): any
	note(`rename:{id}:{name}`)
	for _, e in model.entries do
		e.name = name
	end
	for _, p in model.list do
		p.name = name
	end
	publish()
	return settled(true, nil)
end

ui.command { id = "import_harbor", title = "Import Harbor Night", run = function()
	library.import("/themes/Harbor Night.json")
end }
ui.command { id = "import_broken", title = "Import a broken file", run = function()
	library.import("/themes/Broken.json")
end }

publish()

return ui.scroll {
	w_full = true,
	h_full = true,
	ui.column {
		w_full = true,
		p = "$space.6",
		gap = "$space.4",
		ds.Select { value = ds.look.palette.dark, name = "Dark theme", w = 220, options = library.options("dark") },
		ds.Select { value = ds.look.palette.light, name = "Light theme", w = 220, options = library.options("light") },
		ui.text {
			id = "applied",
			function(): string
				local t = ds.look.palette_theme()
				return if t and t["text.primary"] then t["text.primary"] else "none"
			end,
		},
		library.Section(),
	},
}
