--!strict
-- The validation contract's failing-open fixture
-- (apps/zgx/plugins-dev/imgui-win-probe-broken-open).
--
-- `VAL-LIFE-012`: the window is created, its first render callback raises
-- `probe open failure`, and the host must report that and take the window
-- down without leaving a native window behind or bringing Zeron down. On the
-- installed route the plugin is withdrawn too; the dev route has no plugin
-- manager, so the plugin itself stays loaded. The tool exists so the row can
-- check the installed route stops answering once the failure is reported.
--
-- Run it (the app must survive the failure):
--   ZERON_ZGX_DEV_PLUGINS=<this folder> ZERON_ZGX_PLUGIN_LOG=1 \
--     target/debug/zeron-zgx.exe gallery

local zeron = require("./zeron")("imgui-win-probe-broken-open")

local function say(what: string)
	print(`probe-broken: {what}`)
end

local window = zeron.imgui.open_window {
	id = "broken",
	title = "Probe Broken Open",
	size = { width = 420, height = 300 },
}

local drawn = false

window:on_render(function(view: any)
	-- The row's words: the failure is inside the window's *first* on_render.
	-- A host that cannot draw a window once must take it away (and report the
	-- plugin broken) rather than leave an empty window; raising every frame
	-- would also flood the log, and would hide that first-frame rule behind
	-- the repeated-failure one.
	if not drawn then
		drawn = true
		error("probe open failure")
	end
	-- Only reached by a host that swallowed the first frame's error: say so,
	-- rather than paint as if the window were fine.
	view.text("probe: the first frame raised probe open failure")
end)

window:on_close(function(closed: string)
	say(`closed:closecall {closed}`)
end)

zeron.tool {
	name = "probe_status",
	run = function(): any
		return {
			plugin = "imgui-win-probe-broken-open",
			version = "1",
			window = "broken",
			expected = "probe open failure",
		}
	end,
}

zeron.menu {
	kind = "session",
	id = "probe-broken-open",
	label = "Probe: report the open failure",
	run = function()
		say("still loaded")
	end,
}

say("opened broken: the first render raises probe open failure")
