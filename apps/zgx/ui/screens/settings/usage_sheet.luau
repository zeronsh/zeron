-- Settings → Providers' Usage sheet: every sign-in's plan limits in one
-- place, pooled by the service they draw on. It reads the same device
-- account snapshots composers use (`agents.on_device`) so Claude, ChatGPT,
-- Copilot and the rest can be compared side by side without squeezing every
-- provider into "session" and "week". The device is the Providers page's
-- device: this computer, another computer, or Cloud's vault accounts.

local ds = require("@ds")
local material = require("@ds/material")
local PrivateText = require("@ds/private_text")
local provider_card = require("@ds/provider_card")
local agents = require("@account/agents")
local usage_pools = require("@account/usage_pools")
local cloud = require("@account/cloud")
local sessions = require("@account/sessions")
local usage_open = require("@app/screens/settings/usage_open")
local Chart = require("@ds/chart")
local ink = require("@ds/visual_ink")

local M = {}

local function tone(used: number): string
	return if used >= 0.95 then "danger" elseif used >= 0.8 then "warning" else "accent"
end

local function percent(used: number): string
	return `{math.floor(used * 100 + 0.5)}%`
end

-- A short pace word where the window's length is known. Pace is usage
-- divided by the share of the window elapsed: 1.0 exactly on pace.
local function pace_words(pace: number?): string?
	if not pace then
		return nil
	end
	if pace >= 1.25 then
		return ui.t("settings.providers.usage.pace.fast"):get()
	elseif pace <= 0.75 then
		return ui.t("settings.providers.usage.pace.slow"):get()
	end
	return ui.t("settings.providers.usage.pace.steady"):get()
end

local function window_meta(w: usage_pools.Window): string
	local parts = {}
	if w.resets_at then
		local when = provider_card.when_words(w.resets_at)
		if when then
			table.insert(parts, ui.t("settings.providers.usage.resets", { when = when }):get())
		end
	end
	local pace = pace_words(w.pace)
	if pace then
		table.insert(parts, pace)
	end
	return table.concat(parts, " · ")
end

-- How far through its window a limit is (0 to 1), where its length and
-- reset are known.
local function elapsed(w: usage_pools.Window): number?
	if not w.resets_at or not w.window_seconds or w.window_seconds <= 0 then
		return nil
	end
	return math.clamp(1 - (w.resets_at - os.time()) / w.window_seconds, 0, 1)
end

local function window_row(_w: usage_pools.Window, _i: number, row: State<usage_pools.Window>): Node
	local function w(): usage_pools.Window
		return row:get()
	end
	return ui.row {
		w_full = true,
		gap = "$space.2",
		items = "center",
		ui.text {
			function()
				return w().label
			end,
			w = 112,
			shrink = 0,
			text_size = "$font.size.xs",
			line_height = "$font.line.xs",
			color = "$text.secondary",
			truncate = true,
		},
		ui.div {
			grow = 1,
			min_w = 40,
			relative = true,
			ds.ProgressBar {
				value = function()
					return w().used
				end,
				tone = function()
					return tone(w().used) :: any
				end,
				size = "regular",
				w_full = true,
				label = function()
					return ui.t("settings.providers.usage.meter", { label = w().label }):get()
				end,
			},
			-- Where even spending would be by now: the bar past it is ahead
			-- of pace.
			ui.show(function()
				return elapsed(w()) ~= nil
			end, ui.div {
				absolute = true,
				top = -3,
				left = function(): string
					return `{math.floor((elapsed(w()) or 0) * 1000) / 10}%`
				end,
				w = 2,
				h = 10,
				rounded = "$radius.full",
				bg = "$text.primary",
				tooltip = ui.t("settings.providers.usage.pace_marker"):get(),
			}),
		},
		ui.text {
			function()
				return percent(w().used)
			end,
			w = 36,
			shrink = 0,
			align = "right",
			tabular = true,
			text_size = "$font.size.xs",
			line_height = "$font.line.xs",
			color = function()
				local t = tone(w().used)
				return if t == "accent" then "$text.secondary" else `$zeron.tone.{t}.ink`
			end,
		},
		ui.text {
			function()
				return window_meta(w())
			end,
			w = 132,
			shrink = 0,
			align = "right",
			text_size = "$font.size.xs",
			line_height = "$font.line.xs",
			color = "$text.tertiary",
			truncate = true,
		},
	}
