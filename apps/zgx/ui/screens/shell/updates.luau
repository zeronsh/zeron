-- What there is to update, wherever: on the top bar, beside the panel toggle,
-- a button that only shows when there's something, wearing the marks of what's
-- waiting (three at most) and the download glyph; pressed, the list, by device,
-- each with what it can do. Zeron itself is in it, this Mac's (`about`) and any
-- other device's (src/updates.rs), and so is every agent on every device.
--
--   Updates.Button {}    the button and its panel
--   Updates.width()      how much of the bar it takes (0 with nothing to say)
--
-- What each row offers is as the shipping app's agent list has it
-- (crates/ui/src/shell/harness_updates.rs): Update, View steps, Cancel, Retry
-- update, Check again.

local ds = require("@ds")
local sessions = require("@account/sessions")
local locale = require("@ds/locale")
local Menu = require("@ds/menu")
local toast = require("@ds/toast")
local status = require("@ds/status")
local route = require("@app/screens/shell/route")

local M = {}

-- The bar's tab height and its tile, as a session's tab (shell/session_tabs).
-- The gap between circles is cut out of the one behind, not painted: what's
-- under the button shows through it, whatever the button's state or the theme.
local BUTTON_H, TILE, GAP_CUT = 28, 20, 2.5
-- How far along each circle stands: enough of the one behind shows to know it.
local STEP = 15
-- A circle's mark: a little right of its middle on those behind, to sit in the
-- middle of what shows of them.
local MARK_SIDE, MARK_NUDGE = 11, 2
local MOST = 3
local GLYPH = 16
-- The button's insets: its circles as far in as they are from its top, its
-- glyph clear of its end.
local INSET_L, INSET_R, GAP = 4, 6, 5
local WIDTH, MARK, ACTION_H = 340, 28, 26
-- The most the list stands, Update all's strip included; past that it scrolls.
local MOST_LIST_H = 240
local ROW_H = 52
local HEAD_H = 44
local EDGE_FADE = 20
-- The room over the first row with nothing else up there.
local TOP_ROOM = 8
-- Update all's list: its lines, and how tall before it scrolls.
local REVIEW_ROW_H, REVIEW_MARK = 40, 24
local REVIEW_LIST_H = 6 * REVIEW_ROW_H
-- (A dialog's way out takes 130.)
local DIALOG_GONE_MS = 150

local agents = ui.shared("updates", { rows = {} })
local about = ui.shared("about", { app_version = "", available = false })

-- A restart to update asked for, kept across it: this Mac's Zeron should come
-- back on another version, and if it doesn't, it says so rather than offering
-- the same update as though nothing had happened.
local restarted_from = ui.persist("shell_update_restarted_from", nil :: string?)
local asked_this_run = false
ui.effect(function()
	local from = restarted_from:get()
	local a: any = about:get()
	local have = if a then a.app_version else ""
	if from == nil or asked_this_run or have == nil or have == "" then
		return
	end
	restarted_from:set(nil)
	if have == from then
		toast.show { title = locale.s("shell.updates.not_applied"), body = locale.s("shell.updates.not_applied_body"), tone = "danger" }
	end
end)

-- Updates skipped, kept until there's a newer one: each row's id and what it
-- offered then. (A new release, or a new failure, shows again.)
local skipped = ui.persist("shell_updates_skipped", {} :: { [string]: string })

-- What a row offers that can be skipped; nil: it can't be (it's under way, or
-- only needs a restart).
local function skip_token(r: any): string?
	if r.active or (r.phase ~= "available" and r.phase ~= "failed") then
		return nil
	end
	return r.latest or r.message or r.phase
end

toast.action("updates.unskip", function(ids: { string })
	local now = table.clone(skipped:peek())
	for _, id in ids do
		now[id] = nil
	end
	skipped:set(now)
end)

