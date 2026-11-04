-- Toasts: short messages that come and go in the window's bottom corner,
-- over everything, taking no focus. Each is a card of the menus' glass: a
-- tone's glyph, a title and a line, an action, and a × that shows under
-- the pointer. They stack newest at the bottom, rising in; each goes by
-- itself after a while (longer for a warning; a danger stays until put
-- away), paused while the pointer is over the stack. Showing the same
-- message again restarts it rather than adding another.
--
--   toast.show { title = "Pushed to origin", tone = "success" }
--   toast.action("git.output", show_output)   -- at a module's top, each load
--   toast.show { title = "Commit failed", body = "pre-commit hook exited 1",
--     tone = "danger", action = { label = "Show output", run = "git.output", value = sha } }
--   toast.sync("waiting", { … })   -- this group's toasts are these, by `key`
--   toast.Host {}   -- once, in the window's root
--   toast.Host { front = is_up }   -- in a full-window modal (the lightbox)
--
-- `show` returns the toast's id, which `dismiss(id)` puts away. Use this,
-- never zgx's `ui.toast`: that one's stack is a zgx layer, which zgx draws
-- the modal dim behind (the whole window tinted while a toast shows).
--
-- **An action is data.** A toast is state, which outlives a load of the
-- scripts (a plugin turned on reloads them, and a launch's first look at the
-- plugins does), and a function doesn't: it dies with the VM that made it.
-- So a toast's `run` names an action, which the module that owns it declares
-- with `toast.action` at its top: every load declares it again, so a toast
-- kept across a load finds it there. `run` may be a plain function for
-- what's only worth doing now, as the gallery's: that toast goes with its
-- load, never staying up without the button it was shown with.

local motion = require("@self/motion")
local Icon = require("@self/icon")
local IconButton = require("@self/icon_button").IconButton
local Button = require("@self/button")
local glass = require("@self/glass")
local control = require("@self/control")
local locale = require("@self/locale")

local M = {}

