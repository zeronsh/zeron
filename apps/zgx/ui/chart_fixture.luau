-- Charts as a visual lays them in a reply (ds/chart, the axes as the host
-- gives them), a heading over each and the reply's words under it, for
-- ds/chart's tests and pictures. `only_<name>` shows one alone; `width_<n>`
-- narrows the column; `theme_dark` and `theme_light` wear either.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local Chart = require("@ds/chart")

local only = ui.state("chart_only", "")
local width = ui.state("chart_width", 640)

local function axis(hi: number, step: number, unit: string?): any
	local ticks, labels = {}, {}
	local v = 0
	while v <= hi do
		table.insert(ticks, v)
		table.insert(labels, `{v}{unit or ""}`)
		v += step
	end
	return { lo = 0, hi = hi, ticks = ticks, labels = labels }
end

local MONTHS = { "Jan", "Feb", "Mar", "Apr", "May", "Jun" }
local CHARTS: { { name: string, title: string, data: any } } = {
	-- Wing's: one series across, as a reply comparing builds draws it.
	{
		name = "across",
		title = "Engine CPU per reply (seconds)",
		data = {
			type = "bar",
			horizontal = true,
			x = { "beta.4", "beta.5", "beta.5, 2 windows", "with fixes" },
			series = { { name = "CPU per reply", values = { 2.35, 4.1, 4.4, 0.4 } } },
			yaxis = axis(5, 1),
			fmt = { unit = "s", decimals = 2 },
		},
	},
	{
		name = "bar",
		title = "Build time",
		data = {
			type = "bar",
			x = MONTHS,
			series = { { name = "Compile", values = { 42, 38, 51, 47, 33, 29 } } },
			yaxis = axis(60, 20),
			fmt = { unit = "s" },
		},
	},
	{
		name = "grouped",
		title = "Where the build's time goes",
		data = {
			type = "bar",
			x = MONTHS,
			series = {
				{ name = "Compile", values = { 42, 38, 51, 47, 33, 29 } },
				{ name = "Link", values = { 12, 14, 11, 16, 9, 8 } },
				{ name = "Test", values = { 20, 22, 19, 25, 18, 21 } },
			},
			yaxis = axis(60, 20),
			fmt = { unit = "s" },
		},
	},
	{
		name = "stacked",
		title = "Where the build's time goes, stacked",
		data = {
			type = "bar",
			stacked = true,
			x = MONTHS,
			series = {
				{ name = "Compile", values = { 42, 38, 51, 47, 33, 29 } },
				{ name = "Link", values = { 12, 14, 11, 16, 9, 8 } },
				{ name = "Test", values = { 20, 22, 19, 25, 18, 21 } },
			},
			yaxis = axis(100, 25),
			fmt = { unit = "s" },
		},
	},
	{
		name = "line",
		title = "Cache hit rate",
		data = {
			type = "line",
			x = MONTHS,
			series = { { name = "hits", values = { 61, false, 68, 72, 78, 83 } } },
			yaxis = axis(100, 25, "%"),
			fmt = { unit = "%" },
		},
	},
	{
		name = "area",
		title = "API latency",
		data = {
			type = "area",
			x = { "Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun" },
			series = {
				{ name = "p95", values = { 182, 175, 190, 168, 160, 171, 158 } },
				{ name = "p50", values = { 92, 88, 95, 84, 80, 86, 79 } },
			},
			yaxis = axis(200, 50),
			fmt = { unit = "ms" },
		},
	},
	{
		name = "scatter",
		title = "Size against time",
		data = {
			type = "scatter",
			series = { { name = "crates", points = { 2, 4, 5, 9, 8, 13, 3, 6, 11, 21, 7, 10 } } },
			yaxis = axis(25, 5),
			xaxis = axis(12, 4),
		},
	},
	-- Five long names in a narrow column: the legend wraps at rest, and the
	-- pointer's values would wrap it once more.
	{
		name = "crowded",
		title = "Five services",
		data = {
			type = "line",
			x = MONTHS,
			series = {
				{ name = "Authentication", values = { 10, 12, 11, 14, 13, 12 } },
				{ name = "Billing service", values = { 20, 22, 21, 24, 23, 22 } },
				{ name = "Notifications", values = { 30, 32, 31, 34, 33, 32 } },
				{ name = "Search indexer", values = { 40, 42, 41, 44, 43, 42 } },
				{ name = "Thumbnails", values = { 50, 52, 51, 54, 53, 52 } },
			},
			yaxis = axis(60, 20),
			fmt = { unit = "ms", decimals = 1 },
		},
	},
}

for _, c in CHARTS do
	ui.command { id = `only_{c.name}`, title = c.name, run = function()
		only:set(c.name)
	end }
end
for _, w in { 240, 320, 360, 480, 640 } do
	ui.command { id = `width_{w}`, title = `{w}`, run = function()
		width:set(w)
	end }
end
ui.command { id = "theme_light", title = "Light", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-light")
end }
ui.command { id = "theme_dark", title = "Dark", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-dark")
end }

local kids: { Node } = {}
for _, c in CHARTS do
	table.insert(
		kids,
		ui.show(
			function(): boolean
				return only:get() == "" or only:get() == c.name
			end,
			ui.column {
				id = `chart-{c.name}`,
				w_full = true,
				gap = "$space.4",
				ui.text { c.title, literal = true, text_size = "$font.size.lg", weight = "$font.weight.semibold" },
				Chart {
					data = function(): any
						return c.data
					end,
					height = if c.data.horizontal then 150 else 180,
					label = c.title,
				},
				ui.text { id = `below-{c.name}`, "The reply goes on under the chart.", literal = true },
			}
		)
	)
end
return ui.column {
	id = "page",
	w_full = true,
	h_full = true,
	bg = "$bg",
	p = "$space.6",
	ui.column {
		w = function(): number
			return width:get()
		end,
		gap = "$space.8",
		kids,
	},
}
