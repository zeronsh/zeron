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
			ds.ProgressBar {
				value = function()
					return w().used
				end,
				tone = function()
					return tone(w().used) :: any
				end,
				size = "thin",
				w_full = true,
				label = function()
					return ui.t("settings.providers.usage.meter", { label = w().label }):get()
				end,
			},
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
	return string.format("$%.4f", n):gsub("0+$", ""):gsub("%.$", "")
end

local function metric_card(label: Message, value: () -> string, note: (() -> string?)?): Node
	return ui.column {
		grow = 1,
		min_w = 0,
		p = "$space.3",
		gap = ui.raw(1, "the metric's number on its label"),
		rounded = "$radius.lg",
		bg = material.pick("$zeron.settings.tint", "$zeron.settings.card"),
		border = 1,
		border_color = "$zeron.settings.edge",
		ui.text { label, text_size = "$font.size.xs", line_height = "$font.line.xs", color = "$text.tertiary", truncate = true },
		ui.text {
			value,
			text_size = "$font.size.lg",
			line_height = "$font.line.lg",
			weight = "$font.weight.semibold",
			color = "$text.primary",
			tabular = true,
			truncate = true,
		},
		if note then ui.text {
			function()
				return note() or ""
			end,
			text_size = "$font.size.xs",
			line_height = "$font.line.xs",
			color = "$text.tertiary",
			truncate = true,
		} else nil,
	}
end

local function analytics_row(label: string, value: number, share: number, detail: string?): Node
	return ui.row {
		w_full = true,
		gap = "$space.2",
		items = "center",
		py = ui.raw(3, "an analytics row's breath"),
		ui.text { label, w = 128, shrink = 0, text_size = "$font.size.xs", line_height = "$font.line.xs", color = "$text.secondary", truncate = true },
		ui.div {
			grow = 1,
			min_w = 36,
			ds.ProgressBar {
				value = function()
					return share
				end,
				tone = "accent",
				size = "thin",
				w_full = true,
				label = label,
			},
		},
		ui.text {
			token_words(value),
			w = 54,
			shrink = 0,
			align = "right",
			tabular = true,
			text_size = "$font.size.xs",
			line_height = "$font.line.xs",
			color = "$text.primary",
		},
		ui.text {
			detail or "",
			w = 74,
			shrink = 0,
			align = "right",
			text_size = "$font.size.xs",
			line_height = "$font.line.xs",
			color = "$text.tertiary",
			truncate = true,
		},
	}
end

local function day_label(start: number, hourly: boolean): string
	return os.date(if hourly then "!%b %d %H:00" else "!%b %d", math.floor(start)) :: string
end

