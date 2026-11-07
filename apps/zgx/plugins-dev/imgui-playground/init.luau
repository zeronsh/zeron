--!strict
local zeron = require("./zeron")("imgui-playground")

local window = zeron.imgui.open_window {
	id = "playground",
	title = "Widget Playground",
	size = { width = 560, height = 420 },
}

local frames = 0
local watch = true
local mode = "left"
local speed = 0.35
local steps = 4
local fruit = "apple"
local row = 1
local log: { string } = {}

local function push(line: string)
	table.insert(log, line)
	while #log > 30 do
		table.remove(log, 1)
	end
end

for index = 1, 10 do
	push(`line {index}`)
end

window:on_render(function(ui)
	frames += 1
	push(`frame {frames} mode={mode} fruit={fruit}`)

	ui.text("Widget Playground")
	watch = ui.checkbox("watch", watch)

	if ui.radio_button("left", mode == "left") then
		mode = "left"
	end
	ui.same_line()
	if ui.radio_button("right", mode == "right") then
		mode = "right"
	end

	speed = ui.slider_float("speed", speed, { min = 0, max = 1 })
	steps = ui.drag_int("steps", steps, { min = 0, max = 32 })
	fruit = ui.combo("fruit", fruit, { "apple", "banana", "cherry" })

	for index = 1, 3 do
		if ui.selectable(`row {index}`, row == index) then
			row = index
		end
	end

	ui.progress_bar(row / 3)
	local shown = string.format("%.2f", speed)
	ui.bullet_text(`row {row} of 3, speed {shown}, steps {steps}`)

	ui.begin_child("log", { width = 520, height = 96, border = true })
	for index = 1, #log do
		ui.text(log[index])
	end
	ui.end_child()

	ui.text(`watch {watch} · mode {mode} · speed {shown} · steps {steps} · fruit {fruit} · row {row}`)
end)