-- A machine's head, one line, the same wherever a machine is shown (the
-- Map's three views, the Control room's Machines): its platform's mark,
-- its name, its platform quietly after it and "this one" as a small tag on
-- this device's, then its CPU as a slim fixed meter and a rolling figure
-- right after. Never wraps: with little room the platform word gives way
-- (truncated) first; nothing moves to another line or to the far edge.
--
--   MachineHead {
--     device = function() return { id, name, platform, cloud?, online? } end,
--     here = function() return true end,      -- this device
--     load = function() return orchestration.loads[id]? end,
--     class = "…",                             -- optional
--   }
--
--   MachineHead.H         its height
--   MachineHead.icon(d)   a device's mark
--   MachineHead.word(d)   its platform, as a word

local Icon = require("@self/icon")
local locale = require("@self/locale")
local Counter = require("@self/orch_counter")
local Block = require("@self/orch_block")
local Spark = require("@self/orch_spark")

local M = {}

M.H = 28
-- The meter's width.
local METER_W = 32

function M.icon(d: any): Icon.Name
	local p = d and d.platform
	if d and (d.cloud == true or p == "cloud") then
		return "cloud"
	elseif p == "macos" then
		return "laptop"
	elseif p == "ios" or p == "android" then
		return "smartphone"
	end
	return "monitor"
end

function M.word(d: any): string
	local p = d and d.platform
	if p == "macos" then
		return "macOS"
	elseif p == "windows" then
		return "Windows"
	elseif p == "linux" then
		return "Linux"
	elseif p == "ios" then
		return "iOS"
	elseif p == "android" then
		return "Android"
	end
	return p or ""
end

export type Props = {
	device: () -> any,
	here: () -> boolean,
	load: () -> any,
	class: string?,
}

function M.MachineHead(props: Props): Node
	local function online(): boolean
		local d = props.device()
		return d ~= nil and (d.online ~= false or props.here())
	end
	local function cpu(): number?
		local l = props.load()
		return if l and l.cpu then math.clamp(l.cpu, 0, 1) * 100 else nil
	end
	local small = Block.baseline("$font.size.sm", "$font.size.xs")
	return ui.row {
		class = if props.class then `orch-machine-head {props.class}` else "orch-machine-head",
		label = function(): string
			return (props.device() or {}).name or ""
		end,
		w_full = true,
		h = M.H,
		gap = "$space.2",
		items = "center",
		overflow = "hidden",
		ui.div {
			class = "orch-machine-head-icon",
			color = "$text.secondary",
			shrink = 0,
			Icon {
				name = function(): Icon.Name
					return M.icon(props.device())
				end,
				size = 14,
			},
		},
		ui.text {
			function(): string
				return (props.device() or {}).name or ""
			end,
			class = "orch-machine-head-name",
			shrink = 0,
			max_w = 220,
			truncate = true,
			text_size = "$font.size.sm",
			weight = "$font.weight.medium",
			color = "$text.primary",
		},
		-- Its platform: the first to give way.
		ui.text {
			function(): string
				return M.word(props.device())
			end,
			class = "orch-machine-head-platform",
			shrink = 1,
			min_w = 0,
			truncate = true,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
			y = small,
		},
		ui.show(props.here, ui.row {
			class = "orch-machine-here",
			shrink = 0,
			h = 18,
			px = 6,
			items = "center",
			rounded = "$radius.sm",
			bg = "$hover",
			ui.text { locale.t("ds.orch.map.here"), text_size = "$font.size.xs", color = "$text.secondary" },
		}),
		-- Its load: a slim meter and the figure, right after.
		ui.switch(function(): string
			return if online() then "on" else "off"
		end, {
			on = ui.row {
				class = "orch-machine-head-load",
				shrink = 0,
				pl = "$space.2",
				gap = "$space.1",
				items = "center",
				ui.canvas {
					class = "orch-machine-head-meter",
					w = METER_W,
					h = 3,
					shrink = 0,
					label = function(): string
						local v = cpu()
						return if v then locale.s("ds.orch.map.cpu", { percent = math.floor(v + 0.5) }) else ""
					end,
					draw = function(c: any, w: number, h: number)
						local v = cpu()
						c:rect(0, 0, w, h, Spark.fade("$text.primary", 0.14), h / 2)
						if v and v > 0 then
							c:rect(0, 0, math.max(h, w * v / 100), h, if v > 85 then "$warning" else "$text.secondary", h / 2)
						end
					end,
				},
				ui.div {
					class = "orch-machine-head-figure",
					shrink = 0,
					overflow = "hidden",
					y = small,
					-- (As many cells as it has figures: no blank ones before it.)
					Counter {
						value = cpu,
						suffix = "%",
						anchor = "start",
						max = 0,
						text_size = "$font.size.xs",
						color = "$text.secondary",
					},
				},
			},
			off = ui.text { locale.t("ds.orch.map.offline"), shrink = 0, pl = "$space.2", text_size = "$font.size.xs", color = "$text.tertiary", y = small },
		}),
		ui.div { grow = 1 },
	}
end

return M
