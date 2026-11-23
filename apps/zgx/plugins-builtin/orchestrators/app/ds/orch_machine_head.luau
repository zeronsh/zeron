-- A machine's head, one line, the same wherever a machine is shown (the
-- Map's three views, the Control room's Machines): its platform's mark
-- (this device's with a dot on its corner), its name, its platform quietly
-- after it, then its CPU as a ring (as the composer's context ring) and a
-- rolling figure: right after, or at the line's end in a block whose CPU
-- line runs under it. Never wraps: with little room the platform word
-- gives way (truncated) first.
--
--   MachineHead {
--     device = function() return { id, name, platform, cloud?, online? } end,
--     here = function() return true end,      -- this device
--     load = function() return orchestration.loads[id]? end,
--     load_at_end = true,                     -- optional: over a CPU line
--     class = "…",                             -- optional
--   }
--
--   MachineHead.H         its height
--   MachineHead.icon(d)   a device's mark
--   MachineHead.word(d)   its platform, as a word

local Icon = require("@ds/icon")
local locale = require("@ds/locale")
local Counter = require("@self/orch_counter")
local Block = require("@self/orch_block")
local ContextUsage = require("@ds/context_usage")
local Tooltip = require("@ds/tooltip")
local Ref = require("@self/orch_ref")

local M = {}

M.H = 28

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
	-- Its load at the line's end (a block whose CPU line is under it).
	load_at_end: boolean?,
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
	-- Dragged: a reference to the machine (ds/orch_ref), for Capture.
	local drag = Ref.source(function(): any
		return Ref.device(props.device())
	end)
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
		on_drag = drag.on_drag,
		drag_preview = drag.drag_preview,
		drag_preview_anchor = drag.drag_preview_anchor,
		drag_preview_offset = drag.drag_preview_offset,
		ui.div {
			class = "orch-machine-head-icon",
			relative = true,
			color = "$text.secondary",
			shrink = 0,
			Icon {
				name = function(): Icon.Name
					return M.icon(props.device())
				end,
				size = 14,
			},
			-- This device: a dot on its mark's corner, ringed in the card's
			-- colour so it reads on any.
			ui.show(props.here, ui.div {
				class = "orch-machine-head-here",
				absolute = true,
				right = -3,
				top = -3,
				w = 8,
				h = 8,
				rounded = "$radius.full",
				bg = "$zeron.tone.info.ink",
				border = 1.5,
				border_color = "$zeron.settings.card",
				label = locale.t("ds.orch.map.this_device"),
				tooltip = Tooltip { locale.t("ds.orch.map.this_device") },
			}),
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
		-- (In a block over its CPU line: its load at the line's right end.)
		if props.load_at_end then ui.div { grow = 1 } else nil,
		-- Its load: a ring (as the composer's context ring) and the figure.
		ui.switch(function(): string
			return if online() then "on" else "off"
		end, {
			on = ui.row {
				class = "orch-machine-head-load",
				shrink = 0,
				pl = "$space.2",
				gap = "$space.1",
				items = "center",
				ui.div {
					class = "orch-machine-head-meter",
					shrink = 0,
					label = function(): string
						local v = cpu()
						return if v then locale.s("ds.orch.map.cpu", { percent = math.floor(v + 0.5) }) else ""
					end,
					ContextUsage.Ring {
						fraction = function(): number
							return (cpu() or 0) / 100
						end,
					},
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
		if props.load_at_end then nil else ui.div { grow = 1 },
	}
end

return M
