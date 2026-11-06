-- Settings → Cloud: chats that run on machines of their own, in your
-- GitHub repositories, the shipping app's Settings → Cloud (crates/ui
-- settings/cloud.rs) in this design. Cloud is on for every account that has
-- it (set up as it's first read), so there's no switch: the credits Cloud used,
-- day by day, over a range; GitHub, the way Cloud clones and pushes (the
-- block setup's Connect GitHub shows, ds.GitHubBlock); and the repositories
-- Cloud can open, chosen on GitHub; and the size of machine new chats start on.
-- Billing has its own page.
--
-- Listed only while the account has Cloud (screens/settings/init.luau).
-- Cloud's status and repositories are the account's (account/cloud), read
-- as the page opens; GitHub's device flow is account/github's.

local ds = require("@ds")
local cloud = require("@account/cloud")
local github = require("@account/github")
local search = require("@app/screens/settings/search")
local machine_logic = require("@app/screens/chat/cloud_machine")

local M = {}

M.ID = "cloud"
M.TITLE = "Cloud"

M.ROWS = {
	{ key = "usage", title = ui.t("settings.cloud.usage.title"), terms = ui.t("settings.cloud.usage.terms") },
	{ key = "machine", title = ui.t("settings.cloud.machine.title"), terms = ui.t("settings.cloud.machine.terms") },
	{ key = "github", title = "GitHub", terms = ui.t("settings.cloud.github.terms") },
	{ key = "repos", title = ui.t("settings.cloud.repos.title"), terms = ui.t("settings.cloud.repos.terms") },
}
M.TERMS = ui.t("settings.cloud.terms")

-- The ranges usage is shown over, as the shipping app's (a select's
-- options, read in the locale shown).
function M.RANGES(): { { value: string, label: string } }
	return {
		{ value = "7d", label = ui.t("settings.cloud.range.7d"):get() },
		{ value = "30d", label = ui.t("settings.cloud.range.30d"):get() },
		{ value = "month", label = ui.t("settings.cloud.range.month"):get() },
		{ value = "last_month", label = ui.t("settings.cloud.range.last_month"):get() },
		{ value = "90d", label = ui.t("settings.cloud.range.90d"):get() },
	}
end

local DAY = 86400

local function ymd(t: number): string
	return os.date("!%Y-%m-%d", math.floor(t)) :: any
end

-- `[from, to]`, inclusive UTC days, for the range on `now` (Unix seconds).
function M.days(range: string, now: number): (string, string)
	local today = math.floor(now / DAY) * DAY
	local function back(n: number): string
		return ymd(today - (n - 1) * DAY)
	end
	local t = os.date("!*t", today) :: any
	local month_start = today - (t.day - 1) * DAY
	if range == "7d" then
		return back(7), ymd(today)
	elseif range == "90d" then
		return back(90), ymd(today)
	elseif range == "month" then
		return ymd(month_start), ymd(today)
	elseif range == "last_month" then
		local last_end = month_start - DAY
		local l = os.date("!*t", last_end) :: any
		return ymd(last_end - (l.day - 1) * DAY), ymd(last_end)
	end
	return back(30), ymd(today)
end

-- "45,000": whole, with thousands separators (negative kept).
function M.group(n: number): string
	local digits = tostring(math.floor(math.abs(n) + 0.5))
	local out = string.reverse((string.gsub(string.reverse(digits), "(%d%d%d)", "%1,")))
	if string.sub(out, 1, 1) == "," then
		out = string.sub(out, 2)
	end
	return if n < 0 and out ~= "0" then `-{out}` else out
end

-- "1,240 credits": whole credits (a minute each), a decimal only under ten
-- so a short session still reads as something.
function M.credits(n: number): string
	local v = math.abs(n)
	local number = if v > 0 and v < 10 and v % 1 >= 0.05 then string.format("%.1f", v) else M.group(v)
	local args = { number = `{if n < 0 then "-" else ""}{number}` }
	return if number == "1" then ui.t("settings.cloud.credit", args):get() else ui.t("settings.cloud.credits", args):get()
end

-- "Oct 5" for a `YYYY-MM-DD` day.
function M.short_day(day: string): string
	local _, m, d = string.match(day, "^(%d+)-(%d+)-(%d+)$")
	if not m then
		return day
	end
	return ui.t("time.month_day", { m = tostring(tonumber(m)), day = tostring(tonumber(d)) }):get()
end

-- (Indexed for the search by screens/settings/init.luau, while the
-- account has Cloud.)

local function row(key: string, node: Node): Node
	return search.anchor(key, node)
end

-- A day's bar: its height the day's share of the busiest day's; a day that
-- ran stays visible beside a busy one; a day that didn't, a hairline.
local CHART_H = 88

local function bar(now: State<any>, peak: () -> number, width: () -> number): Node
	local function credits(): number
		return now:get().credits or 0
	end
	return ui.column {
		w = width,
		shrink = 0,
		h_full = true,
		justify = "end",
		role = "img",
		label = function(): string
			return ui.t("settings.cloud.bar.label", { day = M.short_day(now:get().day), credits = M.credits(credits()) }):get()
		end,
		tooltip = ds.Tooltip { function(): string
			return ui.t("settings.cloud.bar.tooltip", { day = M.short_day(now:get().day), credits = M.credits(credits()) }):get()
		end },
		ui.div {
			w_full = true,
			h = function(): number
				local c, p = credits(), peak()
				if c <= 0 or p <= 0 then
					return 2
				end
				return math.max(CHART_H * 0.04, CHART_H * c / p)
			end,
			rounded = "$radius.sm",
			bg = function(): string
				return if credits() > 0 then "$accent" else "$zeron.secondary.bg"
			end,
		},
	}
end

-- The range usage is shown over (kept over a reload, and a visit).
M.range = ui.state("settings_cloud_range", "30d")

local function usage(): Node
	local range = M.range
	-- The usage over the range: what was asked, and its answer.
	local data = ui.state("settings_cloud_usage", nil :: any)
	local failed = ui.state("settings_cloud_usage_failed", false)
	local function load()
		local r = range:peek()
		local from, to = M.days(r, os.time())
		failed:set(false)
		cloud.usage(from, to):after(function(u: any)
			if range:peek() == r then
				data:set(if u and u.available then u else nil)
			end
		end, function()
			if range:peek() == r then
				failed:set(data:peek() == nil)
			end
		end)
	end
	-- Read as the page opens and as the range changes (the bars keep the
	-- last range's until the new ones land).
	ui.effect(function()
		range:get()
		load()
	end)
	local days = ui.computed(function(): { any }
		local u = data:get()
		return if u then u.days or {} else {}
	end)
	local function peak(): number
		local p = 0
		for _, d in days:get() do
			p = math.max(p, d.credits or 0)
		end
		return p
	end
	-- Each day's share of the chart's width, as it's measured: the days
	-- spread edge to edge, the gaps between them the token's.
	local chart_w = ui.use_state(0)
	local GAP = (ui.theme.get("zeron.space.hair") :: number?) or 2
	local function width(): number
		local n = #days:get()
		if n == 0 then
			return 0
		end
		return math.max(1, (chart_w:get() - GAP * (n - 1)) / n)
	end
	local function out(): boolean
		local u = data:get()
		return u ~= nil and u.balance <= 0
	end
	return ds.Settings.Section {
		title = ui.t("settings.cloud.usage.title"),
		description = ui.t("settings.cloud.usage.description"),
		row(
			"usage",
			ds.Settings.Group {
				ui.column {
					w_full = true,
					ds.Settings.Row {
						label = function(): string
							local u = data:get()
							return if u
								then ui.t("settings.cloud.usage.used", { credits = M.credits(u.credits) }):get()
								elseif failed:get() then ui.t("settings.cloud.usage.title"):get()
								else ui.t("settings.cloud.usage.reading"):get()
						end,
						description = function(): string
							local u = data:get()
							if failed:get() then
								return ui.t("settings.cloud.usage.failed"):get()
							elseif not u then
								return " "
							elseif out() then
								return ui.t("settings.cloud.usage.out"):get()
							end
							return ui.t("settings.cloud.usage.left", { credits = M.credits(u.balance) }):get()
						end,
						control = ds.Select {
							value = range,
							options = M.RANGES,
							name = ui.t("settings.cloud.usage.range_name"),
							size = "sm",
							w = 140,
						},
					},
					ui.column {
						w_full = true,
						px = "$space.4",
						pb = "$space.4",
						gap = "$space.2",
						ui.row {
							w_full = true,
							h = CHART_H,
							items = "end",
							on_resize = function(size: any)
								if math.abs(chart_w:peek() - size.w) >= 0.5 then
									chart_w:set(size.w)
								end
							end,
							ui.each(days, function(_d: any, _i: number, now: State<any>): Node
								return bar(now, peak, width)
							end, {
								key = function(d: any): string
									return d.day
								end,
								w_full = true,
								h_full = true,
								grow = 1,
								direction = "row",
								items = "end",
								gap = "$zeron.space.hair",
							}),
						},
						ui.row {
							w_full = true,
							justify = "between",
							ui.text {
								function(): string
									local list = days:get()
									return if list[1] then M.short_day(list[1].day) else ""
								end,
								text_size = "$font.size.xs",
								line_height = "$font.line.xs",
								color = "$text.tertiary",
							},
							ui.text {
								function(): string
									local list = days:get()
									return if #list > 0 then M.short_day(list[#list].day) else ""
								end,
								text_size = "$font.size.xs",
								line_height = "$font.line.xs",
								color = "$text.tertiary",
							},
						},
					},
				},
			}
		),
	}
end

local MachineChoice = ui.component(function(): Node
	local value = ui.use_state(cloud.machine())
	ui.effect(function()
		local now = cloud.machine()
		if value:peek() ~= now then
			value:set(now)
		end
	end)
	return ds.RadioGroup {
		value = value,
		label = ui.t("settings.cloud.machine.title"),
		options = machine_logic.default_options(cloud.machines()),
		on_change = function(id: string)
			cloud.set_machine(id)
		end,
	}
end)

-- The size new chats start on: every size the account's offered, each with
-- its specs and rate. (An edge that offers no choice: no section.)
local function default_machine(): Node
	local function ids(): string
		local out = {}
		for _, m in cloud.machines() do
			table.insert(out, m.id)
		end
		return table.concat(out, ",")
	end
	return ui.show(function(): boolean
		return ids() ~= ""
	end, ds.Settings.Section {
		title = ui.t("settings.cloud.machine.title"),
		description = ui.t("settings.cloud.machine.description"),
		row(
			"machine",
			ds.Settings.Group {
				ui.column {
					w_full = true,
					px = "$space.4",
					py = "$space.3",
					gap = "$space.2",
					-- (Its options are fixed as it's built: built again as the sizes
					-- on offer change.)
					ui.presence(ids, function(): Node
						return MachineChoice {}
					end, { w_full = true }),
					ui.show(function(): boolean
						return cloud.now:get().machine_failure ~= nil
					end, ui.text {
						ui.t("settings.cloud.machine.failed"),
						wrap = true,
						text_size = "$font.size.sm",
						line_height = "$font.line.sm",
						color = "$zeron.tone.danger.ink",
					}),
				},
			}
		),
	})
end

local function github_now(): github.Now
	return github.now:get() :: github.Now
end

-- GitHub's block: the phase it shows, as setup's step reads it.
local function github_phase(): any
	local p = github_now().phase
	if p == "checking" then
		return "starting"
	elseif p == "unavailable" then
		return "failed"
	end
	return p
end

local function github_section(): Node
	local copied = ui.use_state(false)
	local function copy()
		local code = github_now().code
		if code then
			ui.clipboard.write(code)
		end
	end
	-- Connected just now: Cloud's repositories (what the picker offers) again.
	local was = github_now().phase
	ui.effect(function()
		local p = github_now().phase
		if p == "connected" and was ~= "connected" and was ~= "checking" then
			cloud.load_repos()
		end
		was = p
	end)
	return ds.Settings.Section {
		title = "GitHub",
		description = ui.t("settings.cloud.github.description"),
		row(
			"github",
			ds.GitHubBlock {
				hug = true,
				state = github_phase,
				copied = copied,
				code = function()
					return github_now().code
				end,
				account = function()
					return github_now().account
				end,
				repos = function()
					return github_now().repos
				end,
				failed = function()
					return github_now().failure
				end,
				on_connect = function()
					if github_now().phase == "unavailable" then
						github.load()
					else
						github.connect()
					end
				end,
				on_cancel = github.cancel,
				on_disconnect = function()
					github.disconnect()
				end,
				on_copy = copy,
				on_open = function()
					copy()
					github.open()
				end,
				on_choose = github.choose,
			}
		),
	}
end

-- The repositories Cloud can open, newest push first: a few, then how many
-- more; each private one says so. Chosen on GitHub; checked again here.
local SHOWN = 6

local function repositories(): Node
	local shown = ui.computed(function(): { cloud.Repo }
		local out = {}
		for i, r in cloud.now:get().repos or {} do
			if i > SHOWN then
				break
			end
			table.insert(out, r)
		end
		return out
	end)
	local function count(): number
		return #(cloud.now:get().repos or {})
	end
	return ui.show(
		function(): boolean
			return github_now().phase == "connected"
		end,
		ds.Settings.Section {
			title = ui.t("settings.cloud.repos.title"),
			description = ui.t("settings.cloud.repos.description"),
			trail = ds.IconButton {
				icon = "refresh",
				label = ui.t("settings.cloud.repos.refresh"),
				size = "sm",
				on_click = function()
					cloud.load_repos()
					github.load()
				end,
			},
			row(
				"repos",
				ui.column {
					w_full = true,
					gap = "$space.2",
					ui.show(function(): boolean
						return count() == 0
					end, ds.Settings.Group {
						ds.Settings.Row {
							label = function(): string
								return if cloud.now:get().repos == nil
									then ui.t("settings.cloud.repos.checking"):get()
									else ui.t("settings.cloud.repos.none"):get()
							end,
							description = ui.t("settings.cloud.repos.none_description"),
							icon = "git-branch",
							control = ds.Button {
								ui.t("settings.cloud.repos.choose"),
								tone = "secondary",
								size = "sm",
								on_click = github.choose,
								ds.Icon { name = "arrow-up-right", size = 14 },
							},
						},
					}),
					ds.Settings.Each(shown, function(_r: cloud.Repo, _i: number, r: State<cloud.Repo>): Node
						return ds.Settings.Row {
							label = function(): string
								return r:get().name
							end,
							icon = "git-branch",
							truncate = true,
							badge = ui.show(function(): boolean
								return r:get().private
							end, ds.Badge { ui.t("settings.cloud.repos.private"), size = "sm" }),
						}
					end, {
						key = function(r: cloud.Repo): string
							return r.name
						end,
					}),
					ui.show(function(): boolean
						return count() > SHOWN
					end, ui.row {
						px = ds.Settings.inset(),
						ui.text {
							function(): string
								return ui.t("settings.cloud.repos.more", { n = count() - SHOWN }):get()
							end,
							text_size = "$font.size.sm",
							line_height = "$font.line.sm",
							color = "$text.tertiary",
						},
					}),
				}
			),
		}
	)
end

-- Cloud's status couldn't be read: why, and another go.
local function unreachable(): Node
	return ui.show(function(): boolean
		local now = cloud.now:get()
		return now.phase == "failed" and now.status == nil
	end, ds.Callout {
		tone = "warning",
		title = ui.t("settings.cloud.unreachable.title"),
		body = function(): string
			local why = cloud.now:get().failure
			return if why == "offline"
				then ui.t("settings.engine.offline"):get()
				elseif why == "outdated" then ui.t("settings.cloud.unreachable.outdated"):get()
				else ui.t("settings.cloud.unreachable.failed"):get()
		end,
		action = { label = ui.t("common.retry") :: any, on_click = cloud.load },
	})
end

function M.view(): Node
	-- (Once a visit: Cloud's status, GitHub's, as the page opens.)
	cloud.load()
	github.load()
	return ds.Settings.Page {
		title = ui.t("settings.cloud.title"),
		description = ui.t("settings.cloud.description"),
		unreachable(),
		-- (Something wrong with Cloud: what, and another go.)
		ui.show(function(): boolean
			local st = cloud.now:get().status
			return st ~= nil and st.state == "error"
		end, ds.Callout {
			tone = "warning",
			title = ui.t("settings.cloud.attention.title"),
			body = function(): string
				local st = cloud.now:get().status
				return (st and st.error) or ui.t("settings.cloud.attention.failed"):get()
			end,
			action = { label = ui.t("common.retry") :: any, on_click = cloud.load },
		}),
		ui.show(function(): boolean
			return cloud.on()
		end, ui.column {
			w_full = true,
			gap = "$space.6",
			usage(),
			default_machine(),
			github_section(),
			repositories(),
		}),
	}
end

return M