end

local function harness_words(account: usage_pools.Account, names: { [string]: string }): string
	local out = {}
	for _, id in account.harnesses do
		table.insert(out, names[id] or id)
	end
	table.sort(out)
	return table.concat(out, " · ")
end

local function account_row(names: { [string]: string }): (usage_pools.Account, number, State<usage_pools.Account>) -> Node
	return function(_a: usage_pools.Account, _i: number, row: State<usage_pools.Account>): Node
		local function a(): usage_pools.Account
			return row:get()
		end
		local windows = ui.computed(function(): { usage_pools.Window }
			return a().windows
		end)
		return ui.column {
			w_full = true,
			gap = "$space.2",
			py = "$space.3",
			border_sides = { 1, 0, 0, 0 },
			border_color = "$border.subtle",
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				ui.show(function()
					return not a().key_account
				end, ui.presence(function()
					return a().email
				end, function(email: string): Node
					return ds.Avatar { seed = email, size = 24, label = ui.t("settings.providers.usage.account") }
				end)),
				ui.column {
					grow = 1,
					min_w = 0,
					gap = ui.raw(1, "the address on its plan line"),
					ui.row {
						gap = "$space.2",
						items = "center",
						PrivateText {
							function()
								return a().email
							end,
							name = ui.t("settings.providers.usage.account"),
							truncate = true,
							reveal = false,
							text_size = "$font.size.sm",
							line_height = "$font.line.sm",
							weight = "$font.weight.medium",
							color = "$text.primary",
							grow = 1,
							min_w = 0,
						},
						ui.show(function()
							return a().active
						end, ds.Badge { ui.t("settings.providers.usage.active"), tone = "accent", size = "sm" }),
						ui.show(function()
							return a().key_account
						end, ds.Badge { ui.t("settings.providers.usage.api_key"), tone = "neutral", size = "sm" }),
					},
					ui.text {
						function()
							local bits = {}
							if a().plan ~= "" then
								table.insert(bits, a().plan)
							end
							table.insert(bits, harness_words(a(), names))
							return table.concat(bits, " · ")
						end,
						text_size = "$font.size.xs",
						line_height = "$font.line.xs",
						color = "$text.tertiary",
						truncate = true,
					},
				},
				ui.text {
					function()
						return if #a().windows > 0 then percent(a().worst) else "—"
					end,
					w = 42,
					shrink = 0,
					align = "right",
					tabular = true,
					text_size = "$font.size.md",
					line_height = "$font.line.md",
					weight = "$font.weight.semibold",
					color = function()
						local t = tone(a().worst)
						return if #a().windows == 0 then "$text.tertiary" elseif t == "accent" then "$text.primary" else `$zeron.tone.{t}.ink`
					end,
				},
			},
			ui.show(function()
				return a().error ~= nil
			end, ui.text {
				function()
					return a().error or ""
				end,
				text_size = "$font.size.xs",
				line_height = "$font.line.xs",
				color = "$zeron.tone.warning.ink",
				wrap = true,
			}),
			ui.show(function()
				return #a().windows == 0
			end, ui.text {
				ui.t("settings.providers.usage.no_quota"),
				text_size = "$font.size.xs",
				line_height = "$font.line.xs",
				color = "$text.tertiary",
			}),
			ui.column {
				w_full = true,
				gap = "$zeron.space.hair",
				ui.each(windows, window_row, {
					key = function(w: usage_pools.Window): string
						return `{a().key}|{w.label}`
					end,
				}),
			},
		}
	end
end

