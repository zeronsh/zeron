--!strict
-- The validation contract's consent fixture
-- (apps/zgx/plugins-dev/imgui-no-permission).
--
-- Its manifest asks for no `imgui.windows`. The plugin tries to open one
-- window anyway, is refused with a sentence naming the permission, and
-- keeps asking: `VAL-WIN-010` and the consent row add `imgui.windows` to this
-- manifest, install it, and allow it in the consent sheet, and the retried
-- call must then succeed within 120 frames without a restart or a manual
-- reload. Until then it keeps running, so a refused permission is not a
-- crash.
--
-- Run it before and after allowing it (the gallery needs no engine):
--   ZERON_ZGX_DEV_PLUGINS=<this folder> ZERON_ZGX_PLUGIN_LOG=1 \
--     target/debug/zeron-zgx.exe gallery

local zeron = require("./zeron")("imgui-no-permission")

local asked = 0
local last_refusal = ""
local window: any = nil

local function say(what: string)
	print(`probe-no-permission: {what}`)
end

local function draw(view: any)
	view.text("No Permission Probe")
	view.text(`asked {asked}`)
	if view.button("Ping") then
		say("ping")
	end
end

local attempt: () -> ()

attempt = function()
	asked += 1
	local handle: any = nil
	local ok, why = pcall(function()
		handle = zeron.imgui.open_window {
			id = "consent",
			title = "No Permission Probe",
			size = { width = 480, height = 320 },
		}
	end)
	if ok and handle ~= nil then
		window = handle
		handle:on_render(draw)
		handle:on_close(function(closed: string)
			window = nil
			say(`closed:closecall {closed}`)
		end)
		say(`opened after {asked} asks`)
		return
	end
	last_refusal = tostring(why)
	-- One line a second is enough to see the sentence without flooding the
	-- log; the ask itself is retried twice a second.
	if asked == 1 or asked % 2 == 0 then
		say(`refused: {last_refusal}`)
	end
	ui.after(500, attempt)
end

attempt()

-- So a probe can read how many asks were refused and the exact sentence,
-- without reading the log.
zeron.tool {
	name = "probe_status",
	run = function(): any
		local open_now = window ~= nil
		if window then
			local ok, value = pcall(function()
				return window:is_open()
			end)
			open_now = ok and value == true
		end
		return {
			plugin = "imgui-no-permission",
			version = "1",
			windows = if open_now then { "consent" } else {},
			open = if open_now then 1 else 0,
			asked = asked,
			last_refusal = last_refusal,
		}
	end,
}

-- A person can ask again at once instead of waiting for the timer.
zeron.menu {
	kind = "session",
	id = "probe-no-permission-again",
	label = "Probe: ask for imgui.windows again",
	run = function()
		attempt()
	end,
}