-- (Its words may be messages, `ui.t`: read as text as it is shown, in the
-- locale then; the stack is a state, which holds no message.)
export type Tone = "neutral" | "info" | "success" | "warning" | "danger"
export type Toast = {
	title: string | Message,
	body: (string | Message)?,
	tone: Tone?,
	-- `run`: an action's name (`M.action`), called with `value`, which is
	-- kept in state like the rest. `keep`: the toast stays after its button
	-- is pressed (what it asks isn't done yet; it goes when it is).
	action: { label: string | Message, run: string | (any) -> (), value: any?, keep: boolean? }?,
	-- How long it stays, in milliseconds (0: until put away).
	duration: number?,
	-- Who it is, when words aren't enough: shown again, the toast with the
	-- same key starts over, whatever else it says. Put away by you, it's
	-- told in `M.put_away`.
	key: string?,
}

-- How many show at once; older ones wait their turn.
local MAX = 4
-- How long each tone stays.
local DURATION: { [Tone]: number } = { neutral = 4000, info = 4000, success = 4000, warning = 7000, danger = 0 }
local GLYPH: { [Tone]: Icon.Name } = {
	neutral = "info-circle",
	info = "info-circle",
	success = "check-circle",
	warning = "danger-triangle",
	danger = "danger-triangle",
}

-- The stack: each toast as data, with an id and the time it was (re)shown.
local toasts = ui.state("ds_toasts", {} :: { any })
local hovered = ui.state("ds_toasts_hovered", false)
-- (Kept as state, as the stack is: a hot reload keeps the toasts showing,
-- so a new one mustn't take one of their ids.)
local next_id = ui.state("ds_toasts_next", 0)
-- How many hosts in front (`front`) are up: while one is, the stack shows
-- there, over the modal that holds it, and not under it.
local fronts = ui.state("ds_toasts_fronts", 0)
-- Where the stack stands, in points up from the window's foot: over the
-- host's own toasts (a template in preview, signing keys: drawn by Zeron
-- itself, in the same corner), as the main window's host measures them
-- (src/plugins/host_bar.rs `follow`). 16 where no host says (another
-- window; a plugin, which can't read it).
local floor_ok, floor = pcall(ui.shared, "toast_floor", 16)
local function foot(): number
	local at = if floor_ok and floor then floor:get() else nil
	return if type(at) == "number" then at else 16
end
-- What each action runs, by its name: this load's, as `action` declared
-- them (functions can't live in state). `declared` says one was added, for a
-- stack built before the module that owns it loaded.
local actions: { [string]: (any) -> () } = {}
local declared = ui.state("ds_toasts_declared", 0)
-- An action given as a function has a name of its own, `fn:<toast id>`, and
-- lasts as long as this load.
local FN = "fn:"

-- The keys of toasts you put away with the ×, for whoever would show them
-- again to take (and clear): `{ [key]: true }`.
M.put_away = ui.state("ds_toasts_put_away", {} :: { [string]: boolean })

-- Those of a load before this one, whose function is gone, go now.
do
	local kept = {}
	for _, t in toasts:peek() do
		if not (t.action and string.sub(t.action.run, 1, #FN) == FN) then
			table.insert(kept, t)
		end
	end
	if #kept ~= #toasts:peek() then
		toasts:set(kept)
	end
end

-- Declares what `run = name` does. At the top of the module that owns it, so
-- each load declares it again.
function M.action(name: string, run: (any) -> ())
	actions[name] = run
	declared:set(declared:peek() + 1)
end

function M.dismiss(id: number)
	local i = toasts:index_of("id", id)
	if i then
		toasts:remove(i)
		actions[`{FN}{id}`] = nil
	end
end

-- The action as the stack keeps it: data, its words as strings.
local function kept_action(given: any, id: number): any
	if not given then
		return nil
	end
	local run = given.run
	if type(run) == "function" then
		local name = `{FN}{id}`
		actions[name] = run
		run = name
	end
	return { label = control.text(given.label) or "", run = run, value = given.value, keep = given.keep }
end

function M.show(given: Toast): number
	-- (Its words as strings, as it is shown.)
	local t = table.clone(given) :: any
	t.title = control.text(given.title) or ""
	t.body = control.text(given.body)
	local tone: Tone = t.tone or "neutral"
	-- The same one again (by its key, else its words): restart it in place.
	for _, existing in toasts:peek() do
		local same = if t.key
			then existing.key == t.key
			else existing.key == nil and existing.title == t.title and existing.body == t.body and existing.tone == tone
		if same then
			local i = toasts:index_of("id", existing.id)
			if i then
				toasts:set_key({ i, "shown" }, ui.now())
			end
			return existing.id
		end
	end
	local id = next_id:peek() + 1
	next_id:set(id)
	toasts:push({
		id = id,
		key = t.key,
		group = t.group,
		title = t.title,
		body = t.body,
		tone = tone,
		action = kept_action(given.action, id),
		duration = t.duration or DURATION[tone],
		shown = ui.now(),
	})
	return id
end

-- The toasts of `group` are these, each by its `key`: one missing shows, one
-- no longer here goes, one that says something else now is shown anew, and
-- one already up is left as it is. For what follows a list, so asking again
-- (each load does) changes nothing anyone can see.
function M.sync(group: string, wanted: { Toast })
	local want: { [string]: Toast } = {}
	for _, w in wanted do
		want[w.key :: string] = w
	end
	for _, t in table.clone(toasts:peek()) do
		if t.group == group then
			local w = want[t.key]
			if w and control.text(w.title) == t.title and control.text(w.body) == t.body then
				want[t.key] = nil
			else
				M.dismiss(t.id)
			end
		end
	end
	for _, w in wanted do
		if want[w.key :: string] then
			local t = table.clone(w) :: any
			t.group = group
			M.show(t)
		end
	end
end

-- One card. Its timer runs while the stack isn't under the pointer;
-- reshown, it starts again.
local Card = ui.component(function(props: { toast: any }): Node
	local t = props.toast
	local over = ui.use_state(false)
	local tone: Tone = t.tone
	if t.duration > 0 then
		-- From when it was (re)shown, as the stack says now: shown again,
		-- it starts over; built again (a hot reload), it runs on from where
		-- it was. (The stack's rows are made once, as they come.)
		local timer: any = nil
		ui.effect(function()
			local i = toasts:index_of("id", t.id)
			local now = if i then toasts:get()[i] else nil
			if not now then
				return
			end
			if timer then
				timer:cancel()
			end
			timer = anime.createTimer {
				duration = math.max(1, t.duration - (ui.now() - now.shown)),
				onComplete = function()
					M.dismiss(t.id)
				end,
			}
			timer.paused = hovered:peek()
		end)
		ui.effect(function()
			local paused = hovered:get()
			if timer then
				timer.paused = paused
			end
		end)
		ui.on_cleanup(function()
			if timer then
				timer:cancel()
			end
		end)
	end
	-- (Its props typed; the menus' glass laid on them in place.)
	local card: ContainerProps = {
		w = "$zeron.toast.width",
		pl = "$space.4",
		pr = "$space.2",
		py = "$space.3",
		gap = "$space.3",
		items = "start",
		rounded = "$radius.xl",
		role = if tone == "danger" then "alert" else "status",
		live = if tone == "danger" then "assertive" else "polite",
		on_hover = function(inside: boolean)
			over:set(inside)
		end,
		ui.div {
			pt = ui.raw(1, "centres the 16 glyph on an 18 line"),
			shrink = 0,
			color = if tone == "neutral" then "$text.secondary" else `$zeron.tone.{tone}.ink`,
			Icon { name = GLYPH[tone], size = 16 },
		},
		ui.column {
			grow = 1,
			min_w = 0,
			gap = "$zeron.space.hair",
			ui.text { t.title, text_size = "$font.size.md", line_height = "$font.line.md", weight = "$font.weight.medium", color = "$text.primary", wrap = true },
			if t.body
				then ui.text { t.body, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.secondary", wrap = true }
				else nil,
			if t.action
				then ui.row {
					pt = "$space.2",
					Button {
						t.action.label,
						size = "sm",
						tone = "secondary",
						on_click = function()
							local run = actions[t.action.run]
							if not t.action.keep then
								M.dismiss(t.id)
							end
							if run then
								run(t.action.value)
							end
						end,
					},
				}
				else nil,
		},
		-- The ×: always for a toast that stays, else under the pointer.
		ui.div {
			-- In the corner: as far from the top as from the end, concentric.
			mt = ui.raw(-4, "as far from the top as from the end"),
			theme = { zeron = { icon_button = { radius = control.nest("$radius.xl", 1, "$space.2") } } },
			shrink = 0,
			opacity = if t.duration == 0
				then 1
				else ui.tween(over, { duration = 120 }),
			IconButton {
				icon = "close",
				label = locale.t("common.dismiss"),
				size = "sm",
				tone = "quiet",
				tooltip = false,
				on_click = function()
					if t.key then
						M.put_away:set_key(t.key, true)
					end
					M.dismiss(t.id)
				end,
			},
		},
	}
	glass.style(card, "menu")
	return ui.row(card)
end)

-- The stack, in the bottom corner of what holds it: put one in the
-- window's root (an overlay), over the page; `show` fills it. (It is
-- not a zgx layer, which would raise it over dialogs too: zgx draws the
-- modal dim behind a layer for now.) A modal over the whole window (the
-- lightbox) puts one in itself, `front` saying whether it's up (a closed
-- modal keeps what it built): while it is, the stack shows there, and the
-- root's stands aside.
local function Stack(up: () -> boolean): Node
	local visible = ui.computed(function(): { any }
		if not up() then
			return {}
		end
		declared:get()
		-- (Only those whose button is there to press: one whose action isn't
		-- declared yet waits for the module that owns it.)
		local list = {}
		for _, t in toasts:get() do
			if not t.action or actions[t.action.run] then
				table.insert(list, t)
			end
		end
		local out = {}
		for i = math.max(1, #list - MAX + 1), #list do
			table.insert(out, list[i])
		end
		return out
	end)
	return ui.each(visible, function(t: any): Node
		return Card { toast = t }
	end, {
		key = function(t: any)
			return t.id
		end,
		animate = motion.layout({ duration = 260, ease = "out(3)" }),
		enter = motion.transition({ opacity = { 0, 1 }, y = { 16, 0 }, duration = 260, ease = "out(3)" }),
		exit = motion.transition({ opacity = { 1, 0 }, scale = { 1, 0.96 }, duration = 160, ease = "in(2)" }),
		absolute = true,
		-- (Insets take points: space.4 off the corner, and up from the
		-- foot as the host says.)
		right = 16,
		bottom = foot,
		gap = "$space.2",
		items = "end",
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
	})
end

local Front = ui.component(function(props: { up: () -> boolean }): Node
	local up = ui.computed(props.up)
	ui.effect(function()
		if up:get() then
			fronts:set(fronts:peek() + 1)
			ui.on_cleanup(function()
				fronts:set(math.max(0, fronts:peek() - 1))
			end)
		end
	end)
	return Stack(function(): boolean
		return up:get()
	end)
end)

function M.Host(props: { front: (() -> boolean)?, [any]: any }?): Node
	local front = if props then props.front else nil
	if front then
		return Front { up = front }
	end
	return Stack(function(): boolean
		return fronts:get() == 0
	end)
end

return M
