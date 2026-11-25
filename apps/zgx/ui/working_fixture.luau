-- Working lines as a chat page and an orchestrator's crew show them, for
-- tests/working_line.test.luau and a layout measure (zgx-run --inspect):
-- one in a transcript-like column, and `working_fixture_crew` crew cards in
-- a grid, each with its own line under a few rows of text.
local ds = require("@ds")
ds.setup()
local Working = require("@ds/working")
local since = ui.state("working_fixture_since", ui.now() / 1000 - 65)
local crew = ui.state("working_fixture_crew", 24)
-- Rows of text under each card, as a crew card's progress and reports put
-- there: what a line's change laid out with it, when it wasn't a box of its own.
local filler = ui.state("working_fixture_filler", 0)

local function card(i: number): Node
	return ui.column {
		class = "fixture-card",
		w = 220,
		p = "$space.2",
		gap = "$space.1",
		ui.text { `Thread {i}`, weight = "$font.weight.semibold" },
		ui.column {
			w_full = true,
			Working { since = function(): number
				return since:get() - i
			end, seed = i, size = "sm" },
		},
		ui.text { "Reading the sidebar's shape and the rows it walks", wrap = true, color = "$text.secondary" },
		ui.text { "12 of 30 steps", color = "$text.tertiary" },
		ui.each(ui.computed(function(): { number }
			local out = {}
			for k = 1, filler:get() do
				out[k] = k
			end
			return out
		end), function(k: number): Node
			return ui.row { gap = "$space.1", ui.text { `step {k}`, color = "$text.tertiary" }, ui.text { "done", color = "$text.secondary" } }
		end, { w_full = true }),
	}
end

return ui.column {
	w_full = true,
	h_full = true,
	p = "$space.4",
	gap = "$space.4",
	ui.column {
		id = "fixture-transcript",
		w_full = true,
		gap = "$space.2",
		ui.text { "A reply in progress, its words above the line.", wrap = true },
		ui.row {
			w_full = true,
			Working { since = function(): number
				return since:get()
			end, seed = 3 },
		},
	},
	ui.each(ui.computed(function(): { number }
		local out = {}
		for i = 1, crew:get() do
			out[i] = i
		end
		return out
	end), card, { id = "fixture-crew", direction = "row", wrap = true, gap = "$space.2", w_full = true }),
}
