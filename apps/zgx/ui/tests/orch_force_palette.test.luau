-- The force Graph's colours (ds/orch_force_palette) and marks
-- (ds/orch_force_marks), in each theme: a machine's colour never one a
-- status wears (nor grey), soft, apart from the others; a mark on it black
-- or white by the higher contrast; every mark centred on its visual bounds
-- within half a point at the size it's drawn, and sized alike. Prints
-- `FORCEPAL …` (each theme's colours).

local pal = require("@ds/orch_force_palette")
local marks = require("@ds/orch_force_marks")

local function apart(a: number, b: number): number
	local d = math.abs(a - b) % 360
	return math.min(d, 360 - d)
end

local function read(theme: string): any
	local app = mount("orch_map_fixture.luau")
	app:command(theme)
	app:frames(2)
	app:command("map_force_palette")
	app:frames(1)
	return app:state("force_palette")
end

for _, theme in { "theme_dark", "theme_light" } do
	test(`force palette ({theme}): machine colours keep clear of every status colour, none grey, soft, apart`, function()
		local p: any = read(theme)
		assert(p and p.machines, "no palette read")
		local machines: { string } = p.machines
		print(`zeron-zgx: FORCEPAL {p.theme} {table.concat(machines, " ")}`)
		assert(#machines >= 8 and #machines <= pal.COUNT, `{#machines} machine colours`)
		-- (Statuses read from the theme: there are some, and chromatic ones.)
		local hues: { number } = {}
		local inks: { string } = {}
		for t, v in p.status do
			if type(v) == "string" then
				table.insert(inks, v)
				local _, c, h = pal.hue_of(v)
				if c >= pal.GREY_C then
					table.insert(hues, h)
				end
			end
		end
		assert(#hues >= 4, `only {#hues} chromatic status colours read`)
		local names = {}
		for t in p.status do
			table.insert(names, t)
		end
		table.sort(names)
		for _, t in names do
			local v: string = p.status[t]
			local l: number, c: number, h: number = pal.hue_of(v)
			print(`zeron-zgx: FORCEPAL_STATUS {tostring(p.theme)} {t} {v} {string.format("%.2f %.3f %.0f", l, c, h)}`)
		end
		for _, m in machines do
			local l: number, c: number, h: number = pal.hue_of(m)
			print(`zeron-zgx: FORCEPAL_MACHINE {tostring(p.theme)} {m} {string.format("%.2f %.3f %.0f", l, c, h)}`)
		end
		for k, m in machines do
			local l, c, h = pal.hue_of(m)
			assert(c > pal.GREY_C, `machine {k} {m} is grey (chroma {c})`)
			assert(c <= 0.11, `machine {k} {m} too saturated (chroma {c})`)
			if p.dark then
				assert(l >= 0.78, `machine {k} {m} not light enough on dark ({l})`)
			else
				assert(l >= 0.7 and l <= 0.86, `machine {k} {m} lightness on light {l}`)
				-- (Stands off the page.)
				assert(math.abs(l - pal.hue_of(p.background)) >= 0.1, `machine {k} {m} too near the background`)
			end
			-- (1° for rounding into sRGB.)
			for _, sh in hues do
				assert(apart(h, sh) >= pal.MIN_HUE - 1, `machine {k} {m} (hue {h}) within {apart(h, sh)}° of a status hue {sh}`)
			end
			for j, o in machines do
				if j ~= k then
					local _, _, oh = pal.hue_of(o)
					assert(apart(h, oh) >= pal.MIN_APART - 1, `machines {k} and {j} only {apart(h, oh)}° apart`)
				end
			end
			-- (Never a status's own colour.)
			for _, s in inks do
				assert(string.lower(string.sub(s, 1, 7)) ~= m, `machine {k} is a status's colour {s}`)
			end
		end
		-- The same every time.
		local again = pal.make(inks, p.dark)
		for k, m in machines do
			assert(again[k] == m, `machine {k}: {m} then {again[k]}`)
		end
	end)

	test(`force palette ({theme}): a mark is black or white, whichever contrasts more with its fill (WCAG)`, function()
		local p = read(theme)
		for k, m in p.machines do
			local ink = pal.mark_ink(m)
			local black, white = pal.contrast(pal.DARK_INK, m), pal.contrast(pal.LIGHT_INK, m)
			assert(ink == pal.DARK_INK or ink == pal.LIGHT_INK, `machine {k}: ink {ink}`)
			local mine = if ink == pal.DARK_INK then black else white
			assert(mine >= math.max(black, white), `machine {k} {m}: {ink} {mine} < the other`)
			assert(mine >= 4.5, `machine {k} {m}: contrast {mine}`)
		end
	end)
end

test("force palette: contrast ratio is WCAG's; the higher one wins at either end", function()
	assert(math.abs(pal.contrast("#000000", "#ffffff") - 21) < 0.01)
	assert(math.abs(pal.contrast("#777777", "#777777") - 1) < 0.001)
	assert(pal.mark_ink("#202020") == pal.LIGHT_INK)
	assert(pal.mark_ink("#e0e0e0") == pal.DARK_INK)
	-- (Mid blue: white's 4.6 beats black's 4.6? the exact ratios decide.)
	local m = "#3b82f6"
	local want = if pal.contrast("#000000", m) >= pal.contrast("#ffffff", m) then pal.DARK_INK else pal.LIGHT_INK
	assert(pal.mark_ink(m) == want)
end)

test("force palette: statuses round the whole wheel still leave colours, apart from them", function()
	local statuses = {}
	for h = 0, 330, 30 do
		table.insert(statuses, pal.oklch(0.7, 0.15, h))
	end
	local out = pal.make(statuses, true)
	assert(#out >= 6, `{#out} colours`)
	for _, m in out do
		local _, c, h = pal.hue_of(m)
		assert(c > pal.GREY_C)
		for _, s in statuses do
			local _, _, sh = pal.hue_of(s)
			assert(apart(h, sh) >= 11, `{m} {apart(h, sh)}° from a status`)
		end
	end
end)

-- A mark's visual bounds (its curves' own extremes, sampled finely).
local function bounds(m: any, d: number): (number, number, number, number)
	local x0, y0, x1, y1 = math.huge, math.huge, -math.huge, -math.huge
	local function see(x: number, y: number)
		x0, x1 = math.min(x0, x * d), math.max(x1, x * d)
		y0, y1 = math.min(y0, y * d), math.max(y1, y * d)
	end
	for _, part in m.parts do
		local c = part.cmds
		local i, px, py = 1, 0, 0
		while i <= #c do
			local op = c[i]
			if op == 1 or op == 2 then
				px, py = c[i + 1], c[i + 2]
				see(px, py)
				i += 3
			elseif op == 3 then
				for k = 1, 64 do
					local t = k / 64
					local u = 1 - t
					see(u * u * u * px + 3 * u * u * t * c[i + 1] + 3 * u * t * t * c[i + 3] + t * t * t * c[i + 5], u * u * u * py + 3 * u * u * t * c[i + 2] + 3 * u * t * t * c[i + 4] + t * t * t * c[i + 6])
				end
				px, py = c[i + 5], c[i + 6]
				i += 7
			else
				i += 1
			end
		end
	end
	return x0, y0, x1, y1
end

test("force marks: each centred on its visual bounds within half a point, at the sizes drawn; sized alike", function()
	local draw = require("@ds/orch_force_draw")
	local sizes = { draw.MARK_CHAT, 64 }
	for _, shape in { "cookie4", "cookie6", "cookie7", "cookie9", "cookie12", "soft_burst" } do
		local d = draw.mark_hub(shape)
		-- (Within the shape's inner circle: 60% of it, near enough.)
		assert(d >= 0.5 * draw.inner(shape) * draw.HUB_FILL_D and d <= 0.7 * draw.inner(shape) * draw.HUB_FILL_D, `{shape}: {d}`)
		table.insert(sizes, d)
	end
	local sides: { [string]: number } = {}
	for _, name in { "claude", "openai", "cursor", "grok", "gemini", "copilot" } do
		local m = (marks :: any)[name]
		assert(m, `no mark {name}`)
		-- (The sizes the Graph draws them: a chat's, an orchestrator's.)
		for _, d in sizes do
			local x0, y0, x1, y1 = bounds(m, d)
			local cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
			assert(math.abs(cx) <= 0.5 and math.abs(cy) <= 0.5, `{name} at {d}: centre off by {cx}, {cy}`)
			local side = math.max(x1 - x0, y1 - y0)
			-- (60–65% of the fill's inner circle: the base, eased optically.)
			assert(side >= 0.95 * d and side <= 1.1 * d, `{name} at {d}: {side} across`)
		end
		local x0, y0, x1, y1 = bounds(m, 1)
		sides[name] = math.max(x1 - x0, y1 - y0)
	end
	-- Codex wears OpenAI's; Claude's burst and OpenAI's knot read the same size.
	assert(draw.mark_of("codex") == "openai" and draw.mark_of("openai") == "openai" and draw.mark_of("other") == nil)
	assert(math.abs(sides.claude - sides.openai) <= 0.04, `claude {sides.claude} openai {sides.openai}`)
end)

for _, theme in { "theme_dark", "theme_light" } do
	test(`force palette ({theme}): hues spread round the wheel, the first machines' clearly apart (ΔE)`, function()
		local p: any = read(theme)
		local m: { string } = p.machines
		local hues = {}
		for _, c in m do
			local _, _, h = pal.hue_of(c)
			table.insert(hues, h)
		end
		-- (Round the wheel: every quarter of it has one.)
		local quarters = {}
		for _, h in hues do
			quarters[math.floor(h / 90) % 4] = true
		end
		expect(quarters[0] and quarters[1] and quarters[2] and quarters[3]).to_be(true)
		-- (The first six, any two: a clear difference.)
		local least = math.huge
		for i = 1, math.min(6, #m) do
			for j = i + 1, math.min(6, #m) do
				least = math.min(least, pal.delta(m[i], m[j]))
			end
		end
		print(`zeron-zgx: FORCEPAL_DE {p.theme} least ΔE among the first six {string.format("%.1f", least)}`)
		expect(least >= 8).to_be(true)
	end)
end

-- The legend wears the machines' colours as this theme's tokens
-- (`$zeron.force.machine.m1`…, declared with the Map): the same colours
-- the canvas fills them with, in each theme, and again after a switch.
test("force palette: the machines' tokens are their colours, in each theme and after a switch", function()
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 2400)
	app:command("map_only")
	app:command("map_force_big")
	app:command("map_force")
	app:frames(5)
	for _, theme in { "theme_dark", "theme_light", "theme_dark" } do
		app:command(theme)
		app:frames(3)
		app:command("map_force_palette")
		app:frames(1)
		local p: any = app:state("force_palette")
		for k, m in p.machines do
			local t = p.tokens[k]
			expect(`{theme} m{k} {t and string.lower(string.sub(t, 1, 7))}`).to_be(`{theme} m{k} {m}`)
		end
		expect(pal.token(3)).to_be("$zeron.force.machine.m3")
	end
	-- (The legend's dots wear them: a token, no colour written out.)
	app:command("theme_dark")
	app:frames(3)
	expect(app:count(".orch-force-legend-item") >= 2).to_be(true)
end)