local function pool_card(_p: usage_pools.Pool, _i: number, row: State<usage_pools.Pool>): Node
	local function p(): usage_pools.Pool
		return row:get()
	end
	local accounts = ui.computed(function(): { usage_pools.Account }
		return p().accounts
	end)
	local names = {}
	for _, agent in agents.now:peek().list do
		names[agent.id] = agent.name
	end
	return ui.column {
		w_full = true,
		p = "$space.4",
		gap = "$space.2",
		rounded = "$radius.xl",
		bg = material.pick("$zeron.settings.tint", "$zeron.settings.card"),
		border = 1,
		border_color = "$zeron.settings.edge",
		ui.row {
			w_full = true,
			items = "center",
			gap = "$space.3",
			ui.column {
				grow = 1,
				min_w = 0,
				gap = ui.raw(1, "the pool's title on its count"),
				ui.text {
					function()
						return p().title
					end,
					text_size = "$font.size.md",
					line_height = "$font.line.md",
					weight = "$font.weight.semibold",
					color = "$text.primary",
				},
				ui.text {
					function()
						local n = #p().accounts
						return if n == 1
							then ui.t("settings.providers.usage.account_count", { n = n }):get()
							else ui.t("settings.providers.usage.accounts", { n = n }):get()
					end,
					text_size = "$font.size.xs",
					line_height = "$font.line.xs",
					color = "$text.tertiary",
				},
			},
			ui.text {
				function()
					return percent(p().worst)
				end,
				text_size = "$font.size.lg",
				line_height = "$font.line.lg",
				weight = "$font.weight.semibold",
				tabular = true,
				color = function()
					local t = tone(p().worst)
					return if t == "accent" then "$text.primary" else `$zeron.tone.{t}.ink`
				end,
			},
		},
		ui.column {
			w_full = true,
			ui.each(accounts, account_row(names), {
				key = function(a: usage_pools.Account): string
					return a.key
				end,
			}),
		},
	}
end

local function cloud_card(): Node
	return ui.show(function()
		return cloud.on() and cloud.now:get().status ~= nil
	end, ui.column {
		w_full = true,
		p = "$space.4",
		gap = "$space.2",
		rounded = "$radius.xl",
		bg = material.pick("$zeron.settings.tint", "$zeron.settings.card"),
		border = 1,
		border_color = "$zeron.settings.edge",
		ui.row {
			w_full = true,
			items = "center",
			gap = "$space.3",
			ui.row {
				w = 28,
				h = 28,
				shrink = 0,
				items = "center",
				justify = "center",
				rounded = "$radius.full",
				bg = "$zeron.secondary.bg",
				color = "$text.secondary",
				ds.Icon { name = "cloud", size = 15 },
			},
			ui.column {
				grow = 1,
				min_w = 0,
				gap = ui.raw(1, "the cloud title on its note"),
				ui.text { ui.t("settings.providers.usage.cloud"), text_size = "$font.size.md", line_height = "$font.line.md", weight = "$font.weight.semibold", color = "$text.primary" },
				ui.text {
					ui.t("settings.providers.usage.cloud_note"),
					text_size = "$font.size.xs",
					line_height = "$font.line.xs",
					color = "$text.tertiary",
					wrap = true,
				},
			},
			ui.text {
				function()
					local status = cloud.now:get().status
					local c = if status then status.credits else nil
					return if c then `{math.floor(c + 0.5)}` else "—"
				end,
				text_size = "$font.size.lg",
				line_height = "$font.line.lg",
				weight = "$font.weight.semibold",
				color = "$text.primary",
				tabular = true,
			},
		},
	})
end

-- Token numbers in the short form analytics scan best at.
local function token_words(n: number?): string
	n = n or 0
	if n >= 1_000_000_000 then
		return string.format("%.1fB", n / 1_000_000_000)
	elseif n >= 1_000_000 then
		return string.format("%.1fM", n / 1_000_000)
	elseif n >= 10_000 then
		return string.format("%.1fk", n / 1_000)
	elseif n >= 1000 then
		return string.format("%.2fk", n / 1000)
	end
	return tostring(math.floor(n + 0.5))
end

local function money_words(n: number?): string
	if not n then
		return "—"
	end
	if n >= 100 then
		return string.format("$%.2f", n)
	end
	return (string.format("$%.4f", n):gsub("0+$", ""):gsub("%.$", ""))
end

local function share_words(part: number, whole: number): string
	if whole <= 0 then
		return "0%"
	end
	local p = part / whole * 100
	if p > 0 and p < 1 then
		return "<1%"
	end
	return `{math.floor(p + 0.5)}%`
end

local function num(v: any): number
	return tonumber(v) or 0
end

local function card(props: { [any]: any }): Node
	local out = {
		w_full = true,
		p = "$space.4",
		gap = "$space.3",
		rounded = "$radius.xl",
		bg = material.pick("$zeron.settings.tint", "$zeron.settings.card"),
		border = 1,
		border_color = "$zeron.settings.edge",
	}
	for k, v in props do
		if type(k) == "number" then
			table.insert(out, v)
		else
			out[k] = v
		end
	end
	return ui.column(out)
