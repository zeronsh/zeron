--!strict
local zeron = require("./zeron")("imgui-developer-console")

local window = zeron.imgui.open_window {
	id = "console",
	title = "Developer Console",
	size = { width = 640, height = 420 },
}

local command = ""
local log: { string } = { "developer console ready" }

local function push(line: string)
	table.insert(log, line)
	while #log > 20 do
		table.remove(log, 1)
	end
end

window:on_render(function(ui)
	ui.text("Developer Console")
	ui.text_colored(`{#log} of 20 lines`, "#8a8a8a")
	ui.separator()
	for _, line in log do
		ui.text(line)
	end
	ui.separator()
	command = ui.input_text("command", command, { hint = "type a command" })
	ui.same_line()
	if ui.button("Run") and command ~= "" then
		push(`echo {command}`)
		command = ""
	end
end)