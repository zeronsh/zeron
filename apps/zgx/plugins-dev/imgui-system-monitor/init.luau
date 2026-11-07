--!strict
local zeron = require("./zeron")("imgui-system-monitor")

local window = zeron.imgui.open_window {
	id = "monitor",
	title = "System Monitor",
	size = { width = 520, height = 380 },
}

local frames = 0
local paused = false

window:on_render(function(ui)
	if not paused then
		frames += 1
	end
	local cpu = (frames * 7 % 61 + 30) / 100
	local memory = (frames * 3 % 41 + 45) / 100

	ui.text("System Monitor")
	ui.text_colored(`cpu {math.floor(cpu * 100)}%`, "#8fd694")
	ui.progress_bar(cpu)
	ui.text_colored(`memory {math.floor(memory * 100)}%`, "#ffb454")
	ui.progress_bar(memory)
	ui.separator()
	if ui.small_button(if paused then "Resume" else "Pause") then
		paused = not paused
	end
end)

window:on_close(function()
	print(`imgui-system-monitor: closed after {frames} frames`)
end)