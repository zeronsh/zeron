-- The working ring where it can and can't be seen, a case at a time
-- (`ring_case`): what idle CPU is measured against. "shown" in the open;
-- "collapsed" in a zero-width clipped column (the sidebar folded);
-- "scrolled" in a list, scrolled out of view; "outside" past the window's
-- edge; "none" with no ring at all.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local status = require("@ds/status")

local case = ui.shared("ring_case", "shown")

-- Two loops put in phase (`status.in_phase`) a moment apart, as rings that
-- show at different times are: where each stands, and the time between.
local phase = ui.shared("ring_phase", {})
do
	local period = 4800
	local first, later = { currentTime = 0 }, { currentTime = 0 }
	status.in_phase(first, period)
	local made = ui.now()
	local spin = 0
	for i = 1, 200000 do
		spin += i
	end
	status.in_phase(later, period)
	phase:set({ period = period, first = first.currentTime, later = later.currentTime, between = ui.now() - made, now = ui.now() % period })
end

local function ring(): Node
	return status.Spinner { size = 14 }
end

return ui.column {
	w_full = true,
	h_full = true,
	p = "$space.6",
	gap = "$space.3",
	ui.text { id = "case", case },
	ui.show(function(): boolean
		return case:get() == "shown"
	end, ui.div { id = "shown", ring() }),
	ui.show(function(): boolean
		return case:get() == "collapsed"
	end, ui.div { id = "collapsed", w = 0, h = 120, overflow = "hidden", ui.column { w = 240, gap = 4, ring() } }),
	ui.show(function(): boolean
		return case:get() == "scrolled"
	end, ui.scroll {
		id = "scrolled",
		w = 200,
		h = 100,
		-- (Scrolled down past the ring at its top by whoever measures.)
		ui.column { gap = 4, ring(), ui.div { h = 3000 } },
	}),
	ui.show(function(): boolean
		return case:get() == "outside"
	end, ui.div { id = "outside", absolute = true, left = 5000, top = 5000, ring() }),
}
