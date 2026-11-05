-- An agent on a device and the accounts it signs in with, as Settings →
-- Providers draws it and setup's Your agents embeds it: one card each. The
-- agent, its version, its settings and its switch on the first row; its
-- accounts under it, each with its plan, the one in use marked, its two
-- meters (the session and the week, the windows the providers report) and
-- its actions; what Codex banks for an account, the row after it; and at
-- the foot the way to add one (or the sign-in that is under way or went
-- wrong, in its place). Also the pieces round the cards: an agent still to
-- install, the cards' shape while a device is checked, and a device that
-- couldn't be.
--
--   ProviderCard.Card { provider = { id, name, mark, version, latest? }, accounts = { … } | fn, enabled = state, … }
--
-- A card is built once and changes in place (`CardProps` says how); a list
-- of them is a keyed `ui.each` with row state, as setup's Your agents is.
--
-- It only shows: the page round it holds the agents, their accounts and
-- sign-ins (the engine's, once the app speaks to it), and says what each
-- action does.

local Settings = require("@self/settings")
local Button = require("@self/button")
local IconButton = require("@self/icon_button").IconButton
local Icon = require("@self/icon")
local Mark = require("@self/mark")
local AgentMark = require("@self/agent_mark")
local Avatar = require("@self/avatar")
local Badge = require("@self/badge").Badge
local Tooltip = require("@self/tooltip")
local Menu = require("@self/menu")
local Switch = require("@self/switch")
local Callout = require("@self/banner").Callout
local progress = require("@self/progress")
local skeleton = require("@self/skeleton")
local TextField = require("@self/text_field")
local Links = require("@self/links")
local Dialog = require("@self/dialog").Dialog
local RadioGroup = require("@self/radio")
local PrivateText = require("@self/private_text")
local locale = require("@self/locale")

local M = {}

-- (`mark`: ds.Mark's name for an agent Zeron drives itself; `icon`: an
-- agent from the ACP registry's one-ink SVG, in its place.)
export type Provider = { id: string, name: string, mark: string?, icon: string?, version: string?, latest: string? }
-- A reset Codex banked for an account: what it's called, what it starts
-- over (`scope`), and when it expires (a day's words, or seconds since the
-- epoch).
export type Reset = { id: string, title: string, scope: string?, expires: (string | number)? }
-- How an account's last reset went: "using", "reset" (its limits started
-- over), "nothing" (no window needed it: still banked), "gone" (no longer
-- available) or "failed" (`why`: mismatch, signed_out, missing, offline or
-- failed).
export type ResetWord = { state: string, why: string? }
export type Account = {
	-- What keys its row (the email, without one).
	id: string?,
	email: string,
	plan: string,
	session: number,
	week: number,
	resets_at: string?,
	-- What Codex banks for it (a Codex account alone): how many and which,
	-- the soonest to expire first, and how its last reset went.
	banked: number?,
	resets: { Reset }?,
	reset: ResetWord?,
	-- When each window starts over (seconds since the epoch), if known.
	session_resets: number?,
	week_resets: number?,
}

-- When a window starts over, in this Mac's time, as the shipping app says
-- it (settings/accounts.rs format_reset): a clock time within 22 hours
-- ("resets 3:45 PM"), a weekday within a week ("resets Thu"), else the
-- day ("resets Sep 14").
function M.reset_words(at: number?, now: number?): string?
	local when = M.when_words(at, now)
	if not when then
		return nil
	end
	return locale.s("ds.provider_card.resets_at", { when = when })
end

-- The same time, bare ("3:45 PM", "Thu", "Sep 14").
function M.when_words(at: number?, now: number?): string?
	if not at then
		return nil
	end
	local left = at - (now or os.time())
	return if left < 22 * 3600 then locale.clock(at) elseif left < 7 * 86400 then locale.weekday(at) else locale.month_day(at)
end

-- How far along a plan's window is: the accent until it nears its limit,
-- amber past four fifths, red at the limit (Zeron's thresholds).
local function level(fraction: number): string
	return if fraction >= 0.95 then "danger" elseif fraction >= 0.8 then "warning" else "accent"
end

-- One window's meter: its name, the bar, the percent, on one line. Fixed
-- widths, held, so the meters line up down the list. (`label`: its name's
-- message's key.)
local METER = { label = 52, bar = 88, percent = 34 }

local function meter(label: string, fraction: () -> number): Node
	local function tone(): string
		return level(fraction())
	end
	return ui.row {
		gap = "$space.2",
		items = "center",
		shrink = 0,
		ui.text { locale.t(label), w = METER.label, shrink = 0, text_size = "$font.size.xs", line_height = "$font.line.xs", color = "$text.tertiary" },
		ui.div {
			w = METER.bar,
			shrink = 0,
			progress.ProgressBar {
				value = fraction,
				tone = tone :: any,
				size = "thin",
				w_full = true,
				label = locale.s("ds.provider_card.usage", { label = locale.s(label) }),
			},
		},
		ui.text {
			function()
				return `{math.floor(fraction() * 100 + 0.5)}%`
			end,
			w = METER.percent,
			shrink = 0,
			align = "right",
			tabular = true,
			text_size = "$font.size.xs",
			line_height = "$font.line.xs",
			color = function()
				local t = tone()
				return if t == "accent" then "$text.tertiary" else `$zeron.tone.{t}.ink`
			end,
		},
	}
end

-- A round lead for a row that isn't a person: a plus (add an account), the
-- ring of a sign-in under way, the danger's glyph for one that failed.
function M.disc(inside: Node, tone: string?): Node
	return ui.row {
		w = 32,
		h = 32,
		shrink = 0,
		items = "center",
		justify = "center",
		rounded = "$radius.full",
		bg = if tone then `$zeron.tone.{tone}.wash` else "$zeron.secondary.bg",
		color = if tone then `$zeron.tone.{tone}.ink` else "$text.secondary",
		inside,
	}
end

-- What a card's page does with it (setup's step and Settings share the
-- card): `settings`, whether the agent's settings open from it (Settings
-- only, `on_settings`); `on_add`, what Add account starts (a sign-in);
-- `settle`, what a sign-in's row does when it's cancelled, dismissed or
-- tried again (the foot it should show next: "idle" or "waiting"); an
-- account's actions (switch to it, a banked reset, remove it).
--
-- A card stands while what it shows changes: its version, its accounts
-- (keyed by `id`, else `email`: one arriving or going changes that row
-- alone, and an account's plan and meters change in place), the account in
-- use, its foot and its switch. Give them as functions (or the switch's
-- state) and the card is never built again, so a button just pressed keeps
-- its place and focus, and the page round it its scroll. Plain values show
-- as they are (the gallery's).
export type CardProps = {
	-- The agent: its id, name and mark, fixed for the card.
	provider: Provider,
	-- Its version and a newer one, as they stand (else `provider`'s).
	version: (() -> string?)?,
	latest: (() -> string?)?,
	accounts: { Account } | () -> { Account },
	-- "idle" (the way to add one), "waiting", "code" (a code to paste
	-- back, Claude's when it can't finish in the browser) or "failed".
	foot: (string | () -> string)?,
	enabled: State<boolean>,
	-- The account in use, by its email.
	active: () -> string?,
	settings: boolean?,
	on_settings: (() -> ())?,
	-- Update takes the newer version (`latest`); `updating`, one being
	-- taken, says so in the button's place.
	on_update: (() -> ())?,
	updating: (() -> boolean)?,
	on_add: (() -> ())?,
	-- The code pasted back ("code"), to finish the sign-in with.
	on_code: ((code: string) -> ())?,
	settle: (next: string) -> (),
	on_switch: ((a: Account) -> ())?,
	-- A Codex account's banked resets: use one (the page's dialog asks
	-- which: ResetsDialog), and forget how the last went. Without
	-- `on_reset` the card shows none of it (setup's Your agents).
	on_reset: ((a: Account) -> ())?,
	on_dismiss_reset: ((a: Account) -> ())?,
	on_remove: ((a: Account) -> ())?,
	-- An agent that signs in on its own and keeps no accounts Zeron can
	-- list (one from the ACP registry): its foot is a sign-in, not "Connect
	-- an account".
	sign_in_only: boolean?,
	-- What a sign-in under way says to do (a device code to type in), or
	-- why one failed, in the agent's own words: in the row's line, in place
	-- of the usual one.
	login_note: (() -> string?)?,
	-- A sign-in that failed for want of typed input: whether it can be run
	-- in a terminal instead (`terminal`), and what does so.
	terminal: (() -> boolean)?,
	on_terminal: (() -> ())?,
	-- A sign-in under way that opened a page (where its code is confirmed):
	-- whether there is one, and what opens it again.
	login_page: (() -> boolean)?,
	on_open_page: (() -> ())?,
	-- A device code the sign-in under way showed, to check on that page:
	-- said plainly in the row.
	login_code: (() -> string?)?,
	-- An agent that signs in on its own (`sign_in_only`), as it says it is:
	-- signed in (nil until it's said), as whom when it says, and signing it
	-- out where it offers that.
	signed_in: (() -> boolean?)?,
	signed_in_as: (() -> string?)?,
	can_sign_out: (() -> boolean)?,
	signing_out: (() -> boolean)?,
	on_sign_out: (() -> ())?,
	-- The ways it offers to sign in (once it's said): the first it signs in
	-- itself is the sign-in row; any other is a row of its own, by that
	-- method (`on_sign_in_with`), and one that reads a key from its
	-- environment (kind "env_var") takes the key here (`on_key`).
	login_methods: (() -> { LoginMethod })?,
	on_sign_in_with: ((method: string) -> ())?,
	on_key: ((var: string, value: string) -> ())?,
}

export type LoginMethod = {
	id: string,
	name: string,
	description: string?,
	kind: string,
	var: string?,
	link: string?,
}

local function key(a: Account): string
	return a.id or a.email
end

-- What an account banks, as its rows read it: whether it does (a Codex
-- account, on a page that uses resets), how many, and how the last went.
local function banks(c: CardProps, a: Account): boolean
	return c.on_reset ~= nil and (a.banked ~= nil or a.resets ~= nil)
end
local function count(a: Account): number
	return a.banked or #(a.resets or {})
end
local function word(a: Account): string?
	return if a.reset then a.reset.state else nil
end

-- An account's actions, behind the ⋯ at its end: switch to it, use a
-- banked reset (Codex), or remove it. `account` is its row's state: each
-- action takes the account as it stands.
local Actions = ui.component(function(p: { card: CardProps, account: State<Account> }): Node
	local c, account = p.card, p.account
	local open = ui.use_state(false)
	local trigger = IconButton {
		icon = "more-horizontal",
		-- (Not the address: it may be hidden on screen.)
		label = locale.t("ds.provider_card.account_actions"),
		tooltip = locale.s("ds.provider_card.account_actions"),
		size = "md",
		tone = "quiet",
		on_click = function()
			open:set(not open:peek())
		end,
	}
	-- (The menu places its anchor: the button is not added again.)
	return ui.row {
		shrink = 0,
		ui.menu(Menu.panel {
			open = open,
			anchor = trigger,
			placement = { side = "bottom", align = "end", offset = 4 },
			w = 232,
			ui.column {
				w_full = true,
				p = "$space.1",
				gap = "$zeron.list.gap",
				Menu.Row {
					icon = "check-circle",
					label = locale.s("ds.provider_card.switch"),
					on_click = function()
						open:set(false)
						if c.on_switch then
							c.on_switch(account:peek())
						end
					end,
				},
				ui.show(function()
					local a = account:get()
					return banks(c, a) and count(a) > 0 and word(a) ~= "using"
				end, Menu.Row {
					icon = "refresh",
					label = locale.s("ds.provider_card.use_banked"),
					on_click = function()
						open:set(false)
						if c.on_reset then
							c.on_reset(account:peek())
						end
					end,
				}),
				Menu.Row {
					icon = "trash-bin-minimalistic",
					label = locale.s("ds.provider_card.remove"),
					danger = true,
					on_click = function()
						open:set(false)
						if c.on_remove then
							c.on_remove(account:peek())
						end
					end,
				},
			},
		}),
	}
end)

-- An account: who, on what plan (and when a used-up window comes back),
-- the one in use marked, its two meters and its actions at the end. Every
-- account row has the same shape; what Codex banks for one is the row
-- after it. Read from its row's state, so it changes in place.
local function account(c: CardProps, row: State<Account>): Node
	-- (Its limits just started over: the data has its windows at 0 until a
	-- probe says more.)
	local function reset(): boolean
		local a = row:get()
		return banks(c, a) and word(a) == "reset"
	end
	local function session(): number
		return row:get().session
	end
	local function week(): number
		return row:get().week
	end
	local function detail(): string
		local a = row:get()
		if reset() then
			return locale.s("ds.provider_card.reset_just_now", { plan = a.plan })
		elseif a.resets_at then
			return locale.s("ds.provider_card.limit_reached_at", { plan = a.plan, at = a.resets_at })
		end
		-- A window used up: when it starts over (the later, if both are).
		local at: number? = nil
		if a.week >= 1 and a.week_resets then
			at = a.week_resets
		elseif a.session >= 1 and a.session_resets then
			at = a.session_resets
		end
		local words = M.reset_words(at)
		if words then
			return if a.plan ~= ""
				then locale.s("ds.provider_card.limit_reached_plan", { plan = a.plan, words = words })
				else locale.s("ds.provider_card.limit_reached", { words = words })
		end
		return a.plan
	end
	-- Both windows' resets, over the meters (the shipping app's tooltip).
	local function resets(): string?
		local a = row:get()
		local parts: { string } = {}
		for _, w in { { "ds.provider_card.session", a.session_resets }, { "ds.provider_card.week", a.week_resets } } do
			local words = M.reset_words(w[2] :: number?)
			if words then
				table.insert(parts, locale.s("ds.provider_card.window_resets", { window = locale.s(w[1] :: string), words = words }))
			end
		end
		return if #parts > 0 then table.concat(parts, " · ") else nil
	end
	local function email(): string
		return row:get().email
	end
	return Settings.Row {
		label = email,
		private_label = true,
		description = detail,
		-- (The face is the address's: a new one only if the address changes.)
		lead = ui.presence(email, function(e: string): Node
			return Avatar { seed = e, size = 32, label = locale.t("ds.provider_card.account") }
		end),
		badge = ui.show(function()
			return c.active() == email()
		end, Badge { locale.t("ds.provider_card.in_use"), tone = "accent" }),
		control = ui.row {
			gap = "$space.3",
			items = "center",
			ui.column {
				gap = "$zeron.space.hair",
				-- (Labels alone where no reset is known, as the shipping app.)
				tooltip = Tooltip {
					function(): string
						return resets() or locale.s("ds.provider_card.windows")
					end,
				},
				meter("ds.provider_card.session", session),
				meter("ds.provider_card.week", week),
			},
			Actions { card = c, account = row },
		},
	}
end

-- Why a reset couldn't be used, said on its row.
local WHY: { [string]: string } = {
	mismatch = "ds.provider_card.reset_why.mismatch",
	signed_out = "ds.provider_card.reset_why.signed_out",
	missing = "ds.provider_card.reset_why.missing",
	offline = "ds.provider_card.reset_why.offline",
	failed = "ds.provider_card.reset_why.failed",
}

-- What Codex banks for an account, the row after it: how many resets, and,
-- once a limit is hit, the one button that spends one now instead of
-- waiting. Then, in the same row, the reset under way and how it went: the
-- count left (the account's line says its limits started over), still
-- banked (no window needed it), no longer there, or why it couldn't be
-- used, with another go. Only Codex accounts have one.
local function banked(c: CardProps, row: State<Account>): Node
	local function state(): string?
		return word(row:get())
	end
	-- A window used up (the account's line says "Limit reached"), a reset
	-- to spend, and none under way or just used.
	local function at_limit(): boolean
		local a = row:get()
		local s = word(a)
		return s ~= "using" and s ~= "reset" and s ~= "failed" and count(a) > 0 and math.max(a.session, a.week) >= 1
	end
	-- When the used-up window starts over, bare (the later, if both are).
	local function until_when(): string?
		local a = row:get()
		if a.resets_at then
			return a.resets_at
		end
		local at = if a.week >= 1 and a.week_resets then a.week_resets elseif a.session >= 1 and a.session_resets then a.session_resets else nil
		return M.when_words(at)
	end
	-- Which disc leads: a reset under way, one that failed, a limit hit,
	-- or the plain one.
	local function lead(): string
		local s = state()
		if s == "using" then
			return "using"
		elseif s == "failed" then
			return "danger"
		end
		return if at_limit() then "warning" else "plain"
	end
	local function use()
		if c.on_reset then
			c.on_reset(row:peek())
		end
	end
	return Settings.Row {
		label = function()
			if state() == "failed" then
				return locale.s("ds.provider_card.reset_failed")
			end
			return locale.s("ds.provider_card.banked", { n = count(row:get()) })
		end,
		description = function()
			local a = row:get()
			local s = word(a)
			if s == "using" then
				return locale.s("ds.provider_card.reset_using")
			elseif s == "failed" then
				local why = if a.reset then a.reset.why else nil
				return locale.s(WHY[why or "failed"] or WHY.failed)
			elseif s == "nothing" then
				return locale.s("ds.provider_card.reset_nothing")
			elseif s == "gone" then
				return locale.s("ds.provider_card.reset_gone")
			elseif s == "reset" and count(a) == 0 then
				return locale.s("ds.provider_card.reset_last")
			elseif at_limit() then
				local when = until_when()
				return if when then locale.s("ds.provider_card.banked_now", { at = when }) else locale.s("ds.provider_card.banked_now_soon")
			end
			return locale.s("ds.provider_card.banked_kept")
		end,
		lead = ui.presence(lead, function(kind: string): Node
			if kind == "using" then
				return M.disc(progress.ProgressRing { size = 16, tone = "neutral", label = locale.s("ds.provider_card.reset_using") })
			elseif kind == "danger" then
				return M.disc(Icon { name = "danger-triangle", size = 16 }, "danger")
			end
			return M.disc(Icon { name = "refresh", size = 16 }, if kind == "warning" then "warning" else nil)
		end),
		-- (Each button stands while it shows: one just pressed keeps its
		-- place.)
		control = ui.row {
			gap = "$space.2",
			items = "center",
			ui.show(function()
				local s = state()
				return s == "nothing" or s == "gone" or s == "failed"
			end, Button {
				locale.t("ds.provider_card.dismiss"),
				tone = "ghost",
				size = "sm",
				on_click = function()
					if c.on_dismiss_reset then
						c.on_dismiss_reset(row:peek())
					end
				end,
			}),
			ui.show(function()
				return state() == "failed"
			end, Button { locale.t("ds.provider_card.try_again"), tone = "secondary", size = "sm", on_click = use }),
			ui.show(at_limit, Button { locale.t("ds.provider_card.use_reset"), tone = "secondary", size = "sm", on_click = use }),
		},
	}
end

-- A rule between rows, as Settings.Group draws one: an account row brings
-- its own, so the rows a list adds and takes away keep theirs.
local function rule(): Node
	return ui.div { h = 1, w_full = true, shrink = 0, bg = "$zeron.settings.rule", aria_hidden = true }
end

-- A key an agent reads from its environment, pasted: kept in its
-- environment on its device (`on_key`), never shown again.
local function key_row(provider: Provider, method: LoginMethod, c: CardProps): Node
	return ui.component(function(): Node
		local open = ui.use_state(false)
		local key = ui.use_state("")
		local var = method.var or ""
		local function save()
			local value = string.gsub(key:peek(), "^%s*(.-)%s*$", "%1")
			if value ~= "" and c.on_key then
				c.on_key(var, value)
				key:set("")
				open:set(false)
			end
		end
		return ui.presence(function(): boolean
			return open:get()
		end, function(shown: boolean): Node
			if not shown then
				return Settings.Row {
					label = locale.t("ds.provider_card.use_key", { method = method.name }),
					description = locale.t("ds.provider_card.use_key_hint", { name = provider.name, var = var }),
					lead = M.disc(Icon { name = "keyboard", size = 16 }),
					on_click = function()
						open:set(true)
					end,
				}
			end
			return Settings.Row {
				label = locale.t("ds.provider_card.paste_key", { method = method.name }),
				description = locale.t("ds.provider_card.paste_key_hint", { name = provider.name, var = var }),
				lead = M.disc(Icon { name = "keyboard", size = 16 }),
				stacked = true,
				control = ui.row {
					w_full = true,
					gap = "$space.2",
					items = "center",
					ui.div {
						grow = 1,
						min_w = 0,
						TextField {
							value = key,
							name = locale.t("ds.provider_card.key_name", { var = var }),
							placeholder = var,
							password = true,
							size = "sm",
							autofocus = true,
							on_commit = function()
								save()
							end,
						},
					},
					if method.link
						then Button {
							locale.t("ds.provider_card.get_key"),
							tone = "ghost",
							size = "sm",
							on_click = function()
								Links.open(method.link :: string)
							end,
						}
						else nil,
					Button {
						locale.t("ds.provider_card.cancel"),
						tone = "ghost",
						size = "sm",
						on_click = function()
							key:set("")
							open:set(false)
						end,
					},
					Button {
						locale.t("ds.provider_card.save_key"),
						tone = "secondary",
						size = "sm",
						disabled = function(): boolean
							return string.match(key:get(), "%S") == nil
						end,
						on_click = save,
					},
				},
			}
		end)
	end)()
end

-- An agent that signs in on its own, by the ways it offers: `first` (its
-- sign-in row) for the first it signs in itself, then a row for each
-- other way (by its name), a key for one that reads a key. (Rebuilt only
-- when the ways it offers change.)
local function method_rows(provider: Provider, c: CardProps, first: Node): Node
	local function methods(): { LoginMethod }
		return if c.login_methods then c.login_methods() else {}
	end
	return ui.presence(function(): string
		local ids = {}
		for _, m in methods() do
			table.insert(ids, `{m.kind}:{m.id}`)
		end
		return table.concat(ids, "\0")
	end, function(_key: string): Node
		local list = methods()
		local rows: { Node } = {}
		local first_used = false
		local signs_in = false
		for _, m in list do
			if m.kind ~= "env_var" then
				signs_in = true
			end
		end
		if signs_in or #list == 0 then
			table.insert(rows, first)
		end
		for _, m in list do
			if m.kind == "env_var" then
				if m.var then
					table.insert(rows, rule())
					table.insert(rows, key_row(provider, m, c))
				end
			elseif not first_used then
				first_used = true
			else
				table.insert(rows, rule())
				table.insert(rows, Settings.Row {
					label = m.name,
					description = m.description,
					lead = M.disc(Icon { name = "lock-keyhole-unlocked", size = 16 }),
					on_click = function()
						if c.on_sign_in_with then
							c.on_sign_in_with(m.id)
						end
					end,
				})
			end
		end
		return ui.column { w_full = true, table.unpack(rows) }
	end)
end

-- The card's foot: connect the first account, or add another (Zeron's
-- labels), leading into the sign-in.
local function add(provider: Provider, empty: () -> boolean, on_add: (() -> ())?, sign_in_only: boolean?, c: CardProps?): Node
	if sign_in_only then
		local function sign_in(): Node
			local row = Settings.Row {
				label = locale.t("ds.provider_card.sign_in", { name = provider.name }),
				description = locale.t("ds.provider_card.sign_in_hint"),
				lead = M.disc(Icon { name = "lock-keyhole-unlocked", size = 16 }),
				on_click = on_add or function() end,
			}
			if not (c and c.login_methods) then
				return row
			end
			return method_rows(provider, c :: CardProps, row)
		end
		if not (c and c.signed_in) then
			return sign_in()
		end
		local card = c :: CardProps
		-- Signed in, as it says (it was before you asked, or a sign-in just
		-- finished): who as, and the way out.
		return ui.presence(function(): string
			return if (card.signed_in :: any)() == true then "in" else "out"
		end, function(state: string): Node
			if state ~= "in" then
				return sign_in()
			end
			return Settings.Row {
				label = locale.t("ds.provider_card.signed_in", { name = provider.name }),
				description = function(): string
					local who = if card.signed_in_as then card.signed_in_as() else nil
					return if who
						then locale.s("ds.provider_card.signed_in_as", { who = who })
						else locale.s("ds.provider_card.signed_in_hint")
				end,
				lead = M.disc(Icon { name = "check", size = 16 }),
				control = ui.show(function(): boolean
					return card.can_sign_out ~= nil and card.can_sign_out()
				end, Button {
					locale.t("ds.provider_card.sign_out"),
					tone = "secondary",
					size = "sm",
					busy = function(): boolean
						return card.signing_out ~= nil and card.signing_out()
					end,
					on_click = function()
						if card.on_sign_out then
							card.on_sign_out()
						end
					end,
				}),
			}
		end)
	end
	return Settings.Row {
		label = function()
			return if empty() then locale.s("ds.provider_card.connect", { name = provider.name }) else locale.s("ds.provider_card.add_account")
		end,
		lead = M.disc(Icon { name = "plus", size = 16 }),
		on_click = on_add or function() end,
	}
end

-- A sign-in under way, in the row the account will take: the browser has
-- the rest of it.
local function waiting(provider: Provider, settle: (next: string) -> (), note: (() -> string?)?, c: CardProps?): Node
	local function page(): boolean
		return c ~= nil and c.login_page ~= nil and c.login_page()
	end
	local function code(): string?
		return if c and c.login_code then c.login_code() else nil
	end
	return Settings.Row {
		label = locale.t("ds.provider_card.signing_in_to", { name = provider.name }),
		-- (With a code, the line says what to do with it; the code is said
		-- on its own, beside the buttons.)
		description = if note
			then function(): string
				if code() then
					return locale.s("ds.provider_card.code_check")
				end
				return note() or locale.s("ds.provider_card.signing_in_hint")
			end
			else locale.t("ds.provider_card.signing_in_hint"),
		lead = M.disc(progress.ProgressRing { size = 16, tone = "neutral", label = locale.s("ds.provider_card.signing_in") }),
		control = ui.row {
			gap = "$space.2",
			items = "center",
			ui.show(function(): boolean
				return code() ~= nil
			end, ui.text {
				function(): string
					return code() or ""
				end,
				id = "pairing-code",
				font = "$font.mono",
				text_size = "$font.size.lg",
				weight = "$font.weight.semibold",
				color = "$text.primary",
				selectable = true,
			}),
			Button {
				locale.t("ds.provider_card.cancel"),
				tone = "ghost",
				size = "sm",
				on_click = function()
					settle("idle")
				end,
			},
			-- (A tab closed, a browser that didn't come forward.)
			ui.show(page, Button {
				locale.t("ds.provider_card.open_page"),
				tone = "secondary",
				size = "sm",
				on_click = function()
					if c and c.on_open_page then
						c.on_open_page()
					end
				end,
			}),
		},
	}
end

-- A sign-in the browser can't finish on its own: the page it opened
-- shows a code once you've signed in, pasted back here.
local function code_row(provider: Provider, settle: (next: string) -> (), on_code: ((code: string) -> ())?): Node
	return ui.component(function(): Node
		local code = ui.use_state("")
		local function finish()
			local c = string.gsub(code:peek(), "^%s*(.-)%s*$", "%1")
			if c ~= "" and on_code then
				on_code(c)
			end
		end
		return Settings.Row {
			label = locale.t("ds.provider_card.paste_code"),
			description = locale.t("ds.provider_card.paste_code_hint", { name = provider.name }),
			lead = M.disc(Icon { name = "keyboard", size = 16 }),
			stacked = true,
			control = ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				ui.div {
					grow = 1,
					min_w = 0,
					TextField {
						value = code,
						name = locale.t("ds.provider_card.code_name"),
						placeholder = locale.t("ds.provider_card.code"),
						size = "sm",
						autofocus = true,
						on_commit = function()
							finish()
						end,
					},
				},
				Button {
					locale.t("ds.provider_card.cancel"),
					tone = "ghost",
					size = "sm",
					on_click = function()
						settle("idle")
					end,
				},
				Button {
					locale.t("ds.provider_card.finish"),
					tone = "secondary",
					size = "sm",
					disabled = function(): boolean
						return string.match(code:get(), "%S") == nil
					end,
					on_click = finish,
				},
			},
		}
	end)()
end

-- A sign-in that failed, in the same row: what happened, and another go.
local function failed(provider: Provider, settle: (next: string) -> (), note: (() -> string?)?, c: CardProps?): Node
	local function terminal(): boolean
		return c ~= nil and c.terminal ~= nil and c.terminal()
	end
	return Settings.Row {
		label = locale.t("ds.provider_card.failed", { name = provider.name }),
		description = if note
			then function(): string
				return note() or locale.s("ds.provider_card.failed_hint")
			end
			else locale.t("ds.provider_card.failed_hint"),
		danger = true,
		lead = M.disc(Icon { name = "danger-triangle", size = 16 }, "danger"),
		control = ui.row {
			gap = "$space.2",
			items = "center",
			Button {
				locale.t("ds.provider_card.dismiss"),
				tone = "ghost",
				size = "sm",
				on_click = function()
					settle("idle")
				end,
			},
			ui.show(function()
				return not terminal()
			end, Button {
				locale.t("ds.provider_card.try_again"),
				tone = "secondary",
				size = "sm",
				on_click = function()
					settle("waiting")
				end,
			}),
			-- (It asks for typed input: its sign-in, in a terminal.)
			ui.show(terminal, Button {
				locale.t("ds.provider_card.open_terminal"),
				tone = "secondary",
				size = "sm",
				on_click = function()
					if c and c.on_terminal then
						c.on_terminal()
					end
				end,
			}),
		},
	}
end

-- An installed agent's card.
function M.Card(c: CardProps): Node
	local provider = c.provider
	local function accounts(): { Account }
		local given = c.accounts
		return if type(given) == "function" then given() else given
	end
	local function version(): string?
		return if c.version then c.version() else provider.version
	end
	local function latest(): string?
		return if c.latest then c.latest() else provider.latest
	end
	-- The agent's row: (its version, as the agent reports it; none yet,
	-- nothing said).
	local head = Settings.Row {
		label = provider.name,
		description = function()
			local v, l = version(), latest()
			if not v then
				return ""
			end
			return if l then locale.s("ds.provider_card.version_available", { v = v, latest = l }) else `v{v}`
		end,
		lead = AgentMark { label = provider.name, seed = provider.id, mark = provider.mark, icon = provider.icon, size = 32 },
		control = ui.row {
			gap = "$space.3",
			items = "center",
			ui.show(function()
				return latest() ~= nil or (c.updating ~= nil and c.updating())
			end, Button {
				locale.t("ds.provider_card.update"),
				tone = "secondary",
				size = "sm",
				busy = function(): boolean
					return c.updating ~= nil and c.updating()
				end,
				on_click = c.on_update,
			}),
			if c.settings ~= false
				then IconButton {
					icon = "settings-minimalistic",
					label = locale.t("ds.provider_card.settings", { name = provider.name }),
					tooltip = locale.s("ds.provider_card.settings", { name = provider.name }),
					size = "md",
					tone = "quiet",
					on_click = c.on_settings,
				}
				else nil,
			Switch { value = c.enabled, label = locale.t("ds.provider_card.enabled", { name = provider.name }) :: any },
		},
	}
	-- Its accounts, a row each, keyed: one arriving or going is that row
	-- alone, and the rest change in place. Each brings the rule above it
	-- (and above what Codex banks for it).
	local list = ui.each(ui.computed(accounts), function(_a: Account, _i: number, row: State<Account>): Node
		return ui.column {
			w_full = true,
			rule(),
			account(c, row),
			-- (None banked: no row, unless how the last went is still said.)
			ui.show(function()
				local a = row:get()
				return banks(c, a) and (count(a) > 0 or word(a) ~= nil)
			end, ui.column { w_full = true, rule(), banked(c, row) }),
		}
	end, { key = key })
	-- The foot: the way to add one, or the sign-in in its place, swapped
	-- as the sign-in starts and ends (the card round it stays).
	local function foot(kind: string?): Node
		if kind == "waiting" then
			return waiting(provider, c.settle, c.login_note, c)
		elseif kind == "code" then
			return code_row(provider, c.settle, c.on_code)
		elseif kind == "failed" then
			return failed(provider, c.settle, c.login_note, c)
		end
		return add(provider, function()
			return #accounts() == 0
		end, c.on_add, c.sign_in_only, c)
	end
	local given = c.foot
	local kind: () -> string = if type(given) == "function"
		then given
		else function()
			return (given :: string?) or "idle"
		end
	return Settings.Group {
		ui.column { w_full = true, head, list },
		ui.presence(kind, foot, { w_full = true }),
	}
end

-- An agent not on the device: its mark and name, and Install (Zeron's
-- own installer), or, while `installing` says so, the install under way in
-- the button's place; one Zeron can't install (`manual`) says so instead.
function M.InstallRow(p: { name: string, mark: string, manual: boolean? }, installing: (() -> boolean)?, on_install: (() -> ())?): Node
	local function busy(): boolean
		return installing ~= nil and installing()
	end
	local control: Node? = nil
	if not p.manual then
		control = ui.row {
			items = "center",
			ui.show(busy, ui.row {
				gap = "$space.2",
				items = "center",
				color = "$text.tertiary",
				progress.ProgressRing { size = 12, tone = "neutral", label = locale.s("ds.provider_card.installing_named", { name = p.name }) },
				ui.text { locale.t("ds.provider_card.installing"), text_size = "$font.size.sm", line_height = "$font.line.sm" },
			}),
			ui.show(function()
				return not busy()
			end, Button { locale.t("ds.provider_card.install"), tone = "secondary", size = "sm", on_click = on_install }),
		}
	end
	return Settings.Row {
		label = p.name,
		description = if p.manual then locale.t("ds.provider_card.manual", { name = p.name }) else nil,
		lead = Mark { name = p.mark :: any, size = 32 },
		control = control,
	}
end

-- What shows while a device is checked for agents: the cards' shape.
function M.Loading(): Node
	return Settings.Group {
		skeleton.Row { trail = 40 },
		skeleton.Row { title = 150, line = 60, trail = 220 },
		skeleton.Row { title = 170, line = 30, trail = 220 },
		skeleton.Row { title = 110, line = 0, trail = 14 },
	}
end

-- A device that couldn't be checked: what still works, and another go.
function M.Unavailable(where: () -> string, retry: () -> ()): Node
	return Callout {
		tone = "danger",
		title = function()
			return locale.s("ds.provider_card.unavailable", { where = where() })
		end,
		body = function(): string
			return locale.s("ds.provider_card.unavailable_hint")
		end,
		action = { label = locale.s("ds.provider_card.try_again"), on_click = retry },
	}
end

-- Using one of an account's banked resets, in a dialog: which (the soonest
-- to expire first, chosen as it opens), what it does, and the one button
-- that spends it. Codex's own rule, said once: a reset is spent only if a
-- window needed it. How it goes lands on the account's row.
--
--   ResetsDialog { open = state, account = fn, choice = state, on_use = fn(account, reset_id) }
--
-- `account`, the account it's for as it stands (nil: none).
export type ResetsDialogProps = {
	open: State<boolean>,
	account: () -> Account?,
	choice: State<string>,
	on_use: (a: Account, reset: string) -> (),
}

local function expiry(r: Reset): string?
	local e = r.expires
	if type(e) == "number" then
		return locale.month_day(e)
	end
	return e
end

function M.ResetsDialog(p: ResetsDialogProps): Node
	local function resets(): { Reset }
		local a = p.account()
		return if a then a.resets or {} else {}
	end
	-- Opening, the soonest is chosen; while open, the one chosen stays
	-- unless it's gone from the list.
	local was_open = p.open:peek()
	ui.effect(function()
		local open, list = p.open:get(), resets()
		local opening = open and not was_open
		was_open = open
		if not open then
			return
		end
		local chosen = p.choice:peek()
		if not opening then
			for _, r in list do
				if r.id == chosen then
					return
				end
			end
		end
		local first = list[1]
		local next = if first then first.id else ""
		if next ~= chosen then
			p.choice:set(next)
		end
	end)
	-- The choices, made as it opens from the resets then (a reset gone
	-- since is Codex's to say: "no longer available", on the row).
	local function choices(): Node
		local list = resets()
		if #list == 0 then
			return ui.text {
				locale.t("ds.provider_card.resets_unlisted"),
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = "$text.tertiary",
				wrap = true,
			}
		end
		local options: { any } = {}
		for i, r in list do
			local when = expiry(r)
			local label: any = r.title
			if when then
				label = locale.t(if i == 1 and #list > 1 then "ds.provider_card.reset_option_soonest" else "ds.provider_card.reset_option", { title = r.title, expires = when })
			end
			table.insert(options, { value = r.id, label = label, description = r.scope })
		end
		return RadioGroup { value = p.choice, label = locale.t("ds.provider_card.resets_label"), options = options }
	end
	return Dialog {
		open = p.open,
		title = locale.t("ds.provider_card.resets_title"),
		-- (The address only while addresses show.)
		body = function(): string
			local a = p.account()
			if not a or PrivateText.hidden:get() then
				return locale.s("ds.provider_card.resets_body_hidden")
			end
			return locale.s("ds.provider_card.resets_body", { email = a.email })
		end,
		size = "md",
		confirm = {
			label = locale.t("ds.provider_card.use_reset_confirm"),
			on_click = function()
				local a, chosen = p.account(), p.choice:peek()
				if a and chosen ~= "" then
					p.on_use(a, chosen)
				end
			end,
		},
		build = choices,
	}
end

return M
