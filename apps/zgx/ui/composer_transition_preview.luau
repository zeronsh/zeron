-- Real composer for visual transition checks. All input is fixture-local;
-- the development host's engine/account updates cannot replace this draft.
-- It leaves the build's own settings as it found them: Reduce motion is
-- persisted (`motion_reduce`), so it is only changed by its button here and
-- put back as the preview goes, as is the theme it shows in.
local ds = require("@ds")
local motion = require("@ds/motion")
ds.setup()
local reduce_was = motion.reduce:peek()
local theme_was = ui.theme.current:peek()
ui.theme.current:set("zeron-dark")

local draft = ui.state("fixture_preview_draft", "short")
local focus = ui.state("fixture_preview_focus", 0)
local cargo = ui.state("fixture_preview_cargo", {} :: { any })
local function lines(n: number): string
	local out = {}
	for i = 1, n do table.insert(out, `Line {i} for composer transition checks`) end
	return table.concat(out, "\n")
end
local function set_draft(text: string)
	ui.batch(function()
		draft:set(text)
		focus:set(focus:peek() + 1)
	end)
end

local Preview = ui.component(function(): Node
	ui.on_cleanup(function()
		if motion.reduce:peek() ~= reduce_was then
			motion.reduce:set(reduce_was)
		end
		ui.theme.current:set(theme_was)
	end)
	return ui.column {
	w_full = true, h_full = true, p = "$space.4", gap = "$space.4",
	bg = "$background",
	ui.row {
		gap = "$space.2",
		ds.Button { "Short", on_click = function() set_draft("short") end },
		ds.Button { "10 lines", on_click = function() set_draft(lines(10)) end },
		ds.Button { "80 lines", on_click = function() set_draft(lines(80)) end },
		ds.Button { "Header", on_click = function()
			cargo:set(if #cargo:peek() == 0 then { { id = "f", kind = "file", name = "notes.md" } } else {})
			focus:set(focus:peek() + 1)
		end },
		-- (scripts/composer_frames.py clicks these by place: keep the labels.)
		ds.Button { "Reduce motion", on_click = function() motion.reduce:set(not motion.reduce:peek()) end },
		ds.Button { "Wrap", on_click = function() set_draft(string.rep("W", 60)) end },
	},
	ui.div { grow = 1 },
	ui.div {
		w = 660,
		ds.Composer {
			value = draft, cargo = cargo, focus = focus,
			model = ui.state("fixture_preview_model", "sonnet" :: string?),
			effort = ui.state("fixture_preview_effort", "medium" :: string?),
			harnesses = { { id = "claude", label = "Claude", mark = "claude" } },
			models = { { value = "sonnet", label = "Sonnet", harness = "claude" } },
		},
	},
}
end)

return Preview {}
