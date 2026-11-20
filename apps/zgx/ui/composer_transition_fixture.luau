-- Controller-only fixture: no engine, editor or unrelated animations.
local ds = require("@ds")
local Layout = require("@ds/composer_layout")
local Motion = require("@ds/composer_motion")
ds.setup()
local width = ui.state("fixture_transition_width", 660)
local height = ui.state("fixture_transition_height", 22)
local shown = ui.state("fixture_transition_shown", true)
local snap = ui.state("fixture_transition_snap", 0)
local ready = ui.state("fixture_transition_ready", true)
local revision = ui.state("fixture_transition_revision", 0)
local settlement = ui.computed(function()
	return { revision = revision:get(), status = if ready:get() then "ready" else "pending" }
end)
local report = ui.state("fixture_transition_report", {} :: any)
local target = ui.computed(function()
	return Layout.calculate({
		width = width:get(),
		sample = { unwrapped_width = 40, wrapped_height = height:get(), line_height = 22 },
		trailing_width = 64, control_corner = 25,
		header_height = 0, header_gap = 12,
		expanded = false, cargo = false, asking = false, dictating = false,
		multiline = height:get() > 22,
	}, false)
end)
local last: any = nil
ui.command {
	id = "fixture.transition.inspect",
	title = "Inspect the last controller, even after unmount",
	run = function()
		if last then
			report:set({ frame = last.frame:peek(), origin = last.origin:peek(),
				destination = last.destination:peek(), progress = last.progress:peek(), active = last.active:peek(), settling = last.settling:peek() })
		end
	end,
}
local Controller = ui.component(function(): Node
	last = Motion.use(target, snap, settlement)
	local own = last
	return ui.div { w = width, h = function() return own.frame:get().height end }
end)
return ui.show(function() return shown:get() end, Controller {})
