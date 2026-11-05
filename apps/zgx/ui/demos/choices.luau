-- Select, slider and number stepper.

local Select = require("@ds/select")
local Slider = require("@ds/slider")
local NumberStepper = require("@ds/stepper")
local kit = require("@demos/kit")

local appearance = ui.state("ch_appearance", "system")
local editor = ui.state("ch_editor", nil :: string?)
local merge = ui.state("ch_merge", "squash")
local zone = ui.state("ch_zone", "lon")
local long_one = ui.state("ch_long", "long")
local volume = ui.state("ch_volume", 60)
local scale = ui.state("ch_scale", 100)
local context = ui.state("ch_context", 200)
local agents = ui.state("ch_agents", 4)
local timeout = ui.state("ch_timeout", 30)
local opacity = ui.state("ch_opacity", 0.9)

return {
	id = "choices",
	title = "Select, slider, stepper",
	about = "A select opens its options over itself, the chosen one under the pointer. The slider shares the switch's groove and knob. The stepper types, steps, or takes the arrows.",
	demo = function()
		return kit.page(
			kit.block(
				"SELECT",
				ui.row {
					gap = "$space.6",
					items = "start",
					flex_wrap = true,
					Select {
						value = appearance,
						label = "Appearance",
						options = {
							{ value = "system", label = "Match system", icon = "monitor" },
							{ value = "light", label = "Light", icon = "global" },
							{ value = "dark", label = "Dark", icon = "moon" },
						},
					},
					Select {
						value = editor,
						label = "Open files in",
						placeholder = "Choose an editor",
						hint = "Used by Open in editor.",
						options = {
							{ value = "zed", label = "Zed" },
							{ value = "cursor", label = "Cursor" },
							{ value = "code", label = "VS Code" },
							{ value = "xcode", label = "Xcode", disabled = true },
						},
					},
					Select {
						value = merge,
						label = "Merge with",
						w = 280,
						options = {
							{ value = "merge", label = "Merge commit", description = "Every commit, joined by a merge." },
							{ value = "squash", label = "Squash and merge", description = "One commit on the base." },
							{ value = "rebase", label = "Rebase and merge", description = "Each commit replayed on top." },
						},
					},
				},
				kit.row(
					Select {
						value = zone,
						name = "Time zone",
						w = 200,
						options = {
							{ value = "hon", label = "Honolulu" }, { value = "anc", label = "Anchorage" },
							{ value = "van", label = "Vancouver" }, { value = "den", label = "Denver" },
							{ value = "chi", label = "Chicago" }, { value = "nyc", label = "New York" },
							{ value = "sao", label = "São Paulo" }, { value = "lon", label = "London" },
							{ value = "par", label = "Paris" }, { value = "cai", label = "Cairo" },
							{ value = "dub", label = "Dubai" }, { value = "del", label = "Delhi" },
							{ value = "sgp", label = "Singapore" }, { value = "tok", label = "Tokyo" },
							{ value = "syd", label = "Sydney" }, { value = "akl", label = "Auckland" },
						},
					},
					Select {
						value = long_one,
						name = "Long",
						w = 160,
						options = {
							{ value = "long", label = "A name far longer than its button" },
							{ value = "short", label = "Short" },
						},
					},
					Select { value = appearance, size = "lg", w = 200, name = "Large", options = { { value = "system", label = "Match system", icon = "monitor" }, { value = "light", label = "Light", icon = "global" }, { value = "dark", label = "Dark", icon = "moon" } } }
				),
				kit.row(
					Select { value = appearance, size = "sm", w = 160, name = "Small", options = { { value = "system", label = "Match system" }, { value = "light", label = "Light" }, { value = "dark", label = "Dark" } } },
					Select { value = appearance, disabled = true, w = 180, name = "Disabled", options = { { value = "system", label = "Match system" } } },
					Select {
						value = editor,
						w = 200,
						name = "Wrong",
						placeholder = "Required",
						options = { { value = "zed", label = "Zed" } },
						error = function()
							return if editor:get() == nil then "Choose one." else nil
						end,
					}
				)
			),
			kit.block(
				"SLIDER",
				ui.column {
					gap = "$space.5",
					w = 360,
					Slider {
						value = volume,
						min = 0,
						max = 100,
						step = 1,
						label = "Volume",
						w_full = true,
						format = function(v)
							return `{math.floor(v)}%`
						end,
					},
					Slider {
						value = scale,
						min = 75,
						max = 150,
						label = "Interface scale",
						w_full = true,
						marks = {
							{ value = 75, label = "75%" },
							{ value = 100, label = "100%" },
							{ value = 125, label = "125%" },
							{ value = 150, label = "150%" },
						},
					},
					Slider {
						value = opacity,
						min = 0.5,
						max = 1,
						step = 0.01,
						label = "Window opacity",
						w_full = true,
						disabled = true,
						format = function(v)
							return `{math.floor(v * 100 + 0.5)}%`
						end,
					},
				}
			),
			kit.block(
				"NUMBER STEPPER",
				kit.row(
					NumberStepper { value = agents, min = 1, max = 8, label = "Parallel agents" },
					NumberStepper { value = timeout, min = 5, max = 300, step = 5, unit = "s", label = "Timeout" },
					NumberStepper { value = context, min = 0, max = 1000, step = 50, unit = "k", size = "sm", name = "Context" },
					NumberStepper { value = agents, disabled = true, name = "Disabled", label = "Locked" }
				)
			)
		)
	end,
}
