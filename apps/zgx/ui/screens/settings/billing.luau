-- Settings → Billing: the credits that run Cloud chats, the shipping app's
-- Settings → Billing (crates/ui settings/billing.rs) in this design. What's
-- left (this period's plan credits and extra credits that never expire), the
-- monthly plans, and packs of extra credits. Buying and changing a plan
-- happen on Stripe's own pages in the browser; while one is open the page
-- reads the balance again every few seconds until the change lands, then
-- says what changed.
--
-- Listed only while the account has Cloud (screens/settings/init.luau).

local ds = require("@ds")
local billing = require("@account/billing")
local cloud = require("@account/cloud")
local cloud_page = require("@app/screens/settings/cloud")
local search = require("@app/screens/settings/search")
local toast = require("@ds/toast")
local locale = require("@ds/locale")

local M = {}

M.ID = "billing"
M.TITLE = "Billing"

M.ROWS = {
	{ key = "credits", title = ui.t("settings.billing.credits.title"), terms = ui.t("settings.billing.credits.terms") },
	{ key = "plan", title = ui.t("settings.billing.plan.title"), terms = ui.t("settings.billing.plan.terms") },
	{ key = "extra", title = ui.t("settings.billing.extra.title"), terms = ui.t("settings.billing.extra.terms") },
}
M.TERMS = ui.t("settings.billing.terms")

-- How often an open Stripe page is checked on, and for how long.
local POLL_EVERY = 3000
local POLL_FOR = 10 * 60

-- (Indexed for the search by screens/settings/init.luau, while the
-- account has Cloud.)

local group = cloud_page.group
local credits_of = cloud_page.credits

-- "$20" whole dollars, "$12.50" otherwise.
function M.price(cents: number): string
	cents = math.floor(cents + 0.5)
	if cents % 100 == 0 then
		return `${cents // 100}`
	end
	return string.format("$%d.%02d", cents // 100, cents % 100)
end

-- "about 750 hours" of a standard machine.
function M.hours(credits: number): string
	return ui.t("settings.billing.hours", { n = group(math.floor(credits / 60 + 0.5)) }):get()
end

-- "Nov 5", in this Mac's calendar.
function M.date(at: number?): string
	if not at then
		return ""
	end
	return locale.month_day(at)
end

-- What a Stripe page changed, in a sentence, or nil when nothing worth one.
type Snapshot = { total: number, extra: number, plan: string?, cancel: boolean }

function M.snapshot(s: billing.Status): Snapshot
	return {
		total = s.credits.total,
		extra = s.credits.extra,
		plan = if s.plan then s.plan.id else nil,
		cancel = s.plan ~= nil and s.plan.cancel_at_period_end,
	}
end

function M.changed(before: Snapshot, now: Snapshot, s: billing.Status): string?
	if now.plan ~= before.plan then
		if s.plan then
			return ui.t("settings.billing.changed.plan", { plan = s.plan.name, credits = credits_of(s.plan.monthly_credits) }):get()
		end
		return ui.t("settings.billing.changed.ended"):get()
	end
	if now.cancel ~= before.cancel then
		return if now.cancel then ui.t("settings.billing.changed.wont_renew"):get() else ui.t("settings.billing.changed.will_renew"):get()
	end
	local added = now.extra - before.extra
	if added >= 1 then
		return ui.t("settings.billing.changed.added", { credits = credits_of(added) }):get()
	end
	return nil
end

-- What the plan's row says under its name.
function M.plan_line(p: billing.Current): (string, string?)
	if p.status == "past_due" then
		return ui.t("settings.billing.plan_line.past_due"):get(), "warning"
	elseif p.cancel_at_period_end then
		return ui.t("settings.billing.plan_line.ends", { date = M.date(p.period_end) }):get(), nil
	end
	return ui.t("settings.billing.plan_line.renews", { date = M.date(p.period_end) }):get(), nil
end

local function row(key: string, node: Node): Node
	return search.anchor(key, node)
end

local function quiet(text: string | Message | () -> string, props: { [string]: any }?): Node
	local t: { [any]: any } = {
		text,
		text_size = "$font.size.sm",
		line_height = "$font.line.sm",
		color = "$text.tertiary",
	}
	for k, v in props or {} do
		t[k] = v
	end
	return ui.text(t)
end

function M.view(): Node
	local status = ui.state("settings_billing_status", nil :: billing.Status?)
	local failed = ui.state("settings_billing_failed", nil :: string?)
	-- Which button waits on the engine ("plan:<id>", "credits", "portal").
	local busy = ui.state("settings_billing_busy", nil :: string?)
	-- A Stripe page is open: what it may change, as it was, and since when.
	local pending = ui.state("settings_billing_pending", nil :: any)
	local packs = ui.state("settings_billing_packs", 1)

	local function load()
		billing.status():after(function(s: any)
			failed:set(nil)
			local p = pending:peek()
			if p then
				local now = M.snapshot(s)
				local was = p.before :: Snapshot
				if now.total ~= was.total or now.extra ~= was.extra or now.plan ~= was.plan or now.cancel ~= was.cancel then
					local said = M.changed(was, now, s)
					if said then
						toast.show { title = said, tone = "success" }
					end
					pending:set(nil)
					-- (The composer's Cloud choice reads the credits from Cloud.)
					cloud.load()
				end
			end
			status:set(s)
		end, function(e: any)
			failed:set(e.message)
		end)
	end
	load()
	-- While a Stripe page is open, the balance is read again until the
	-- change lands, or for ten minutes. (The tick belongs to the effect's
	-- run, so it stops with it.)
	ui.effect(function()
		local p = pending:get()
		if not p then
			return
		end
		ui.every(POLL_EVERY, function()
			if os.time() - p.since > POLL_FOR then
				pending:set(nil)
				return false
			end
			load()
			return true
		end)
	end)
	local function open(key: string, ask: () -> any)
		if busy:peek() then
			return
		end
		busy:set(key)
		ask():after(function()
			busy:set(nil)
			local s = status:peek()
			if s then
				pending:set { before = M.snapshot(s), since = os.time() }
			end
		end, function(e: any)
			busy:set(nil)
			toast.show {
				title = ui.t("settings.billing.stripe_failed"):get(),
				body = if e.message == "offline" then ui.t("settings.engine.offline"):get() else ui.t("settings.billing.try_later"):get(),
				tone = "danger",
			}
		end)
	end
	local function ready(): boolean
		return status:get() ~= nil
	end
	local function can_buy(): boolean
		local s = status:get()
		return s ~= nil and s.available and busy:get() == nil
	end

	-- The balance: one number and the time it buys, a bar split into plan
	-- and extra credits, then a row for each kind.
	local function credits(): Node
		local function c(): billing.Credits
			local s = status:get()
			return if s then s.credits else { monthly = 0, monthly_total = 0, extra = 0, total = 0 }
		end
		local function share(kind: string): number
			local now = c()
			local monthly, extra = math.max(0, now.monthly), math.max(0, now.extra)
			local all = monthly + extra
			if all <= 0 then
				return 0
			end
			return (if kind == "monthly" then monthly else extra) / all
		end
		local function figure_line(): string
			local total = math.max(0, c().total)
			return if total > 0 then M.hours(total) else ui.t("settings.billing.credits.none"):get()
		end
		local summary = ui.column {
			w_full = true,
			px = "$space.4",
			py = "$space.4",
			gap = "$space.3",
			ui.row {
				w_full = true,
				gap = "$space.3",
				items = "baseline",
				flex_wrap = true,
				ui.row {
					gap = "$space.2",
					items = "baseline",
					grow = 1,
					ui.text {
						function(): string
							return group(math.floor(math.max(0, c().total)))
						end,
						text_size = "$font.size.xxl",
						line_height = "$font.line.xxl",
						weight = "$font.weight.semibold",
						color = "$text.primary",
						tabular = true,
					},
					ui.text {
						function(): string
							return ui.t("settings.billing.credits.unit", { n = math.floor(math.max(0, c().total)) }):get()
						end,
						text_size = "$font.size.md",
						line_height = "$font.line.md",
						color = "$text.secondary",
					},
				},
				quiet(figure_line),
			},
			-- Plan credits, then extra, on one track.
			ui.show(function(): boolean
				return share("monthly") + share("extra") > 0
			end, ui.row {
				w_full = true,
				h = 4,
				rounded = "$radius.full",
				bg = "$zeron.secondary.bg",
				overflow = "hidden",
				role = "img",
				label = function(): string
					return ui.t("settings.billing.credits.split", { plan = group(math.max(0, c().monthly)), extra = group(math.max(0, c().extra)) }):get()
				end,
				ui.div {
					h_full = true,
					w = function(): string
						return `{share("monthly") * 100}%`
					end,
					bg = "$accent",
				},
				ui.div {
					h_full = true,
					w = function(): string
						return `{share("extra") * 100}%`
					end,
					bg = "$zeron.tone.accent.edge",
				},
			}),
		}
		local function amount(text: () -> string): Node
			return ui.text { text, text_size = "$font.size.md", line_height = "$font.line.md", color = "$text.primary", tabular = true }
		end
		return ds.Settings.Section {
			title = ui.t("settings.billing.credits.title"),
			description = ui.t("settings.billing.credits.description"),
			row(
				"credits",
				ds.Settings.Group {
					summary,
					ds.Settings.Row {
						label = ui.t("settings.billing.credits.plan_label"),
						description = function(): string
							local now = c()
							if now.monthly_total <= 0 then
								return ui.t("settings.billing.credits.no_plan"):get()
							elseif now.monthly_ends then
								return ui.t("settings.billing.credits.of_resets", { total = group(now.monthly_total), date = M.date(now.monthly_ends) }):get()
							end
							return ui.t("settings.billing.credits.of", { total = group(now.monthly_total) }):get()
						end,
						control = amount(function(): string
							return if c().monthly_total > 0 then group(math.floor(math.max(0, c().monthly))) else "—"
						end),
					},
					ds.Settings.Row {
						label = ui.t("settings.billing.credits.extra_label"),
						description = ui.t("settings.billing.credits.extra_description"),
						control = amount(function(): string
							return group(math.floor(math.max(0, c().extra)))
						end),
					},
				}
			),
		}
	end

	-- The plans side by side, the one you're on marked; on a plan, switching
	-- and cancelling go through Stripe's portal.
	local function plan_card(p: billing.Plan, i: number): Node
		local function current(): boolean
			local s = status:get()
			return s ~= nil and s.plan ~= nil and s.plan.id == p.id
		end
		local function on_any(): boolean
			local s = status:get()
			return s ~= nil and s.plan ~= nil
		end
		local function label(): string
			if current() then
				return ui.t("settings.billing.plan.current_plan"):get()
			elseif on_any() then
				return ui.t("settings.billing.plan.switch"):get()
			end
			return ui.t("settings.billing.plan.choose", { plan = p.name }):get()
		end
		return ui.column {
			grow = 1,
			basis = 0,
			min_w = 150,
			p = "$space.4",
			gap = "$space.1",
			rounded = "$radius.xl",
			bg = "$zeron.settings.card",
			border = 1,
			border_color = function(): string
				return if current() then "$zeron.selected.ring" else "$zeron.settings.edge"
			end,
			role = "group",
			label = ui.t("settings.billing.plan.card_name", { plan = p.name }),
			ui.row {
				w_full = true,
				items = "center",
				justify = "between",
				h = 20,
				ui.text { p.name, text_size = "$font.size.md", line_height = "$font.line.md", weight = "$font.weight.medium", color = "$text.secondary" },
				ui.show(current, ds.Badge { ui.t("settings.billing.plan.current"), size = "sm", tone = "accent" }),
			},
			ui.row {
				gap = "$space.1",
				items = "baseline",
				pt = "$space.1",
				ui.text { M.price(p.price_cents), text_size = "$font.size.xl", line_height = "$font.line.xl", weight = "$font.weight.semibold", color = "$text.primary", tabular = true },
				quiet(ui.t("settings.billing.plan.per_month")),
			},
			ui.text {
				function(): string
					return ui.t("settings.billing.plan.a_month", { credits = credits_of(p.monthly_credits) }):get()
				end,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = "$text.primary",
				pt = "$space.1",
			},
			quiet(function(): string
				return M.hours(p.monthly_credits)
			end),
			ui.div {
				w_full = true,
				pt = "$space.3",
				ds.Button {
					label,
					tone = if i == 1 then "primary" else "secondary",
					size = "sm",
					w_full = true,
					busy = function(): boolean
						return busy:get() == `plan:{p.id}`
					end,
					disabled = function(): boolean
						return current() or not can_buy()
					end,
					on_click = function()
						if current() then
							return
						elseif on_any() then
							open(`plan:{p.id}`, billing.manage)
						else
							open(`plan:{p.id}`, function()
								return billing.subscribe(p.id)
							end)
						end
					end,
				},
			},
		}
	end

	local function plans(): Node
		local list = ui.computed(function(): { billing.Plan }
			local s = status:get()
			return if s then s.plans else {}
		end)
		local function current(): billing.Current?
			local s = status:get()
			return if s then s.plan else nil
		end
		return ds.Settings.Section {
			title = ui.t("settings.billing.plan.title"),
			description = ui.t("settings.billing.plan.description"),
			row(
				"plan",
				ui.column {
					w_full = true,
					gap = "$space.3",
					ui.each(list, function(p: billing.Plan, i: number): Node
						return plan_card(p, i)
					end, {
						key = function(p: billing.Plan): string
							return p.id
						end,
						w_full = true,
						direction = "row",
						flex_wrap = true,
						gap = "$space.3",
					}),
					ui.show(function(): boolean
						return current() ~= nil
					end, ds.Settings.Group {
						ds.Settings.Row {
							label = function(): string
								local p = current()
								return if p then ui.t("settings.billing.plan.current_row", { plan = p.name, price = M.price(p.price_cents) }):get() else ""
							end,
							description = function(): string
								local p = current()
								if not p then
									return ""
								end
								local line = M.plan_line(p)
								return line
							end,
							control = ds.Button {
								ui.t("settings.billing.plan.manage"),
								tone = "secondary",
								size = "sm",
								busy = function(): boolean
									return busy:get() == "portal"
								end,
								disabled = function(): boolean
									return busy:get() ~= nil
								end,
								on_click = function()
									open("portal", billing.manage)
								end,
								ds.Icon { name = "arrow-up-right", size = 14 },
							},
						},
					}),
				}
			),
		}
	end

	-- Packs of extra credits: how many, then Buy.
	local function extra(): Node
		local function pack(): any
			local s = status:get()
			return if s then s.pack else { price_cents = 0, credits = 0 }
		end
		return ds.Settings.Section {
			title = ui.t("settings.billing.extra.title"),
			description = ui.t("settings.billing.extra.description"),
			row(
				"extra",
				ds.Settings.Group {
					ds.Settings.Row {
						label = function(): string
							return credits_of(pack().credits * packs:get())
						end,
						description = function(): string
							return ui.t("settings.billing.extra.packs", { n = packs:get(), hours = M.hours(pack().credits * packs:get()) }):get()
						end,
						control = ui.row {
							gap = "$space.2",
							items = "center",
							ds.NumberStepper {
								value = packs,
								min = 1,
								max = billing.MAX_PACKS,
								step = 1,
								name = ui.t("settings.billing.extra.packs_name"),
								size = "sm",
							},
							ds.Button {
								function(): string
									return ui.t("settings.billing.extra.buy", { price = M.price(pack().price_cents * packs:get()) }):get()
								end,
								tone = "secondary",
								size = "sm",
								busy = function(): boolean
									return busy:get() == "credits"
								end,
								disabled = function(): boolean
									return not can_buy()
								end,
								on_click = function()
									local n = packs:peek()
									open("credits", function()
										return billing.buy(n)
									end)
								end,
							},
						},
					},
				}
			),
		}
	end

	return ds.Settings.Page {
		title = ui.t("settings.billing.title"),
		description = ui.t("settings.billing.description"),
		ui.show(function(): boolean
			return failed:get() ~= nil and not ready()
		end, ds.Callout {
			tone = "warning",
			title = ui.t("settings.billing.load_failed"),
			body = function(): string
				return if failed:get() == "offline" then ui.t("settings.engine.offline"):get() else ui.t("settings.cloud.unreachable.failed"):get()
			end,
			action = { label = ui.t("common.retry") :: any, on_click = load },
		}),
		ui.show(function(): boolean
			local s = status:get()
			return s ~= nil and not s.available
		end, ds.Callout {
			tone = "info",
			title = ui.t("settings.billing.unavailable.title"),
			body = ui.t("settings.billing.unavailable.body"),
		}),
		ui.show(function(): boolean
			return pending:get() ~= nil
		end, ds.Callout {
			tone = "info",
			title = ui.t("settings.billing.pending.title"),
			body = ui.t("settings.billing.pending.body"),
			action = {
				label = ui.t("common.dismiss") :: any,
				on_click = function()
					pending:set(nil)
				end,
			},
		}),
		ui.show(function(): boolean
			return not ready() and failed:get() == nil
		end, ds.Settings.Group {
			ds.SkeletonRow {},
			ds.SkeletonRow {},
			ds.SkeletonRow {},
		}),
		ui.show(ready, ui.column {
			w_full = true,
			gap = "$space.6",
			credits(),
			plans(),
			extra(),
		}),
	}
end

return M
