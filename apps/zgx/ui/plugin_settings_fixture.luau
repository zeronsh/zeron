-- Plugins' settings pages in the main window (screens/plugins/settings):
-- the shell (ui/shell_fixture.luau, as it is) with screens/plugins loaded,
-- one plugin made up here (Greeter, plugins-dev/settings-demo's manifest
-- as the host publishes it), and the host played here: what a page sets is
-- kept in `stored` (the plugin's `.settings.json`, which outlives it being
-- turned off) and published back with the defaults filled in, as
-- src/plugins/settings.rs does. What tests/plugin_settings.test.luau drives.

local node = require("./shell_fixture")
require("@app/screens/plugins")
local psettings = require("@app/screens/plugins/settings")
local pstate = require("@app/screens/plugins/state")
local permissions = require("@app/screens/plugins/permissions")

-- Greeter's settings, as the host checks and fills them in.
local function declared(words_default: string, every_default: number): { any }
	return {
		{ id = "show", kind = "switch", section = "Greeting", label = "Show the greeting", description = "A word of welcome in the title bar.", default = true },
		{ id = "words", kind = "text", section = "Greeting", label = "Greeting", description = "What it says.", placeholder = "Hello", default = words_default },
		{
			id = "tone",
			kind = "choice",
			style = "segmented",
			section = "Greeting",
			label = "Tone",
			description = "How it says it.",
			options = { { value = "plain", label = "Plain" }, { value = "excited", label = "Excited" } },
			default = "plain",
		},
		{
			id = "place",
			kind = "choice",
			style = "select",
			section = "More",
			label = "Where it shows",
			description = "Its place in the title bar.",
			options = {
				{ value = "end", label = "At the end", description = "After the title bar's buttons." },
				{ value = "start", label = "At the start", description = "Before the title bar's buttons." },
			},
			default = "end",
		},
		{ id = "every", kind = "number", section = "More", label = "Greet again every", description = "How often it says hello again.", min = 1, max = 120, unit = "minutes", default = every_default },
		{ id = "keys", kind = "shortcut", section = "More", label = "Wave shortcut", description = "Keys that wave back.", default = "ctrl+alt+g" },
		{ id = "album", kind = "folder", section = "More", label = "Postcards folder", description = "Where it keeps its postcards.", default = "" },
	}
end

