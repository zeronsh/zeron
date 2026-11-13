-- The gallery's live pieces for orchestration: segmented cells
-- (ds/orch_cells), the sparkline (ds/orch_spark), the rolling counter
-- (ds/orch_counter) and minimum presence (ds/orch_hold), each moving on
-- its own clock so what they do shows.

local ds = require("@ds")
local status = require("@ds/status")
local Cells = require("@ds/orch_cells")
local Spark = require("@ds/orch_spark")
local Counter = require("@ds/orch_counter")
local hold = require("@ds/orch_hold")
local Block = require("@ds/orch_block")

local function caption(s: string): Node
	return ds.Text { s, style = "label", tone = "tertiary" }
end

-- A made-up CPU line: a wander with a few bursts, one sample every 5 s.
local function walk(n: number, now: number): { Spark.Sample }
	local out = {}
	local v = 0.3
	for i = 1, n do
		v = math.clamp(v + (math.sin(i * 1.7) + math.sin(i * 0.43) * 0.6) * 0.07, 0.04, 0.96)
		table.insert(out, { at = now - (n - i) * 5, v = v })
	end
	return out
end

return {
	id = "orch-live",
	title = "Live pieces",
	about = "A block (a widget's own unit: title, live figures, menu), segmented cells (done, failed, the current one breathing), a live sparkline with event marks, a rolling counter, and the 300 ms minimum presence a status gets.",
	demo = function(): Node
		local t = ui.use_state(0)
		ui.every(1200, function()
			t:set(t:peek() + 1)
		end)
		local start = os.time()
		local samples = ui.computed(function(): { Spark.Sample }
			local n = 40 + t:get()
			return walk(n, start + t:get() * 5)
		end)
		-- A status flipping faster than it can be seen, and the same held.
		local fast = ui.use_state("working" :: status.Kind)
		ui.every(120, function()
			fast:set(if fast:peek() == "working" then "done" else "working")
		end)
		local held = hold.value(function(): status.Kind
			return fast:get()
		end)
		return ui.column {
			w_full = true,
			max_w = 560,
			gap = "$space.6",
			Block.Block {
				title = "Block",
				figures = ui.row {
					gap = "$space.2",
					items = "center",
					Counter { value = function(): number
						return 3 + t:get() % 4
					end, text_size = "$font.size.xs", color = "$text.secondary" },
					Block.figure("1 blocked", "failed"),
				},
				menu = ds.IconButton { icon = "more-horizontal", label = "Menu", size = "sm", tone = "quiet" },
				ds.Text { "Its body.", style = "body", tone = "secondary" },
			},
			ui.column {
				w_full = true,
				gap = "$space.2",
				caption("Cells"),
				Cells { done = 3, total = 8 },
				Cells { done = function(): number
					return t:get() % 10
				end, total = 9 },
				Cells { states = function(): { Cells.State }
					return { "done", "failed", "done", "done", "failed", "done", "current", "pending", "pending", "pending" }
				end },
				Cells { done = 18, failed = 3, total = 30, current = false, fill = true },
			},
			ui.column {
				w_full = true,
				gap = "$space.2",
				caption("Sparkline"),
				Spark.Spark {
					samples = function(): { Spark.Sample }
						return samples:get()
					end,
					events = function(): { Spark.Event }
						local now = start + t:get() * 5
						return { { at = now - 170, tone = "working" }, { at = now - 95, tone = "danger" }, { at = now - 40, tone = "success" } }
					end,
					label = "CPU",
					value_node = Counter {
						value = function(): number?
							local s = samples:get()
							return if #s > 0 then s[#s].v * 100 else nil
						end,
						suffix = "%",
						text_size = "$font.size.xs",
						color = "$text.secondary",
						anchor = "end",
					},
					caption = "5 min",
					caption_end = "now",
				},
			},
			ui.column {
				w_full = true,
				gap = "$space.2",
				caption("Counter"),
				ui.row {
					gap = "$space.5",
					items = "baseline",
					Counter { value = function(): number
						return 128 + t:get() * 7
					end, text_size = "$font.size.xl", weight = "$font.weight.medium" },
					Counter { value = function(): number
						return (t:get() * 1.3) % 60
					end, format = function(n: number): string
						return string.format("%.1f", n)
					end, suffix = "s", color = "$text.secondary" },
				},
			},
			ui.column {
				w_full = true,
				gap = "$space.2",
				caption("Minimum presence (flipping every 120 ms; held, every 300)"),
				ui.row {
					gap = "$space.5",
					items = "center",
					status.StatusIcon { kind = function(): status.Kind
						return fast:get()
					end },
					status.StatusIcon { kind = held },
				},
			},
		}
	end,
}