end

local function card_title(label: Message, right: (string | Message)?): Node
	return ui.row {
		w_full = true,
		items = "center",
		justify = "between",
		gap = "$space.2",
		ui.text { label, text_size = "$font.size.sm", line_height = "$font.line.sm", weight = "$font.weight.semibold", color = "$text.primary" },
		if right
			then ui.text { right, text_size = "$font.size.xs", line_height = "$font.line.xs", color = "$text.tertiary", tabular = true }
			else nil,
	}
end

-- A round axis for `hi` in four or five steps of 1, 2 or 5 × 10ⁿ.
local function nice_axis(hi: number, unit: string, scale: number): any
	if hi <= 0 then
		hi = 1
	end
	local raw = hi / 4
	local mag = 10 ^ math.floor(math.log10(raw))
	local step = mag
	for _, m in { 1, 2, 2.5, 5, 10 } do
		if m * mag >= raw then
			step = m * mag
			break
		end
	end
	local top = math.ceil(hi / step) * step
	local ticks, labels = {}, {}
	local v = 0
	while v <= top + step / 2 do
		table.insert(ticks, v)
		local shown = v / scale
		local text = if shown % 1 == 0 then string.format("%d", shown) else string.format("%.1f", shown)
		table.insert(labels, if v == 0 then "0" else `{text}{unit}`)
		v += step
	end
	return { lo = 0, hi = top, ticks = ticks, labels = labels }
end