-- (Greeter's manifest also declares a Screen Recording row: it isn't one
-- of Zeron's own, so the host leaves it out, as here.)
local model = {
	on = false,
	-- Greeter as someone else's, signed by no one Zeron knows.
	unverified = false,
	settings = declared("Hello", 30),
	-- What's been set (only that): kept while the plugin is off.
	stored = {} :: { [string]: any },
	-- What it reports of its settings (held while it's on).
	status = {} :: { [string]: any },
	-- Snapper: one of Zeron's own, with permission rows.
	snapper = false,
	-- Parallax scenes: Zeron's own plugin that draws a new chat's
	-- background (it replaces #new-chat-background), with settings of its
	-- own. (Load the real one too, ZERON_ZGX_DEV_PLUGINS=plugins-builtin/
	-- parallax-scenes, and its scene draws in the new chat.)
	scenes = false,
	scenes_stored = {} :: { [string]: any },
	-- Backdrops: someone else's plugin that offers a wallpaper too.
	backdrops = false,
}

-- Parallax scenes' settings (plugins-builtin/parallax-scenes/zeron-plugin.json's).
local SCENES_SETTINGS = {
	{ id = "wallpaper", kind = "wallpaper", section = "Wallpaper" },
	{ id = "scene", kind = "choice", style = "select", section = "Scene", label = "Scene", description = "What a new chat opens on.", default = "violet-dusk", options = { { value = "shuffle", label = "Shuffle", description = "Another scene each time a new chat opens." }, { value = "violet-dusk", label = "Dusk" }, { value = "aurora", label = "Aurora" }, { value = "blossom", label = "Blossom" }, { value = "lighthouse", label = "Lighthouse" }, { value = "canyon", label = "Canyon" }, { value = "lotus", label = "Lotus" }, { value = "icebergs", label = "Icebergs" }, { value = "wheat", label = "Wheat" }, { value = "volcano", label = "Volcano" }, { value = "terraces", label = "Terraces" }, { value = "surf", label = "Surf" }, { value = "saturnine", label = "Saturnine" }, { value = "rooftops", label = "Rooftops" }, { value = "station", label = "Station" } } },
	{ id = "parallax", kind = "choice", style = "segmented", section = "Motion", label = "Parallax", description = "How far the layers move with the pointer.", default = "normal", options = { { value = "off", label = "Off", description = "The scene holds still." }, { value = "subtle", label = "Subtle" }, { value = "normal", label = "Normal" }, { value = "deep", label = "Deep" } } },
}

-- The permissions as macOS has them, and as the host last read them.
local granted = { capture = false, text = true }
local function read_permissions()
	ui.shared("appshots.state", nil :: any):set({
		capture = if granted.capture then "ready" else "required",
		text = if granted.text then "ready" else "required",
	})
end
read_permissions()

local calls = ui.state("plugin_settings_calls", {} :: { string })
local function note(what: string)
	local list = table.clone(calls:peek())
	table.insert(list, what)
	calls:set(list)
end

local function publish()
	local values: { [string]: any } = {}
	for _, s in model.settings do
		local v = model.stored[s.id]
		values[s.id] = if v == nil then s.default else v
	end
	pstate.now:set({
		state = "ready",
		dirs = { plugins = "/tmp/plugins", templates = "/tmp/templates" },
		list = {
			{
				id = "settings-demo",
				name = "Greeter",
				version = "1.0.0",
				description = "A greeting in the title bar.",
				author = if model.unverified then { kind = "unknown" } else { kind = "you" },
				status = if model.on then "on" else "off",
				permissions = { "ui.add" },
				allowed = { "ui.add" },
				ask = {},
				hash = "greeter-1",
				own = not model.unverified,
			},
			if model.scenes
				then {
					id = "parallax-scenes",
					name = "Parallax scenes",
					version = "1.0.0",
					description = "Zeron's member-card scenes behind a new chat, each layer moving with the pointer.",
					author = { kind = "zeron" },
					status = "on",
					permissions = { "ui.change" },
					allowed = { "ui.change" },
					ask = {},
					ui = { change = { "#new-chat-background" } },
					hash = "parallax-scenes-1",
				}
				else nil,
			if model.backdrops
				then {
					id = "backdrops",
					name = "Backdrops",
					version = "1.0.0",
					author = { kind = "you" },
					status = "on",
					permissions = { "ui.change" },
					allowed = { "ui.change" },
					ask = {},
					ui = { change = { "#new-chat-background" } },
					hash = "backdrops-1",
				}
				else nil,
		} :: { any },
	})
	local list: { any } = {}
	if model.on then
		table.insert(list, { plugin = "settings-demo", name = "Greeter", settings = model.settings, values = values, status = model.status })
	end
	if model.snapper then
		table.insert(list, {
			plugin = "snapper",
			name = "Snapper",
			description = "Set up once, one permission at a time.",
			settings = {
				{ id = "enabled", kind = "switch", section = "Capture", label = "Capture windows", default = false },
				{ id = "screen-recording", kind = "permission", permission = "screen-recording", section = "Permissions" },
				{ id = "accessibility", kind = "permission", permission = "accessibility", section = "Permissions" },
			},
			values = { enabled = false },
			status = {},
		})
	end
	if model.scenes then
		local scene_values: { [string]: any } = {}
		for _, s in SCENES_SETTINGS do
			if s.kind ~= "wallpaper" then
				local v = model.scenes_stored[s.id]
				scene_values[s.id] = if v == nil then s.default else v
			end
		end
		table.insert(list, {
			plugin = "parallax-scenes",
			name = "Parallax scenes",
			description = "Zeron's member-card scenes behind a new chat. Use it as the wallpaper here; Zeron's effect is drawn over the scene.",
			settings = SCENES_SETTINGS,
			values = scene_values,
			status = {},
		})
		-- (What the plugin itself reads, when it's loaded.)
		ui.shared("plugin.parallax-scenes.settings", {} :: any):set(scene_values)
	end
	if model.backdrops then
		table.insert(list, {
			plugin = "backdrops",
			name = "Backdrops",
			settings = { { id = "wallpaper", kind = "wallpaper" } },
			values = {},
			status = {},
		})
	end
	psettings.list:set(list)
end

-- The host, played.
local stub = psettings :: any
stub.set = function(plugin: string, id: string, value: any)
	note(`set:{plugin}:{id}:{tostring(value)}`)
	if plugin == "parallax-scenes" then
		model.scenes_stored[id] = value
		publish()
		return
	end
	model.stored[id] = value
	publish()
end
stub.choose_folder = function(plugin: string, id: string, _title: string): any
	note(`folder:{plugin}:{id}`)
	model.stored[id] = "/Users/me/Postcards"
	publish()
	return nil
end
local state_stub = pstate :: any
state_stub.set_off = function(id: string, off: boolean): any
	note(`set_off:{id}:{tostring(off)}`)
	model.on = not off
	publish()
	return nil
end
local perm_stub = permissions :: any
perm_stub.request = function(permission: string)
	note(`request:{permission}`)
end
perm_stub.refresh = function()
	note("refresh")
	read_permissions()
end
state_stub.check_updates = function(): any
	return nil
end

ui.command {
	id = "greeter_unverified",
	title = "Greeter is someone else's, unverified",
	run = function()
		model.unverified = true
		publish()
	end,
}
ui.command {
	id = "plugin_on",
	title = "Greeter is turned on",
	run = function()
		model.on = true
		publish()
	end,
}
ui.command {
	id = "plugin_off",
	title = "Greeter is turned off",
	run = function()
		model.on = false
		publish()
	end,
}
ui.command {
	id = "defaults_changed",
	title = "Greeter's update changes its defaults",
	run = function()
		model.settings = declared("Howdy", 45)
		publish()
	end,
}

ui.command {
	id = "status_set",
	title = "Greeter reports its shortcut didn't register",
	run = function()
		model.status = { keys = { badge = "Set up", tone = "warning", text = "Another app has these keys. Choose others." } }
		publish()
	end,
}
ui.command {
	id = "status_clear",
	title = "Greeter clears its shortcut's status",
	run = function()
		model.status = {}
		publish()
	end,
}
ui.command {
	id = "snapper_on",
	title = "Zeron's own Snapper is on",
	run = function()
		model.snapper = true
		publish()
	end,
}
ui.command {
	id = "scenes_on",
	title = "Parallax scenes, which draws a new chat's background, is on",
	run = function()
		model.scenes = true
		publish()
	end,
}
ui.command {
	id = "scenes_off",
	title = "Parallax scenes is turned off",
	run = function()
		model.scenes = false
		publish()
	end,
}
ui.command {
	id = "backdrops_on",
	title = "Backdrops, another plugin with a wallpaper, is on",
	run = function()
		model.backdrops = true
		publish()
	end,
}
ui.command {
	id = "grant_capture",
	title = "Screen Recording granted in System Settings (Zeron hasn't looked yet)",
	run = function()
		granted.capture = true
	end,
}

publish()

return node
