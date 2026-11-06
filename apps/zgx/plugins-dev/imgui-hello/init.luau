--!strict
-- The smallest plugin that opens a window of its own: one real operating
-- system window titled "Hello from Luau", drawn from Luau every frame the
-- window is shown, with two lines of text and a button. Released, the
-- button prints exactly one line, "imgui-hello: clicked N", and the
-- "clicks" line under it shows N.
--
-- Letting it run (the app's gallery screen needs no engine or sign-in):
--   ZERON_ZGX_DEV_PLUGINS=<this folder> ZERON_ZGX_PLUGIN_LOG=1 \
--     target/debug/zeron-zgx.exe gallery
--
-- The window asks for 520x300. The frame line counts the render callbacks
-- the host has run: it advances while the window is shown and stops while
-- it is hidden or minimized (no callback runs then).

local zeron = require("./zeron")("imgui-hello")

local window = zeron.imgui.open_window {
	id = "hello",
	title = "Hello from Luau",
	size = { width = 520, height = 300 },
}

local frames = 0
local clicks = 0

window:on_render(function(ui)
	frames += 1
	ui.text("Hello from Luau! This window is drawn by a plugin.")
	ui.text(`frame {frames}`)
	if ui.button("Click me") then
		clicks += 1
		print(`imgui-hello: clicked {clicks}`)
	end
	ui.text(`clicks {clicks}`)
end)