local function skip(list: { any })
	local now = table.clone(skipped:peek())
	local ids: { string } = {}
	for _, r in list do
		local token = skip_token(r)
		if token then
			now[r.id] = token
			table.insert(ids, r.id)
		end
	end
	if #ids == 0 then
		return
	end
	skipped:set(now)
	toast.show {
		title = locale.s("shell.updates.skipped", { n = #ids }),
		body = locale.s("shell.updates.skipped_body"),
		action = { label = locale.s("shell.updates.undo"), run = "updates.unskip", value = ids },
	}
end

-- This Mac's Zeron, as About & updates has it: nothing unless it can update
-- itself and a release is there for it.
local function zeron_here(): any?
	local a: any = about:get()
	if not a then
		return nil
	end
	-- Another device updated this Mac: the new version's on disk, and this
	-- window only has to reopen on it.
	if a.moved_on_to then
		return {
			id = "here:zeron",
			device = "",
			device_name = "",
			here = true,
			connected = true,
			kind = "zeron",
			installed = a.app_version,
			latest = a.moved_on_to,
			phase = "ready",
			active = false,
			action = "restart",
		}
	end
	if a.self_updating ~= true or a.held ~= nil then
		return nil
	end
	local flow = a.flow or "idle"
	if flow == "installed" or (flow == "idle" and a.available ~= true) then
		return nil
	end
	local row: any = {
		id = "here:zeron",
		device = "",
		device_name = "",
		here = true,
		connected = true,
		kind = "zeron",
		installed = a.app_version,
		latest = a.flow_version or a.latest,
		phase = if flow == "downloading"
			then "downloading"
			elseif flow == "ready" then "ready"
			elseif flow == "failed" then "failed"
			else "available",
		active = flow == "downloading",
		message = a.flow_error,
		action = if flow == "ready"
			then "restart"
			elseif flow == "failed" then "retry"
			elseif flow == "idle" then "update"
			else nil,
	}
	return row
end

-- Everything there is to say: this Mac's Zeron, others', then the agents.
local rows = ui.computed(function(): { any }
	local out: { any } = {}
	local passed = skipped:get()
	local function add(r: any)
		local token = skip_token(r)
		if token == nil or passed[r.id] ~= token then
			table.insert(out, r)
		end
	end
	local here = zeron_here()
	if here then
		add(here)
	end
	local given: any = agents:get()
	for _, r in (given and given.rows) or {} do
		add(r)
	end
	return out
end)

local function shown(): boolean
	return #rows:get() > 0
end

-- What the stack shows: each thing once (Zeron on three devices is one Zeron),
-- three at most.
local marks = ui.computed(function(): { any }
	local out: { any } = {}
	local seen: { [string]: boolean } = {}
	for _, r in rows:get() do
		local what = if r.kind == "zeron" then "zeron" else r.harness or r.id
		if not seen[what] then
			seen[what] = true
			table.insert(out, r)
			if #out == MOST then
				break
			end
		end
	end
	return out
end)

local function stack_w(): number
	return TILE + STEP * (math.max(#marks:get(), 1) - 1)
end

function M.width(): number
	return if shown() then INSET_L + stack_w() + GAP + GLYPH + INSET_R else 0
end

-- What the button's glyph says of them all.
local function state(): string
	local list = rows:get()
	local all_done = #list > 0
	for _, r in list do
		if r.active then
			return "working"
		end
		if r.phase ~= "updated" then
			all_done = false
		end
	end
	return if all_done then "done" else "download"
end

-- A row's mark on its tile, whatever it's on.
local function mark_of(r: any, size: number, bare: boolean?): Node
	if r.kind == "zeron" then
		if bare then
			return ui.svg { src = "assets/icons/zeron-mark.svg", w = size, h = size, shrink = 0 }
		end
		-- (Its logo on the white tile the agents' marks wear.)
		return ui.row {
			w = size,
			h = size,
			shrink = 0,
			items = "center",
			justify = "center",
			rounded = ui.raw(size * 0.25, "as a mark's tile"),
			bg = "$zeron.mark.bg",
			border = 1,
			border_color = "$zeron.mark.border",
			shadow = "$shadow.sm",
			ui.svg { src = "assets/icons/zeron-mark.svg", w = size * 0.62, h = size * 0.62, shrink = 0 },
		}
	end
	return ds.AgentMark {
		label = r.name or "",
		mark = r.mark,
		icon = r.agent_icon,
		seed = r.harness,
		size = size,
		bare = bare,
	}
end

local function name_of(r: any): string
	return if r.kind == "zeron" then "Zeron" else r.name or ""
end

-- Running chats on its device: updating its Zeron restarts its engine, which
-- stops them, and they carry on after. (One waiting on background work counts:
-- the restart ends that work too.)
local function running_on(device: string, here: boolean): number
	local n = 0
	for _, c in sessions.now:get().chats or {} do
		if (c.device == device or (here and device == "")) and (c.status == "working" or c.status == "input" or c.status == "waiting") then
			n += 1
		end
	end
	return n
end

-- The line under its name.
local function line_of(r: any): string
	if not r.connected then
		return locale.s("shell.updates.offline")
	end
	local p = r.phase
	if p == "failed" then
		return r.message or locale.s("shell.updates.failed")
	elseif p == "waiting" then
		return r.message or locale.s("shell.updates.waiting")
	elseif p == "preparing" then
		return locale.s("shell.updates.preparing")
	elseif p == "downloading" then
		return r.message or locale.s("shell.updates.downloading")
	elseif p == "installing" then
		return locale.s("shell.updates.installing")
	elseif p == "verifying" then
		return locale.s("shell.updates.verifying")
	elseif p == "ready" then
		local n = running_on("", true)
		if n > 0 then
			return locale.s("shell.updates.stops_chats", { n = n })
		end
		return locale.s("shell.updates.ready", { version = r.latest or "" })
	elseif p == "updated" then
		return if r.installed
			then locale.s("shell.updates.updated_to", { version = r.installed })
			else locale.s("shell.updates.updated")
	end
	local versions = if r.installed and r.latest
		then `{r.installed} → {r.latest}`
		elseif r.latest then locale.s("shell.updates.version", { version = r.latest })
		else locale.s("shell.updates.available")
	if r.kind == "zeron" and not r.here then
		if r.why_not then
			return r.why_not
		end
		if r.action == nil then
			return locale.s("shell.updates.update_there", { versions = versions })
		end
		local n = running_on(r.device, false)
		if n > 0 then
			return locale.s("shell.updates.stops_chats", { n = n })
		end
	end
	return versions
end

local function label_of(r: any): string
	local a = r.action
	if a == "update" then
		if r.kind == "zeron" and not r.here and running_on(r.device, false) > 0 then
			return locale.s("shell.updates.update_anyway")
		end
		return locale.s("shell.updates.update")
	elseif a == "restart" then
		return locale.s("shell.updates.restart")
	elseif a == "steps" then
		return locale.s("shell.updates.view_steps")
	elseif a == "copy" then
		return locale.s("shell.updates.copy_command")
	elseif a == "cancel" then
		return locale.s("common.cancel")
	elseif a == "retry" then
		return locale.s("shell.updates.retry")
	elseif a == "check" then
		return locale.s("shell.updates.check_again")
	end
	return ""
end

local function failed(what: string, err: any)
	toast.show {
		title = locale.s("shell.updates.failed_named", { name = what }),
		body = if type(err) == "string" then err else nil,
		tone = "danger",
	}
end

-- What pressing its action does.
local function act(r: any, close: () -> ())
	local a = r.action
	if a == "copy" then
		ui.clipboard.write(r.manual_command or "")
		toast.show { title = locale.s("shell.updates.copied", { device = r.device_name }) }
		return
	end
	if a == "steps" then
		close()
		pcall(ui.invoke, "agents.target", if r.here then nil else r.device)
		route.settings("providers")
		return
	end
	local name = name_of(r)
	if r.kind == "zeron" and r.here then
		if a == "restart" then
			local about_now: any = about:peek()
			asked_this_run = true
			restarted_from:set(about_now and about_now.app_version)
		end
		ui.call("about.apply", {}):after(function() end, function(err: any)
			asked_this_run = false
			restarted_from:set(nil)
			failed(name, err)
		end)
	elseif r.kind == "zeron" then
		ui.call("updates.apply_zeron", { device = r.device }):after(function() end, function(err: any)
			failed(name, err)
		end)
	else
		ui.call("updates.run", { device = r.device, harness = r.harness, action = a }):after(function() end, function(err: any)
			failed(name, err)
		end)
	end
end

-- A row's action, quiet in a list of them: solid only where it's the one thing
-- to do (a restart to finish; a list of one).
local function action_button(r: any, close: () -> ()): Node
	local primary = r.action == "restart" or (#rows:peek() == 1 and (r.action == "update" or r.action == "retry"))
	local enabled = r.connected or r.action == "steps" or r.action == "copy"
	return ui.button {
		id = `update-action-{r.id}`,
		label = `{label_of(r)} · {name_of(r)}`,
		disabled = not enabled,
		h = ACTION_H,
		px = ui.raw(10, "an action's inset"),
		shrink = 0,
		items = "center",
		justify = "center",
		rounded = "$radius.full",
		bg = if primary then "$text.primary" else "$hover",
		color = if primary then "$background" else "$text.secondary",
		opacity = if enabled then 1 else 0.45,
		hover = if not enabled then nil elseif primary then { opacity = 0.85 } else { color = "$text.primary" },
		cursor = if enabled then "pointer" else nil,
		on_click = function()
			act(r, close)
		end,
		ui.text { label_of(r), literal = true, text_size = "$font.size.xs", weight = "$font.weight.medium" },
	}
end

-- A row's skip: quiet words beside its action.
local function skip_button(r: any): Node?
	if skip_token(r) == nil then
		return nil
	end
	return ui.button {
		id = `update-skip-{r.id}`,
		label = `{locale.s("shell.updates.skip")} · {name_of(r)}`,
		tooltip = locale.s("shell.updates.skip_hint"),
		h = ACTION_H,
		px = ui.raw(4, "words, close by what's beside them"),
		shrink = 0,
		items = "center",
		bg = "#00000000",
		color = "$text.tertiary",
		hover = { color = "$text.primary", bg = "#00000000" },
		active = { bg = "#00000000" },
		cursor = "pointer",
		on_click = function()
			skip({ r })
		end,
		ui.text { ui.t("shell.updates.skip"), text_size = "$font.size.xs", weight = "$font.weight.medium" },
	}
end

local function row(r: any, close: () -> ()): Node
	return ui.row {
		id = `update-row-{r.id}`,
		w_full = true,
		min_h = ui.raw(ROW_H, "a row of two lines and its room"),
		px = "$space.3",
		py = "$space.2",
		gap = "$space.3",
		items = "center",
		ui.div { shrink = 0, mark_of(r, MARK) },
		ui.column {
			grow = 1,
			min_w = 0,
			gap = ui.raw(1, "a name and its line, one thing"),
			ds.FadeText { name_of(r), literal = true, w_full = true, text_size = "$font.size.sm", weight = "$font.weight.medium", color = "$text.primary", truncate = true },
			ds.FadeText {
				line_of(r),
				literal = true,
				w_full = true,
				text_size = "$font.size.xs",
				color = if r.phase == "failed" and r.connected then "$danger" else "$text.secondary",
				tabular = true,
			},
		},
		if r.active then ui.div { shrink = 0, status.StatusIcon { kind = "working", size = 14 } } else nil,
		if r.phase == "updated" then ui.div { shrink = 0, color = "$success", ds.Icon { name = "check", size = 14 } } else nil,
		skip_button(r),
		if r.action then action_button(r, close) else nil,
	}
end

-- By device: this Mac's first, as they come. One device needs no heading.
local function by_device(list: { any }): { { name: string, rows: { any } } }
	local out: { { name: string, rows: { any } } } = {}
	local at: { [string]: number } = {}
	for _, r in list do
		local key = if r.here then "" else r.device
		local i = at[key]
		if not i then
			table.insert(out, { name = if r.here then locale.s("shell.updates.this_device") else r.device_name, rows = {} })
			i = #out
			at[key] = i
		end
		table.insert(out[i].rows, r)
	end
	return out
end

-- What the list says now: it's made again as this changes (a short list).
local function signature(): string
	local parts: { string } = {}
	for _, r in rows:get() do
		table.insert(parts, table.concat({
			r.id,
			r.phase or "",
			r.action or "",
			r.message or "",
			r.installed or "",
			r.latest or "",
			tostring(r.connected),
			tostring(if r.kind == "zeron" and not r.here then running_on(r.device, false) else 0),
		}, "\0"))
	end
	return table.concat(parts, "\n")
end

-- What Update all would do, now: what it updates, what running chats that
-- stops, and what it can't reach and why.
local function plan(list: { any }): any
	local out: any = { agents = {}, zerons = {}, here = nil, chats = 0, skipped = {} }
	for _, r in list do
		local go = r.connected and r.action == "update"
		if go and r.kind == "agent" then
			table.insert(out.agents, r)
		elseif go and r.here then
			out.here = r
		elseif go then
			table.insert(out.zerons, r)
			out.chats += running_on(r.device, false)
		elseif r.phase == "available" and not r.connected then
			table.insert(out.skipped, { r = r, why = locale.s("shell.updates.offline") })
		elseif r.phase == "available" and r.action == "steps" then
			table.insert(out.skipped, { r = r, why = locale.s("shell.updates.manual") })
		elseif r.phase == "available" and r.kind == "zeron" and (r.action == nil or r.action == "copy") then
			table.insert(out.skipped, { r = r, why = r.why_not or locale.s("shell.updates.on_that_device") })
		end
	end
	return out
end

-- Everything it can do, in the order that strands nothing: agents, other
-- devices' Zeron, then this Mac's, which ends by asking to restart.
local function update_all()
	local p = plan(rows:peek())
	local function close() end
	for _, r in p.agents do
		act(r, close)
	end
	for _, r in p.zerons do
		act(r, close)
	end
	if p.here then
		act(p.here, close)
	end
end

local function can_update_all(): boolean
	local p = plan(rows:get())
	return #p.agents + #p.zerons + (if p.here then 1 else 0) > 1
end

-- A line of Update all's list: its mark, its name, and on the far side what it
-- goes to, or why it's left out.
local function review_row(r: any, side: string, out: boolean?): Node
	return ui.row {
		w_full = true,
		h = ui.raw(REVIEW_ROW_H, "a line of the list and its room"),
		gap = "$space.3",
		items = "center",
		opacity = if out then 0.55 else 1,
		ui.div { shrink = 0, mark_of(r, REVIEW_MARK) },
		ds.FadeText {
			name_of(r),
			literal = true,
			grow = 1,
			text_size = "$font.size.sm",
			weight = "$font.weight.medium",
			color = "$text.primary",
		},
		ds.FadeText {
			side,
			literal = true,
			shrink = 1,
			max_w = "60%",
			tabular = true,
			text_size = "$font.size.xs",
			color = "$text.secondary",
		},
	}
end

local function review_caption(words: string): Node
	return ds.FadeText {
		words,
		literal = true,
		w_full = true,
		pt = "$space.2",
		pb = "$space.1",
		text_size = "$font.size.xs",
		color = "$text.tertiary",
	}
end

local function going_to(r: any): string
	if r.installed and r.latest then
		return `{r.installed} → {r.latest}`
	end
	return r.latest or ""
end

-- What's in the list, built as it opens: what goes, by device, then what can't.
local function review_list(): Node
	local p = plan(rows:peek())
	local going: { any } = {}
	for _, list in { p.agents, p.zerons } do
		for _, r in list do
			table.insert(going, r)
		end
	end
	if p.here then
		table.insert(going, 1, p.here)
	end
	local groups = by_device(going)
	local parts: { any } = { w_full = true, pb = "$space.1" }
	for _, g in groups do
		if #groups > 1 then
			table.insert(parts, review_caption(g.name))
		end
		for _, r in g.rows do
			table.insert(parts, review_row(r, going_to(r)))
		end
	end
	if #p.skipped > 0 then
		table.insert(parts, review_caption(locale.s("shell.updates.not_included")))
		for _, s in p.skipped do
			local where = if s.r.here or #groups < 2 then s.why else `{s.r.device_name} · {s.why}`
			table.insert(parts, review_row(s.r, where, true))
		end
	end
	local notes: { any } = { w_full = true, gap = "$space.1" }
	local function note(words: string, ink: string?)
		table.insert(notes, ui.text {
			words,
			literal = true,
			wrap = true,
			w_full = true,
			text_size = "$font.size.xs",
			line_height = "$font.line.xs",
			color = ink or "$text.secondary",
		})
	end
	if #p.agents > 0 then
		note(locale.s("shell.updates.plan_agents"))
	end
	if p.chats > 0 then
		note(locale.s("shell.updates.plan_chats", { n = p.chats }), "$text.primary")
	end
	if p.here then
		note(locale.s("shell.updates.plan_here"))
	end
	return ui.column {
		id = "updates-review",
		w_full = true,
		gap = "$space.3",
		ui.scroll {
			w_full = true,
			max_h = ui.raw(REVIEW_LIST_H, "six lines, then it scrolls"),
			fade = { EDGE_FADE, EDGE_FADE },
			fade_overflow = true,
			ui.column(parts),
		},
		if #notes > 0 then ui.column(notes) else nil,
	}
end

-- Update all's second look: what it will do, before it does.
local function review(open: any): Node
	return ds.Dialog {
		open = open,
		title = ui.t("shell.updates.all_title"),
		body = function(): string
			local p = plan(rows:get())
			local n = #p.agents + #p.zerons + (if p.here then 1 else 0)
			local devices: { [string]: boolean } = {}
			for _, list in { p.agents, p.zerons } do
				for _, r in list do
					devices[if r.here then "" else r.device] = true
				end
			end
			if p.here then
				devices[""] = true
			end
			local d = 0
			for _ in devices do
				d += 1
			end
			if d > 1 then
				return locale.s("shell.updates.all_count_devices", { n = n, devices = d })
			end
			return locale.s("shell.updates.all_count", { n = n })
		end,
		size = "md",
		build = review_list,
		aside = {
			label = ui.t("shell.updates.skip_all"),
			on_click = function()
				-- (Once it has left: the button it hangs from goes with the last.)
				local list = rows:peek()
				ui.after(DIALOG_GONE_MS, function()
					skip(list)
				end)
			end,
		},
		confirm = { label = ui.t("shell.updates.update_all"), on_click = update_all },
	}
end

local function panel(open: any, anchor: any, reviewing: any): Node
	local function close()
		open:set(false)
	end
	return ui.popover(Menu.panel {
		class = "updates-panel",
		open = open,
		anchor = anchor,
		placement = { side = "bottom", align = "end", offset = 6 },
		w = WIDTH,
		build = function()
			local offset = ui.use_state(0)
			-- The room over the first row: Update all's, when it's there.
			local function above(): number
				return if can_update_all() then HEAD_H else TOP_ROOM
			end
			-- How tall its rows are: a fair guess as it opens, then measured.
			local content_h = ui.use_state(above() + #rows:peek() * ROW_H)
			return ui.column {
				w_full = true,
				relative = true,
				-- (No title on show: what it's called, for what reads it aloud.)
				label = locale.s("shell.updates.title"),
				ui.scroll {
					w_full = true,
					offset = offset,
					-- As tall as its rows, up to its most. (A scroll with no height
					-- of its own stands at a fixed one, whatever it holds.)
					h = function(): number
						return math.min(content_h:get(), MOST_LIST_H)
					end,
					-- Each edge fades where there's more to scroll that way; the top
					-- reaches the panel's edge, under Update all.
					fade = function(): { number }
						local o = offset:get()
						local most = math.max(0, content_h:get() - MOST_LIST_H)
						return { math.min(o, above() + EDGE_FADE), math.min(math.max(0, most - o), EDGE_FADE) }
					end,
					ui.column {
						w_full = true,
						shrink = 0,
						pt = function(): any
							return ui.raw(above(), "the room over the first row")
						end,
						-- (Nothing over the rows: the same room under them, so they sit
						-- in the middle.)
						pb = function(): any
							return if can_update_all() then "$space.1" else ui.raw(TOP_ROOM, "as much as over them")
						end,
						on_resize = function(size: { w: number, h: number })
							if math.abs(size.h - content_h:peek()) >= 0.5 then
								content_h:set(size.h)
							end
						end,
						ui.presence(signature, function(_: string): Node
							local groups = by_device(rows:peek())
							local parts: { any } = { w_full = true }
							for _, g in groups do
								if #groups > 1 then
									table.insert(parts, ds.FadeText {
										g.name,
										literal = true,
										w_full = true,
										px = "$space.3",
										pt = "$space.2",
										text_size = "$font.size.xs",
										color = "$text.tertiary",
									})
								end
								for _, r in g.rows do
									table.insert(parts, row(r, close))
								end
							end
							return ui.column(parts)
						end, { w_full = true }),
					},
				},
				-- Update all, over the list, which scrolls away under it. (Its strip
				-- takes its own presses: a row faded out beneath it isn't there to
				-- be pressed.)
				ui.show(
					can_update_all,
					ui.row {
						absolute = true,
						left = 0,
						right = 0,
						top = 0,
						h = ui.raw(HEAD_H, "Update all's strip"),
						on_press = function() end,
						pl = "$space.3",
						pr = "$space.2",
						items = "center",
						justify = "end",
						ui.button {
							id = "updates-all",
							label = ui.t("shell.updates.update_all"),
							h = ACTION_H,
							px = ui.raw(10, "an action's inset"),
							items = "center",
							rounded = "$radius.full",
							bg = "$text.primary",
							color = "$background",
							hover = { opacity = 0.85 },
							cursor = "pointer",
							on_click = function()
								close()
								reviewing:set(true)
							end,
							ui.text { ui.t("shell.updates.update_all"), text_size = "$font.size.xs", weight = "$font.weight.medium" },
						},
					}
				),
			}
		end,
	})
end

-- A circle behind another, as a crescent: the whole circle less the one in
-- front of it and the gap around that.
local CRESCENT = (function(): string
	local r, far = TILE / 2, TILE / 2 + GAP_CUT
	local a = (STEP * STEP + r * r - far * far) / (2 * STEP)
	local h = math.sqrt(r * r - a * a)
	local x, top, bottom = r - a, r - h, r + h
	return `code:<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {TILE} {TILE}"><path fill="#ffffff" d="M{x} {top}A{r} {r} 0 1 1 {x} {bottom}A{far} {far} 0 0 0 {x} {top}Z"/></svg>`
end)()

-- A circle of the stack: the mark's own tile colour, the mark on it. Ink-only
-- marks take their tile's ink, as on the list's tiles.
local function tile(at: number): Node
	local behind = at > 1
	return ui.show(function(): boolean
		return #marks:get() >= at
	end, ui.div {
		id = `updates-mark-{at}`,
		absolute = true,
		top = 0,
		left = (at - 1) * STEP,
		w = TILE,
		h = TILE,
		if behind
			then ui.svg { src = CRESCENT, tint = "$zeron.mark.bg", absolute = true, left = 0, top = 0, w = TILE, h = TILE }
			else ui.div { absolute = true, left = 0, top = 0, w = TILE, h = TILE, rounded = "$radius.full", bg = "$zeron.mark.bg" },
		ui.row {
			absolute = true,
			left = if behind then MARK_NUDGE else 0,
			top = 0,
			w = TILE,
			h = TILE,
			items = "center",
			justify = "center",
			color = "#000000",
			ui.presence(function(): string
				local r = marks:get()[at]
				return if r then r.id else ""
			end, function(id: string): Node
				for _, r in marks:peek() do
					if r.id == id then
						return mark_of(r, MARK_SIDE, true)
					end
				end
				return ui.div {}
			end),
		},
	})
end

M.Button = ui.component(function(): Node
	local open = ui.use_state(false)
	local reviewing = ui.use_state(false)
	local hovered = ui.use_state(false)
	-- (Nothing left to say: nothing left open.)
	ui.effect(function()
		if not shown() and open:peek() then
			open:set(false)
		end
	end)
	local stack: { any } = { relative = true, shrink = 0, h = TILE, w = stack_w }
	-- (Those behind first: each paints under the one after it.)
	for at = MOST, 1, -1 do
		table.insert(stack, tile(at))
	end
	local trigger = ui.row {
			id = "updates-button",
			role = "button",
			label = function(): string
				return locale.s("shell.updates.button", { n = #rows:get() })
			end,
			expanded = open,
			h = BUTTON_H,
			pl = ui.raw(INSET_L, "as far in as they are from its top"),
			pr = ui.raw(INSET_R, "clear of its end"),
			gap = ui.raw(GAP, "its marks and its glyph"),
			items = "center",
			rounded = "$radius.lg",
			cursor = "pointer",
			pointer_events = "auto",
			bg = function(): string
				if open:get() then
					return "$zeron.selected.flat.bg"
				end
				return if hovered:get() then "$hover" else "#00000000"
			end,
			color = function(): string
				return if open:get() or hovered:get() then "$text.primary" else "$text.secondary"
			end,
			on_hover = function(inside: boolean)
				hovered:set(inside)
			end,
			on_click = function()
				open:set(not open:peek())
			end,
			ui.div(stack),
			ui.presence(state, function(which: string): Node
				if which == "working" then
					return status.StatusIcon { kind = "working", size = GLYPH }
				end
				return ds.Icon { name = if which == "done" then "check" else "download", size = GLYPH }
			end, { w = GLYPH, h = GLYPH, shrink = 0 }),
	}
	-- (The popover draws its anchor. The dialog stands apart from it: a press
	-- in one reads as a press outside the other.)
	return ui.div { shrink = 0, panel(open, trigger, reviewing), review(reviewing) }
end)

M.shown = shown

return M