-- The summary laid on the whole range: every step from `from` to `to`
-- (empty ones zero), each split by agent in rank order.
local function series_of(view: any, agents_ranked: { any }): any
	local data = view.data
	local step = view.step
	local first = math.floor(view.from / step) * step
	local by_start: { [number]: any } = {}
	for _, b in data.buckets or {} do
		by_start[num(b.start)] = b
	end
	local x, totals = {}, {}
	local series = {}
	for i, h in agents_ranked do
		series[i] = { name = h.name, color = `chart{((i - 1) % 6) + 1}`, values = {} }
	end
	local t = first
	local peak = 0
	while t < view.to do
		local b = by_start[t]
		-- (Hours in local time; days are the engine's UTC days.)
		table.insert(x, os.date(if view.hourly then "%H:00" else "!%b %d", t) :: string)
		local split: { [string]: number } = {}
		for _, part in (b and b.byHarness) or {} do
			split[tostring(part.harness)] = num(part.totalTokens)
		end
		local sum = 0
		for i, h in agents_ranked do
			local v = split[h.id] or 0
			table.insert(series[i].values, v)
			sum += v
		end
		-- An old engine's bucket has no split: its whole as the top agent's.
		if b and not b.byHarness and series[1] then
			series[1].values[#series[1].values] = num(b.totalTokens)
			sum = num(b.totalTokens)
		end
		table.insert(totals, sum)
		peak = math.max(peak, sum)
		t += step
	end
	local scale, unit = 1, ""
	if peak >= 1_000_000 then
		scale, unit = 1_000_000, "M"
	elseif peak >= 1_000 then
		scale, unit = 1_000, "k"
	end
	for _, s in series do
		for i, v in s.values do
			s.values[i] = v / scale
		end
	end
	local axis = nice_axis(peak, unit, scale)
	axis.hi /= scale
	for i, v in axis.ticks do
		axis.ticks[i] = v / scale
	end
	return {
		type = "bar",
		stacked = true,
		x = x,
		series = series,
		yaxis = axis,
		fmt = { unit = unit, decimals = if unit == "" then 0 else 1 },
	}
end

local function dot(color: any, size: number?): Node
	return ui.div {
		w = size or 8,
		h = size or 8,
		shrink = 0,
		rounded = "$radius.full",
		bg = ink.paint(color),
	}
end

-- One bar split by share: Input, cache read, cache write, output.
local function share_bar(parts: { { label: string, value: number, color: any } }, whole: number): Node
	local segs = {}
	for _, p in parts do
		if p.value > 0 then
			table.insert(segs, ui.div {
				grow = p.value,
				basis = 0,
				min_w = 4,
				h = 8,
				rounded = "$radius.full",
				bg = ink.paint(p.color),
				tooltip = `{p.label} · {token_words(p.value)} · {share_words(p.value, whole)}`,
			})
		end
	end
	return ui.row { w_full = true, gap = "$zeron.space.hair", table.unpack(segs) }
end

local function legend_cell(color: any, label: string, value: string, share: string): Node
	return ui.column {
		grow = 1,
		basis = 0,
		min_w = 0,
		gap = ui.raw(2, "a legend value under its name"),
		ui.row {
			items = "center",
			gap = "$space.2",
			dot(color),
			ui.text { label, literal = true, text_size = "$font.size.xs", line_height = "$font.line.xs", color = "$text.tertiary", truncate = true },
		},
		ui.row {
			items = "baseline",
			gap = "$space.1",
			ui.text { value, literal = true, text_size = "$font.size.md", line_height = "$font.line.md", weight = "$font.weight.semibold", color = "$text.primary", tabular = true },
			ui.text { share, literal = true, text_size = "$font.size.xs", line_height = "$font.line.xs", color = "$text.tertiary", tabular = true },
		},
	}
end

local function agent_row(h: any, whole: number): Node
	local bits = { ui.t("settings.providers.usage.share_of", { share = share_words(h.total, whole) }):get() }
	if h.cost then
		table.insert(bits, money_words(h.cost))
	end
	return ui.row {
		w_full = true,
		items = "center",
		gap = "$space.3",
		py = ui.raw(6, "an agent row's breath"),
		border_sides = { 1, 0, 0, 0 },
		border_color = "$border.subtle",
		dot(h.color, 10),
		ui.column {
			grow = 1,
			min_w = 0,
			gap = ui.raw(1, "the agent's name on its share"),
			ui.text { h.name, literal = true, text_size = "$font.size.sm", line_height = "$font.line.sm", weight = "$font.weight.medium", color = "$text.primary", truncate = true },
			ui.text { table.concat(bits, " · "), literal = true, text_size = "$font.size.xs", line_height = "$font.line.xs", color = "$text.tertiary", truncate = true },
		},
		ui.text { token_words(h.total), literal = true, text_size = "$font.size.sm", line_height = "$font.line.sm", weight = "$font.weight.semibold", color = "$text.primary", tabular = true },
	}
end

local function model_row(rank: number, m: any, peak: number, whole: number): Node
	local small = { text_size = "$font.size.xs", line_height = "$font.line.xs", tabular = true, align = "right", shrink = 0 }
	local function cell(text: string, w: number, color: string): Node
		local props = table.clone(small) :: any
		props[1] = text
		props.literal = true
		props.w = w
		props.color = color
		return ui.text(props)
	end
	return ui.row {
		w_full = true,
		items = "center",
		gap = "$space.3",
		py = ui.raw(7, "a model row's breath"),
		border_sides = { 1, 0, 0, 0 },
		border_color = "$border.subtle",
		cell(tostring(rank), 16, "$text.tertiary"),
		ui.column {
			grow = 1,
			min_w = 0,
			gap = ui.raw(4, "a model's name on its bar"),
			ui.text { tostring(m.model or "unknown"), literal = true, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.primary", truncate = true },
			ui.row {
				w_full = true,
				max_w = 220,
				h = 3,
				ui.div {
					grow = math.max(num(m.totalTokens), peak * 0.01),
					basis = 0,
					h = 3,
					rounded = "$radius.full",
					bg = ink.paint("chart1"),
				},
				ui.div { grow = math.max(0, peak - num(m.totalTokens)), basis = 0 },
			},
		},
		cell(if m.costUsd then money_words(num(m.costUsd)) else "—", 72, if m.costUsd then "$text.secondary" else "$text.tertiary"),
		cell(share_words(num(m.totalTokens), whole), 44, "$text.tertiary"),
		cell(token_words(num(m.totalTokens)), 56, "$text.primary"),
	}
end

local function header_cell(label: Message, w: number?, grow: boolean?): Node
	return ui.text {
		label,
		literal = if type(label) == "string" then true else nil,
		w = w,
		grow = if grow then 1 else nil,
		shrink = 0,
		align = if grow then nil else "right",
		text_size = "$font.size.xs",
		line_height = "$font.line.xs",
		weight = "$font.weight.medium",
		color = "$text.tertiary",
	}
end

local function tokens_body(view: any, names: { [string]: string }): Node
	local data = view.data
	local totals = data.totals or {}
	local whole = num(totals.totalTokens)
	-- Agents by tokens, largest first, each with the colour its bars wear.
	local ranked = {}
	for _, h in data.byHarness or {} do
		local id = tostring(h.harness or "unknown")
		table.insert(ranked, { id = id, name = names[id] or id, total = num(h.totalTokens), cost = tonumber(h.costUsd) })
	end
	table.sort(ranked, function(a, b)
		return a.total > b.total
	end)
	for i, h in ranked do
		h.color = `chart{((i - 1) % 6) + 1}`
	end
	local chart_data = series_of(view, ranked)
	local models = table.clone(data.byModel or {})
	table.sort(models, function(a, b)
		return num(a.totalTokens) > num(b.totalTokens)
	end)
	local model_peak = if models[1] then num(models[1].totalTokens) else 0

	local cache_read = num(totals.cacheReadInputTokens)
	local cache_write = num(totals.cacheCreationInputTokens)
	local input = num(totals.inputTokens)
	local output = num(totals.outputTokens)
	local reasoning = num(totals.reasoningTokens)
	local prompt = input + cache_read + cache_write
	local L = function(key: string, args: any?): string
		return ui.t(`settings.providers.usage.{key}`, args):get()
	end
	local types = {
		{ label = L("type.input"), value = input, color = { token = "chart1", alpha = 0.55 } },
		{ label = L("type.cache_read"), value = cache_read, color = { token = "chart1", alpha = 0.25 } },
		{ label = L("type.cache_write"), value = cache_write, color = { token = "chart3", alpha = 0.6 } },
		{ label = L("type.output"), value = output, color = "chart1" },
	}

	local sub = { L("turns", { n = num(totals.events) }) }
	if totals.costUsd then
		table.insert(sub, L("cost_reported", { cost = money_words(num(totals.costUsd)) }))
	end
	if prompt > 0 then
		table.insert(sub, L("cache_hit", { share = share_words(cache_read, prompt) }))
	end

	local agent_rows = {}
	for _, h in ranked do
		table.insert(agent_rows, agent_row(h, whole))
	end
	local model_rows = {}
	for i, m in models do
		table.insert(model_rows, model_row(i, m, model_peak, whole))
	end
	local type_cells = {}
	for _, p in types do
		table.insert(type_cells, legend_cell(p.color, p.label, token_words(p.value), share_words(p.value, whole)))
	end

	return ui.column {
		w_full = true,
		gap = "$space.3",
		card {
			ui.row {
				w_full = true,
				items = "end",
				justify = "between",
				gap = "$space.3",
				ui.column {
					min_w = 0,
					gap = ui.raw(2, "the headline under its label"),
					ui.text { ui.t("settings.providers.usage.processed"), text_size = "$font.size.xs", line_height = "$font.line.xs", color = "$text.tertiary" },
					ui.text {
						token_words(whole),
						literal = true,
						text_size = "$font.size.xxl",
						line_height = "$font.line.xxl",
						weight = "$font.weight.semibold",
						color = "$text.primary",
						tabular = true,
					},
					ui.text { table.concat(sub, " · "), literal = true, text_size = "$font.size.xs", line_height = "$font.line.xs", color = "$text.secondary", truncate = true },
				},
				ui.text {
					if view.hourly then ui.t("settings.providers.usage.per_hour") else ui.t("settings.providers.usage.per_day"),
					text_size = "$font.size.xs",
					line_height = "$font.line.xs",
					color = "$text.tertiary",
					shrink = 0,
				},
			},
			Chart {
				data = function()
					return chart_data
				end,
				height = 180,
				label = L("chart_label"),
			},
			ui.column { w_full = true, table.unpack(agent_rows) },
		},
		card {
			card_title(ui.t("settings.providers.usage.by_type")),
			share_bar(types, whole),
			ui.row { w_full = true, gap = "$space.3", table.unpack(type_cells) },
			if reasoning > 0
				then ui.text {
					L("reasoning_note", { n = token_words(reasoning) }),
					literal = true,
					text_size = "$font.size.xs",
					line_height = "$font.line.xs",
					color = "$text.tertiary",
				}
				else nil,
		},
		card {
			gap = "$space.2",
			card_title(ui.t("settings.providers.usage.by_model"), L("model_count", { n = #models })),
			ui.row {
				w_full = true,
				gap = "$space.3",
				header_cell("#", 16),
				header_cell(ui.t("settings.providers.usage.col.model"), nil, true),
				header_cell(ui.t("settings.providers.usage.col.cost"), 72),
				header_cell(ui.t("settings.providers.usage.col.share"), 44),
				header_cell(ui.t("settings.providers.usage.col.tokens"), 56),
			},
			ui.column { w_full = true, table.unpack(model_rows) },
		},
	}
end

local function tokens_view(resource: any, target: any): Node
	return ui.column {
		w_full = true,
		gap = "$space.3",
		ui.show(function()
			return target:get() == "cloud"
		end, ds.EmptyState {
			icon = "cloud",
			title = ui.t("settings.providers.usage.tokens_cloud_title"),
			body = ui.t("settings.providers.usage.tokens_cloud_body"),
			size = "sm",
		}),
		ui.show(function()
			return target:get() ~= "cloud" and resource.loading:get() and resource.data:get() == nil
		end, ui.text {
			ui.t("settings.providers.usage.tokens_loading"),
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			color = "$text.tertiary",
		}),
		ui.show(function()
			return target:get() ~= "cloud" and resource.error:get() ~= nil
		end, ui.text {
			function()
				return ui.t("settings.providers.usage.tokens_error", { error = tostring(resource.error:get()) }):get()
			end,
			text_size = "$font.size.xs",
			line_height = "$font.line.xs",
			color = "$zeron.tone.warning.ink",
			wrap = true,
		}),
		ui.presence(function()
			local view = if target:get() ~= "cloud" then resource.data:get() else nil
			return if view and view.data then view else nil
		end, function(view: any): Node
			local data = view.data
			local totals = data.totals or {}
			local coverage = data.coverage or {}
			local names = {}
			for _, agent in agents.now:peek().list do
				names[agent.id] = agent.name
			end
			return ui.column {
				w_full = true,
				gap = "$space.3",
				if num(totals.events) == 0
					then ds.EmptyState {
						icon = "inbox",
						title = ui.t("settings.providers.usage.tokens_empty"),
						body = ui.t("settings.providers.usage.tokens_empty_body"),
						size = "sm",
					}
					else tokens_body(view, names),
				ui.text {
					function()
						local started = tonumber(coverage.startedCollectingAt)
						local when = started and provider_card.when_words(started) or ui.t("settings.providers.usage.just_now"):get()
						local note = ui.t("settings.providers.usage.coverage_note", { when = when }):get()
						if coverage.backfillTimestampsEstimated then
							note ..= " " .. ui.t("settings.providers.usage.backfill_note"):get()
						end
						return note
					end,
					text_size = "$font.size.xs",
					line_height = "$font.line.xs",
					color = "$text.tertiary",
					wrap = true,
				},
			}
		end),
	}
end



-- The device a global opening reads: the Providers target when one was
-- chosen, else this computer.
local function global_device_name(): string
	local target = agents.now:get().target or ""
	if target == "cloud" then
		return ui.t("settings.providers.cloud"):get()
	end
	if target ~= "" then
		for _, d in sessions.devices:get() do
			if d.id == target then
				return if d.here then ui.t("settings.providers.this_mac"):get() else d.name
			end
		end
	end
	return ui.t("settings.providers.this_mac"):get()
end

-- The sheet itself.
function M.sheet(open: State<boolean>, device_name: () -> string): Node
	-- The device's account rows are the composers' shared cache. Opening the
	-- sheet watches the same target, so a first open is warm and the app's
	-- periodic usage poll keeps it fresh afterwards.
	ui.effect(function()
		if not open:get() then
			return
		end
		local target = agents.now:get().target or ""
		agents.watch_device(target)
	end)
	local tab = ui.state("settings_usage_tab", "limits")
	local range = ui.state("settings_usage_range", "7d")
	local data = ui.computed(function()
		local target = agents.now:get().target or ""
		return usage_pools.collect((agents.on_device:get() :: any)[target], os.time())
	end)
	local pools = ui.computed(function(): { usage_pools.Pool }
		return data:get().pools
	end)
	local latest = ui.computed(function(): number?
		local at: number? = nil
		for _, p in data:get().pools do
			for _, a in p.accounts do
				if a.usage_fetched_at and (not at or a.usage_fetched_at > at) then
					at = a.usage_fetched_at
				end
			end
		end
		return at
	end)
	local target = ui.computed(function(): string
		return agents.now:get().target or ""
	end)
	local summary = ui.resource(function(): any
		if not open:get() or tab:get() ~= "tokens" then
			return nil
		end
		local target = agents.now:get().target or ""
		if target == "cloud" then
			return nil
		end
		local spec = if range:get() == "24h"
			then { seconds = 86400, bucket = "hour" }
			elseif range:get() == "30d" then { seconds = 30 * 86400, bucket = "day" }
			else { seconds = 7 * 86400, bucket = "day" }
		local step = if spec.bucket == "hour" then 3600 else 86400
		-- The range ends at the end of the current step, so the newest bar
		-- is the one still filling.
		local to = (math.floor(os.time() / step) + 1) * step
		local from = to - spec.seconds
		local data = agents.usage_summary(target, from, to, spec.bucket):await()
		if data == nil then
			return nil
		end
		return { data = data, from = from, to = to, step = step, hourly = spec.bucket == "hour" }
	end)
	return ds.Sheet {
		open = open,
		title = ui.t("settings.providers.usage.title"),
		description = function()
			return ui.t("settings.providers.usage.description", { device = device_name() }):get()
		end,
		size = 640,
		ds.Tabs {
			value = tab,
			look = "pill",
			size = "sm",
			label = ui.t("settings.providers.usage.title"),
			options = {
				{ value = "limits", label = ui.t("settings.providers.usage.limits_tab") },
				{ value = "tokens", label = ui.t("settings.providers.usage.tokens_tab") },
			},
		},
		ui.show(function()
			return tab:get() == "limits"
		end, function(): Node
			return ui.column {
				w_full = true,
				gap = "$space.3",
				ui.row {
					w_full = true,
					items = "center",
					justify = "between",
					gap = "$space.3",
					ui.text {
						function()
							local at = latest:get()
							if not at then
								return ui.t("settings.providers.usage.never"):get()
							end
							return ui.t("settings.providers.usage.checked", { when = provider_card.when_words(at) or "" }):get()
						end,
						text_size = "$font.size.xs",
						line_height = "$font.line.xs",
						color = "$text.tertiary",
					},
					ds.Button {
						ui.t("settings.providers.usage.refresh"),
						tone = "secondary",
						size = "sm",
						on_click = function()
							agents.refresh_device(agents.now:peek().target or "")
						end,
						ui.div { y = -0.5, ds.Icon { name = "refresh", size = 13 } },
					},
				},
				cloud_card(),
				ui.show(function()
					return #pools:get() == 0
				end, ds.EmptyState {
					icon = "inbox",
					title = ui.t("settings.providers.usage.empty"),
					body = ui.t("settings.providers.usage.empty_body"),
					size = "sm",
				}),
				ui.each(pools, pool_card, {
					key = function(p: usage_pools.Pool): string
						return p.id
					end,
				}),
			}
		end),
		ui.show(function()
			return tab:get() == "tokens"
		end, function(): Node
			return ui.column {
				w_full = true,
				gap = "$space.3",
				ui.row {
					w_full = true,
					items = "center",
					justify = "between",
					gap = "$space.3",
					ds.Tabs {
						value = range,
						look = "pill",
						size = "sm",
						label = ui.t("settings.providers.usage.range"),
						options = {
							{ value = "24h", label = "24h" },
							{ value = "7d", label = "7d" },
							{ value = "30d", label = "30d" },
						},
					},
					ds.Button {
						ui.t("settings.providers.usage.refresh"),
						tone = "secondary",
						size = "sm",
						on_click = function()
							summary.reload()
						end,
						ui.div { y = -0.5, ds.Icon { name = "refresh", size = 13 } },
					},
				},
				tokens_view(summary, target),
			}
		end),
	}
end

-- The one copy mounted by the shell, so Usage opens over the screen the
-- person is on rather than taking them to Settings first.
function M.global(): Node
	return M.sheet(usage_open, global_device_name)
end

return M