local function tokens_view(resource: any, range: State<string>, target: any): Node
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
			return if target:get() ~= "cloud" then resource.data:get() else nil
		end, function(data: any): Node
			local totals = data.totals or {}
			local coverage = data.coverage or {}
			local buckets = data.buckets or {}
			local by_harness = data.byHarness or {}
			local by_model = data.byModel or {}
			local max_bucket = 0
			for _, b in buckets do
				max_bucket = math.max(max_bucket, tonumber(b.totalTokens) or 0)
			end
			local max_harness = 0
			for _, h in by_harness do
				max_harness = math.max(max_harness, tonumber(h.totalTokens) or 0)
			end
			return ui.column {
				w_full = true,
				gap = "$space.3",
				ui.row {
					w_full = true,
					gap = "$space.2",
					metric_card(ui.t("settings.providers.usage.total_tokens"), function()
						return token_words(tonumber(totals.totalTokens) or 0)
					end),
					metric_card(ui.t("settings.providers.usage.input_tokens"), function()
						return token_words(tonumber(totals.inputTokens) or 0)
					end),
					metric_card(ui.t("settings.providers.usage.output_tokens"), function()
						return token_words(tonumber(totals.outputTokens) or 0)
					end),
					metric_card(ui.t("settings.providers.usage.reported_cost"), function()
						return money_words(tonumber(totals.costUsd))
					end, function()
						return if totals.costEvents then ui.t("settings.providers.usage.provider_reported"):get() else nil
					end),
				},
				ui.show(function()
					return (tonumber(totals.events) or 0) == 0
				end, ds.EmptyState {
					icon = "inbox",
					title = ui.t("settings.providers.usage.tokens_empty"),
					body = ui.t("settings.providers.usage.tokens_empty_body"),
					size = "sm",
				}),
				ui.show(function()
					return (tonumber(totals.events) or 0) > 0
				end, ui.column {
					w_full = true,
					gap = "$space.2",
					ui.column {
						w_full = true,
						p = "$space.4",
						gap = "$space.2",
						rounded = "$radius.xl",
						bg = material.pick("$zeron.settings.tint", "$zeron.settings.card"),
						border = 1,
						border_color = "$zeron.settings.edge",
						ui.text { ui.t("settings.providers.usage.per_day"), text_size = "$font.size.md", line_height = "$font.line.md", weight = "$font.weight.semibold", color = "$text.primary" },
						ui.each(buckets, function(_b: any, _i: number, row: State<any>): Node
							local function b(): any
								return row:get()
							end
							return analytics_row(
								day_label(tonumber(b().start) or 0, range:peek() == "24h"),
								tonumber(b().totalTokens) or 0,
								if max_bucket > 0 then (tonumber(b().totalTokens) or 0) / max_bucket else 0,
								`{token_words(tonumber(b().inputTokens) or 0)} in · {token_words(tonumber(b().outputTokens) or 0)} out`
							)
						end, {
							key = function(b: any): string
								return tostring(b.start)
							end,
						}),
					},
					ui.column {
						w_full = true,
						p = "$space.4",
						gap = "$space.2",
						rounded = "$radius.xl",
						bg = material.pick("$zeron.settings.tint", "$zeron.settings.card"),
						border = 1,
						border_color = "$zeron.settings.edge",
						ui.text { ui.t("settings.providers.usage.by_agent"), text_size = "$font.size.md", line_height = "$font.line.md", weight = "$font.weight.semibold", color = "$text.primary" },
						ui.each(by_harness, function(_h: any, _i: number, row: State<any>): Node
							local function h(): any
								return row:get()
							end
							return analytics_row(
								tostring(h().harness or "unknown"),
								tonumber(h().totalTokens) or 0,
								if max_harness > 0 then (tonumber(h().totalTokens) or 0) / max_harness else 0,
								if h().costUsd then money_words(tonumber(h().costUsd)) else nil
							)
						end, {
							key = function(h: any): string
								return tostring(h.harness)
							end,
						}),
					},
					ui.column {
						w_full = true,
						p = "$space.4",
						gap = "$space.2",
						rounded = "$radius.xl",
						bg = material.pick("$zeron.settings.tint", "$zeron.settings.card"),
						border = 1,
						border_color = "$zeron.settings.edge",
						ui.text { ui.t("settings.providers.usage.by_model"), text_size = "$font.size.md", line_height = "$font.line.md", weight = "$font.weight.semibold", color = "$text.primary" },
						ui.each(by_model, function(_m: any, _i: number, row: State<any>): Node
							local function m(): any
								return row:get()
							end
							return ui.row {
								w_full = true,
								items = "center",
								gap = "$space.2",
								py = ui.raw(3, "a model row's breath"),
								ui.text {
									function()
										return tostring(m().model or "unknown")
									end,
									grow = 1,
									min_w = 0,
									text_size = "$font.size.xs",
									line_height = "$font.line.xs",
									color = "$text.secondary",
									truncate = true,
								},
								ui.text {
									function()
										return token_words(tonumber(m().totalTokens) or 0)
									end,
									tabular = true,
									text_size = "$font.size.xs",
									line_height = "$font.line.xs",
									color = "$text.primary",
								},
							}
						end, {
							key = function(m: any): string
								return tostring(m.model)
							end,
						}),
					},
				}),
				ui.text {
					function()
						local started = tonumber(coverage.startedCollectingAt)
						local when = started and provider_card.when_words(started) or ui.t("settings.providers.usage.just_now"):get()
						return ui.t("settings.providers.usage.coverage_note", { when = when }):get()
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
		local to = os.time()
		return agents.usage_summary(target, to - spec.seconds, to, spec.bucket):await()
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
				tokens_view(summary, range, target),
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
