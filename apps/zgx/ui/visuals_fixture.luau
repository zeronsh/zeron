-- Agent visuals in a reply, run by the real host (src/visuals.rs, the
-- shipping app's crates/visual): every node type, from the shipping app's
-- demo visuals (crates/client/src/demo/visuals.rs) and the visual crate's
-- own examples, and two that fail. For src/visuals_tests.rs and a look in
-- a window: `zeron-zgx fixture=visuals_fixture.luau --inspect`. Commands
-- switch the theme. `only=<name>` (an arg) shows one of them alone.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local Visual = require("@ds/visual")
local Markdown = require("@ds/markdown")

local VISUALS: { { name: string, title: string, source: string } } = {
	{
		name = "dashboard",
		title = "API latency",
		source = [[
local s = ui.state { range = "7d", table = false, window = 3 }
local days = { "Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun" }
local p95 = { ["7d"] = { 182, 175, 190, 168, 160, 171, 158 }, ["30d"] = { 210, 198, 190, 181, 176, 170, 162 } }
local p50 = { ["7d"] = { 92, 88, 95, 84, 80, 86, 79 }, ["30d"] = { 104, 99, 95, 91, 88, 85, 81 } }
return function()
  local rows = {}
  for i, day in ipairs(days) do
    rows[i] = { day, p95[s.range][i], p50[s.range][i] }
  end
  return ui.column { gap = 14,
    ui.row { gap = 16,
      ui.stat { value = "158 ms", label = "p95 latency", delta = "−8%", tone = "success" },
      ui.stat { value = "99.95%", label = "uptime" },
      ui.stat { value = "4.2k", label = "req / min" },
    },
    ui.tabs { options = { "7d", "30d" }, value = s.range, on_change = function(v) s.range = v end },
    ui.chart { type = "area", height = 160, x = days, unit = "ms",
      series = { { name = "p95", values = p95[s.range] }, { name = "p50", values = p50[s.range] } } },
    ui.slider { label = "Smoothing window", value = s.window, min = 1, max = 7, step = 1,
      on_change = function(v) s.window = v end },
    ui.row { gap = 12, align = "center",
      ui.toggle { label = "Show table", value = s.table, on_change = function(v) s.table = v end },
      ui.button { label = "Reset", on_click = function() s.window = 3 end },
    },
    s.table and ui.table { columns = { "Day", { label = "p95", align = "end" }, { label = "p50", align = "end" } }, rows = rows }
      or ui.space(0),
  }
end
]],
	},
	{
		name = "charts",
		title = "Build times",
		source = [[
local months = { "Jan", "Feb", "Mar", "Apr", "May", "Jun" }
local s = ui.state { kind = "Grouped" }
return function()
  local bars = ui.chart { type = "bar", height = 180, x = months, unit = "s", stacked = s.kind == "Stacked",
    horizontal = s.kind == "Across",
    series = {
      { name = "Compile", values = { 42, 38, 51, 47, 33, 29 } },
      { name = "Link", values = { 12, 14, 11, 16, 9, 8 } },
      { name = "Test", values = { 20, 22, 19, 25, 18, 21 }, color = ui.mix("chart3", "warning", 0.4) },
    } }
  return ui.column { gap = 16,
    ui.text { "Where the build's time goes", size = "lg", weight = "semibold" },
    ui.tabs { options = { "Grouped", "Stacked", "Across" }, value = s.kind, on_change = function(v) s.kind = v end },
    bars,
    ui.rule(),
    ui.grid { columns = 2, gap = 20,
      ui.column { gap = 6,
        ui.text { "Cache hit rate", size = "sm", color = "muted" },
        ui.chart { type = "line", height = 140, x = months, unit = "%", min = 0, max = 100, legend = false,
          series = { { name = "hits", values = { 61, 64, nil, 72, 78, 83 } } } },
      },
      ui.column { gap = 6,
        ui.text { "Size against time", size = "sm", color = "muted" },
        ui.chart { type = "scatter", height = 140, unit = "s", x_unit = "MB", legend = false,
          series = { { name = "crates", points = { { 2, 4 }, { 5, 9 }, { 8, 13 }, { 3, 6 }, { 11, 21 }, { 7, 10 } } } } },
      },
    },
  }
end
]],
	},
	{
		name = "words",
		title = "Release notes",
		source = [[
local picked = 0
local items = { "Faster cold starts", "Smaller downloads", "Quieter fans" }
return function()
  local rows = {}
  for i, name in ipairs(items) do
    rows[i] = ui.row { hover = true, gap = 10, align = "center", key = name,
      on_click = function() picked = i end,
      ui.text { picked == i and "●" or "○", color = picked == i and "accent" or "faint" },
      ui.text { name, grow = true },
      ui.text { "v2." .. i, size = "xs", color = "muted", mono = true },
    }
  end
  return ui.column { gap = 10,
    ui.text { size = "xl", weight = "semibold", "What changed in ", ui.span { "2.4", color = "accent" } },
    ui.text { color = "muted", "Three things, each ", ui.span { "measured", weight = "semibold", color = "text" },
      " on a cold machine; ", ui.span { "cargo build", mono = true }, " included." },
    ui.column { gap = 2, rows },
    ui.space(4),
    ui.row { gap = 16, align = "center",
      ui.link { label = "Read the full notes", url = "https://zeron.sh" },
      ui.text { picked > 0 and ("Picked " .. items[picked]) or "Pick one", size = "sm", color = "faint", italic = true },
    },
  }
end
]],
	},
	{
		name = "canvas",
		title = "Orbiting dots",
		source = [[
return ui.column { gap = 6,
  ui.text { "Drag across it", size = "sm", color = "muted" },
  ui.canvas { height = 220, animate = true, draw = function(g, w, h, t)
    local p = g.pointer
    local cx = p and p.x or w / 2 + math.cos(t * 0.7) * w / 4
    local cy = p and p.y or h / 2 + math.sin(t * 0.9) * h / 4
    for i = 1, 36 do
      local a = t * 0.8 + i * (math.pi * 2 / 36)
      local r = 34 + 22 * math.sin(t * 2 + i * 0.7)
      g:circle(cx + math.cos(a) * r, cy + math.sin(a) * r, 3.5, { fill = "chart" .. (i % 6 + 1) })
    end
    g:circle(cx, cy, 18, { fill = ui.color("accent", 0.18) })
    g:line(0, h - 1, w, h - 1, { color = ui.mix("border", "accent", 0.5), width = 1 })
    g:circle(cx, cy, 6, { fill = "accent" })
    g:text(8, 8, "t = " .. string.format("%.1f", t), { size = "xs", color = "faint", align = "start", baseline = "top" })
  end },
}
]],
	},
	{
		name = "scene",
		title = "Materials",
		source = [[
return ui.scene { height = 260, orbit = true, spin = 0.25, draw = function(s)
  s:box { at = { 0, -0.05, 0 }, size = { 5, 0.1, 3.5 }, color = "muted" }
  s:box { at = { -1.4, 0.5, 0 }, size = 1, color = "chart1", roughness = 0.35 }
  s:sphere { at = { 0, 0.6, 0.4 }, radius = 0.6, color = "chart2", metal = 1, roughness = 0.25 }
  s:box { at = { 1.4, 0.6, 0 }, size = { 0.9, 1.2, 0.9 }, color = "chart3", glass = true, dispersion = 1 }
  s:sphere { at = { 1.4, 1.6, -1 }, radius = 0.2, color = "chart4", glow = 4 }
end }
]],
	},
	{
		name = "shader",
		title = "Light",
		source = [[
return ui.shader { height = 220, bloom = 1.2, code = [=[
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
  vec2 uv = (fragCoord - 0.5 * iResolution.xy) / iResolution.y;
  vec2 m = iMouse.x > 0.0 ? (iMouse.xy - 0.5 * iResolution.xy) / iResolution.y : vec2(0.0);
  vec3 col = vec3(0.0);
  for (int i = 0; i < 6; i++) {
    float fi = float(i);
    vec2 c = m + 0.28 * vec2(cos(iTime * 0.9 + fi), sin(iTime * 1.2 + fi * 2.0));
    float d = length(uv - c);
    col += 0.010 / (d * d + 0.002) * (0.5 + 0.5 * cos(6.2831 * (fi / 6.0 + vec3(0.0, 0.33, 0.67))));
  }
  fragColor = vec4(col, 1.0);
}
]=] }
]],
	},
	{
		name = "broken",
		title = "Runaway",
		source = [[
local n = 0
while true do n += 1 end
return function() return ui.text { tostring(n) } end
]],
	},
	{
		name = "handler",
		title = "Divide",
		source = [[
local d = 2
return function()
  return ui.row { gap = 12, align = "center",
    ui.text { "100 / " .. d .. " = " .. (100 / d) },
    ui.button { label = "Divide by nil", on_click = function() d = nil .. "x" end },
  }
end
]],
	},
}

ui.command { id = "theme_light", title = "Light theme", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-light")
end }
ui.command { id = "theme_dark", title = "Dark theme", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-dark")
end }

local only = nil :: string?
for _, arg in ui.args or {} do
	local name = string.match(arg, "^only=(.+)$")
	if name then
		only = name
	end
end

local blocks: { Node } = {}
for _, v in VISUALS do
	if only == nil or only == v.name then
		table.insert(blocks, ui.column {
			id = `visual-{v.name}`,
			w_full = true,
			gap = "$space.3",
			Markdown { text = `**{v.title}**: what the agent showed.` },
			Visual { id = `v:fixture-{v.name}`, title = v.title, source = v.source },
		})
	end
end

return ui.scroll {
	id = "visuals",
	w_full = true,
	h_full = true,
	bg = "$background",
	ui.column {
		id = "reply",
		selectable = true,
		w_full = true,
		max_w = "$zeron.prose.measure",
		p = "$space.6",
		gap = "$space.7",
		blocks,
	},
}
