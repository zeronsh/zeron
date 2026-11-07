--!strict
local zeron = require("./zeron")("imgui-custom-chrome")

local window = zeron.imgui.open_window {
	id = "chrome",
	title = "Custom Chrome",
	size = { width = 420, height = 340 },
	titlebar = "hidden",
}

local dragging = false
local was_hovered = false
local last_click = 0.0

local function run(name: string, action: () -> ())
	local ok, why = pcall(action)
	if ok then
		print(`imgui-custom-chrome: {name}`)
	else
		print(`imgui-custom-chrome: {name} refused: {why}`)
	end
end

window:on_render(function(ui)
	ui.text(if window:is_maximized() then "Custom Chrome (maximized)" else "Custom Chrome")

	local bar = ui.invisible_button {
		id = "drag",
		size = { width = 404, height = 32 },
	}
	local hovered = ui.is_item_hovered()
	local active = ui.is_item_active()
	if hovered ~= bar.hovered or active ~= bar.active then
		print(`imgui-custom-chrome: disagreement hovered={hovered}/{bar.hovered} active={active}/{bar.active}`)
	end
	if hovered ~= was_hovered then
		was_hovered = hovered
		local dx, dy = ui.mouse_delta()
		print(`imgui-custom-chrome: bar hovered={hovered} dx={dx} dy={dy}`)
	end

	if bar.active then
		if not dragging then
			local dx, dy = ui.mouse_delta()
			print(`imgui-custom-chrome: drag dx={dx} dy={dy}`)
			run("drag", function()
				window:begin_drag()
			end)
			dragging = true
		end
	else
		dragging = false
	end

	if bar.clicked then
		local now = os.clock()
		if now - last_click < 0.4 then
			last_click = 0.0
			run("double click", function()
				if window:is_maximized() then
					window:restore()
				else
					window:maximize()
				end
			end)
		else
			last_click = now
		end
	end

	if ui.button("minimize") then
		run("minimize", function()
			window:minimize()
		end)
	end
	if ui.button("maximize") then
		run("maximize", function()
			if window:is_maximized() then
				window:restore()
			else
				window:maximize()
			end
		end)
	end
	if ui.button("close") then
		run("close", function()
			window:close()
		end)
	end
	if ui.button("resize") then
		run("resize", function()
			window:begin_resize("se")
		end)
	end
end)