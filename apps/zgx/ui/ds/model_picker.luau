-- Picking the model an agent runs, as Zeron's compact picker does
-- (crates/ui/src/pickers.rs, render_harness_model_popover and
-- pickers/compact.rs): it opens folded on the chosen model, its options
-- (Devin Fusion's lead and sidekick, each a row opening its choices beside
-- the panel) under it, then its reasoning (an EffortSlider, its fill in
-- ASCII, Fast mode's bolt in its header). A press on the model opens the
-- list out: a tab for each agent along the top (its mark; the tab shown
-- wears an accent bar on the rule), a search scoped to the tab, the tab's
-- models. Devin Fusion's options (Lead, Sidekick) are rows like any
-- model's, under it once it's chosen: no card beside its row.
-- A dropdown underneath: its trigger, keys, search, list and panel.
--
-- `ModelPicker { model = state, effort = state, options = state, harnesses = { … }, models = { … } }`

local control = require("@self/control")
local Dropdown = require("@self/dropdown")
local Mark = require("@self/mark")
local AgentMark = require("@self/agent_mark")
local EffortSlider = require("@self/effort_slider")
local Switch = require("@self/switch")
local Icon = require("@self/icon")
local icon_button = require("@self/icon_button")
local IconButton = icon_button.IconButton
local Menu = require("@self/menu")
local FadeText = require("@self/fade_text")
local locale = require("@self/locale")

-- An agent: a tab of its own.
-- (`icon`: an agent from the ACP registry's one-ink SVG, in its mark's
-- place.)
export type Harness = { id: string, label: string, mark: Mark.Name, icon: string? }
-- One of a model option's choices.
export type Choice = { id: string, label: string }
-- An option of a model (Devin Fusion's lead and sidekick): its choices and
-- the one it starts on (by id).
export type Option = { id: string, label: string, default: string, choices: { Choice } }
-- A model: its agent, its reasoning levels (low to high) and the one it
-- starts on, whether it offers Fast mode, and its other options.
export type Model = {
	value: string,
	label: string,
	harness: string,
	efforts: { EffortSlider.Level }?,
	effort: string?,
	fast: boolean?,
	options: { Option }?,
}
export type Props = {
	model: State<string?>,
	effort: State<string?>,
	fast: State<boolean>?,
	-- The choices made of the model's options (option id to choice id; one
	-- left out is at its default), changed as they're picked.
	options: State<{ [string]: string }>?,
	-- The agents and their models: lists, or states or functions of them
	-- (a catalog that arrives or changes, followed in place: the tabs and
	-- rows come and go, the picker stays).
	harnesses: { Harness } | State<{ Harness }> | () -> { Harness },
	models: { Model } | State<{ Model }> | () -> { Model },
	tone: Dropdown.Tone?,
	label: string?,
	placement: { side: string?, align: string?, offset: number? }?,
	-- The panel read bottom up (opening above its trigger).
	bottom_up: boolean?,
	-- The models starred (by value), kept across launches unless given.
	favorites: State<{ [string]: boolean }>?,
	-- Bumped, it opens (a shortcut to it; Dropdown's `open_with`).
	open_with: State<number>?,
}

-- Devin Fusion's lead (what runs), listed first among its options.
local LEAD = "lead"
-- A choice list's width.
local CHOICES_W = 232

-- A model's options, in order: the lead first (Zeron's `card_order`),
-- then the rest as the agent lists them.
local function ordered(m: Model): { Option }
	local out: { Option } = {}
	for _, o in m.options or {} do
		if o.id == LEAD then
			table.insert(out, o)
		end
	end
	for _, o in m.options or {} do
		if o.id ~= LEAD then
			table.insert(out, o)
		end
	end
	return out
end
-- The starred tab's id (no agent's).
local STARRED = "*starred"

-- The models starred (by value), kept across launches: one set every
-- picker shares (a star given in one shows in the rest), unless a picker
-- is given its own.
local FAVORITES = ui.persist("model_favorites", {} :: { [string]: boolean })

-- A list, however it was given.
local function read(given: any): any
	if type(given) == "function" then
		local call: () -> any = given
		return call()
	elseif type(given) == "userdata" then
		local state: State<any> = given
		return state:get()
	end
	return given or {}
end

return ui.component(function(props: Props): Node
	local function harness_list(): { Harness }
		return read(props.harnesses) :: { Harness }
	end
	local by_id = ui.computed(function(): { [string]: Model }
		local out: { [string]: Model } = {}
		for _, m in read(props.models) :: { Model } do
			out[m.value] = m
		end
		return out
	end)
	local agents = ui.computed(function(): { [string]: Harness }
		local out: { [string]: Harness } = {}
		for _, h in harness_list() do
			out[h.id] = h
		end
		return out
	end)
	local function current(): Model?
		return by_id:get()[props.model:get() or ""]
	end
	-- Read bottom up (the tabs by the trigger, the list over them): see
	-- Dropdown's `bottom_up`.
	local bottom_up = props.bottom_up == true
	-- The choice made of a model's option: the pick, or the option's own
	-- start (a pick the option doesn't offer reads as that too).
	local function chosen(opt: Option): Choice
		local picks = if props.options then props.options:get() else {}
		local id = picks[opt.id] or opt.default
		for _, c in opt.choices do
			if c.id == id then
				return c
			end
		end
		return opt.choices[1] or { id = opt.default, label = "" }
	end
	-- A choice picked: kept by option id; the option's own start is no pick
	-- (Zeron leaves a default out of the chat's options).
	local function pick(opt: Option, choice: string)
		if not props.options then
			return
		end
		local now = table.clone(props.options:peek())
		now[opt.id] = if choice == opt.default then nil else choice
		props.options:set(now)
	end
	local favorites: State<{ [string]: boolean }> = props.favorites or FAVORITES
	local function starred(value: string): boolean
		return favorites:get()[value] == true
	end
	local function toggle_star(value: string)
		local now = table.clone(favorites:peek())
		now[value] = if now[value] then nil else true
		favorites:set(now)
	end
	-- The tab shown: the one picked since the panel opened, else the chosen
	-- model's agent.
	local tab = ui.use_state(nil :: string?)
	-- Opened out (the tabs, the search, every model) or folded to the chosen
	-- model over its reasoning: it opens folded, opens out around the chosen
	-- model on a click, and folds back around the pick (or on Escape).
	local expanded = ui.use_state(false)
	local function viewed(): string
		local t = tab:get()
		if t then
			return t
		end
		local m = current()
		if m then
			return m.harness
		end
		-- No model chosen: the first agent's tab; with none (a catalog
		-- still coming), the starred one.
		local first = harness_list()[1]
		return if first then first.id else STARRED
	end
	local function levels(): { EffortSlider.Level }
		local m = current()
		return if m and m.efforts then m.efforts else {}
	end

	-- A model moved to keeps the effort when it has it, else takes its own.
	ui.effect(function()
		local m = current()
		if not m or not m.efforts or #m.efforts == 0 then
			return
		end
		local e = props.effort:peek()
		for _, l in m.efforts do
			if l.value == e then
				return
			end
		end
		props.effort:set(m.effort or m.efforts[math.ceil(#m.efforts / 2)].value)
	end)

	-- The models as the dropdown's items, each in its agent's mark.
	local items = ui.computed(function(): { Dropdown.Item }
		local out: { Dropdown.Item } = {}
		local by_agent = agents:get()
		for _, m in read(props.models) :: { Model } do
			local agent = by_agent[m.harness]
			table.insert(out, {
				value = m.value,
				label = m.label,
				mark = if agent then agent.mark else nil,
				svg = if agent then agent.icon else nil,
				svg_seed = m.harness,
			})
		end
		return out
	end)

	-- The tabs: the starred models, then an agent's mark each; the shown
	-- one bright, with its bar on the rule below.
	-- The corner of what sits against the panel's edge.
	local corner = control.nest("$radius.lg", 1, "$space.1")
	local function tab_of(id: string, label: Label, glyph: Node): Node
		local shown = function(): boolean
			return viewed() == id
		end
		return ui.button {
			label = label,
			role = "tab",
			shrink = 0,
			w = 32,
			h = 32,
			relative = true,
			items = "center",
			justify = "center",
			-- A button's corner, as the icon buttons beside it.
			rounded = "$zeron.icon_button.radius",
			bg = "#00000000",
			color = function()
				return if shown() then "$text.primary" else "$text.tertiary"
			end,
			hover = { bg = "$hover", color = "$text.primary" },
			active = { bg = "$pressed" },
			on_click = function()
				tab:set(id)
			end,
			glyph,
			ui.show(
				shown,
				ui.div {
					absolute = true,
					left = 8,
					right = 8,
					-- On the rule between the tabs and the list: under
					-- them, or over them when the panel reads bottom up.
					top = if bottom_up then -4 else nil,
					bottom = if bottom_up then nil else -4,
					h = 2,
					rounded = "$radius.full",
					bg = "$accent",
				}
			),
		}
	end
	local starred_tab = tab_of(
		STARRED,
		locale.t("ds.model_picker.starred"),
		Icon {
			name = function(): Icon.Name
				return if viewed() == STARRED then "star-bold" else "star"
			end,
			size = 16,
		}
	)
	-- An agent's tab each, following the agents as they come and go.
	local agent_tabs = ui.each(ui.computed(harness_list), function(h: Harness): Node
		return tab_of(h.id, h.label, AgentMark { label = h.label, seed = h.id, mark = if h.icon then nil else h.mark, icon = h.icon, size = 16, bare = true })
	end, {
		key = function(h: Harness): string
			return h.id
		end,
	})

	-- A row's star: shown under the pointer (or the keys), and always on a
	-- starred model; a click stars it or takes the star off.
	local function star(item: Dropdown.Item, lit: () -> boolean): Node
		local button: icon_button.Props = {
			-- (Followed as it's starred and unstarred.)
			label = function(): string
				return locale.s(if starred(item.value) then "ds.model_picker.unstar" else "ds.model_picker.star")
			end,
			-- (No tooltip: the row's own star says it.)
			tooltip = false,
			size = "sm",
			tone = "quiet",
			self = "center",
			-- Starred, the full ink.
			color = function()
				return if starred(item.value) then "$text.primary" else "$text.tertiary"
			end,
			on_click = function()
				toggle_star(item.value)
			end,
			icon = function(): Icon.Name
				return if starred(item.value) then "star-bold" else "star"
			end,
		}
		return ui.show(function(): boolean
			return lit() or starred(item.value)
		end, IconButton(button))
	end

	-- Fast mode as Zeron's picker shows it beside the reasoning: a bare
	-- bolt (a plate only under the pointer), filling as it turns on, the
	-- glyph's colour the state.
	local fast = props.fast
	local function bolt(): Node?
		if not fast then
			return nil
		end
		local on = fast
		local g = icon_button.glyph_size("lg")
		return IconButton {
			label = locale.t("ds.model_picker.fast_mode"),
			tooltip = locale.s("ds.model_picker.fast_mode"),
			role = "switch",
			checked = on,
			size = "lg",
			tone = "quiet",
			-- On, the accent.
			color = function()
				return if on:get() then "$accent" else "$text.tertiary"
			end,
			on_click = function()
				on:toggle()
			end,
			ui.overlay {
				w = g,
				h = g,
				Icon { name = "fast-tier", size = g },
				ui.div {
					opacity = ui.tween(on, { duration = 160 }),
					Icon { name = "fast-tier-bold", size = g },
				},
			},
		}
	end

	-- An option's row (Zeron's setting row): its name, the choice made
	-- (quiet) and a chevron, opening its choices beside the panel as the
	-- composer's Where rows do: a dropdown whose trigger is the row, the
	-- chosen one in the selected state, the option's start tagged. `before`
	-- runs ahead of a pick (a card's: picking its model first).
	local function option_row(opt: Option, before: (() -> ())?): Node
		local value = ui.use_state(chosen(opt).id :: string?)
		ui.effect(function()
			local c = chosen(opt).id
			if value:peek() ~= c then
				value:set(c)
			end
		end)
		local items: { Dropdown.Item } = {}
		for _, c in opt.choices do
			table.insert(items, { value = c.id, label = c.label, tag = if c.id == opt.default then locale.s("ds.model_picker.default") else nil })
		end
		return Dropdown {
			value = value,
			items = items,
			label = opt.label,
			search = false,
			width = CHOICES_W,
			-- Close beside the panel, as a submenu sits: 2 from its edge (the
			-- row it opens from sits 5 inside it, its inset and border).
			placement = { side = "right", align = "start", offset = 7 },
			on_choose = function(c: string)
				if before then
					before()
				end
				pick(opt, c)
			end,
			trigger = function(menu: Dropdown.Menu): Node
				local row: ContainerProps = {
					label = opt.label,
					bg = function()
						return if menu.open:get() then "$hover" else "#00000000"
					end,
					on_click = menu.toggle,
					ui.text { opt.label, text_size = "$font.size.md", color = "$text.primary", grow = 1 },
					-- The choice, quiet, held to a width (fading past it).
					FadeText {
						function(): string
							return chosen(opt).label
						end,
						text_size = "$font.size.md",
						color = "$text.tertiary",
						max_w = 120,
					},
					Menu.chevron(),
				}
				Menu.look(row)
				return ui.row(row)
			end,
		}
	end

	-- Fast mode's switch row (a model with Fast mode and no reasoning has
	-- it under its options; a card has it last).
	local function fast_row(before: (() -> ())?): Node?
		if not fast then
			return nil
		end
		local on = fast
		local row: ContainerProps = {
			on_click = function()
				if before then
					before()
				end
				on:toggle()
			end,
			ui.text { locale.t("ds.model_picker.fast_mode"), text_size = "$font.size.md", color = "$text.primary", grow = 1 },
			Switch {
				value = on,
				label = locale.t("ds.model_picker.fast_mode") :: any,
				on_change = function()
					if before then
						before()
					end
				end,
			},
		}
		Menu.look(row)
		return ui.row(row)
	end

	-- Under the list, Zeron's tray: the chosen model's options (each a row),
	-- then its reasoning (Fast mode's bolt in its header); a model with Fast
	-- mode and no reasoning has its switch row. One slider while the model
	-- has levels, following the model as it changes (its levels, its bolt),
	-- so moving between models moves its thumb rather than making it again.
	-- The rows follow the model too, each keyed by its model and option: a
	-- model's own rows come and go with it.
	local function has_fast(): boolean
		local m = current()
		return m ~= nil and m.fast == true
	end
	local accessory = bolt()
	type Owned = { model: Model, option: Option }
	local owned = ui.computed(function(): { Owned }
		local m = current()
		local out: { Owned } = {}
		for _, o in if m then ordered(m) else {} do
			table.insert(out, { model = m :: Model, option = o })
		end
		return out
	end)
	local foot: { Node } = {
		ui.each(owned, function(it: Owned): Node
			return option_row(it.option)
		end, {
			key = function(it: Owned): string
				return `{it.model.value}/{it.option.id}`
			end,
			w_full = true,
			gap = "$zeron.list.gap",
		}),
		ui.show(
			function(): boolean
				return #levels() > 0
			end,
			ui.div {
				w_full = true,
				-- The groove as wide as a row's wash above it (the foot's own
				-- inset); room above and below.
				py = "$space.1",
				EffortSlider {
					value = props.effort,
					levels = levels,
					title = locale.t("ds.model_picker.reasoning"),
					accessory = if accessory then ui.show(has_fast, accessory) else nil,
				},
			}
		),
	}
	local switch_row = fast_row()
	if switch_row then
		table.insert(
			foot,
			ui.show(function(): boolean
				local m = current()
				return m ~= nil and m.fast == true and (m.efforts == nil or #m.efforts == 0)
			end, switch_row)
		)
	end

	return Dropdown {
		value = props.model,
		items = items :: any,
		tone = props.tone or "chip",
		mark = true,
		detail = function(): string
			local e = props.effort:get()
			for _, l in levels() do
				if l.value == e then
					return l.label
				end
			end
			return ""
		end,
		-- (Dropdown hands its label on, as the trigger's name.)
		label = props.label or locale.t("ds.model_picker.model") :: any,
		search = function(): string
			if viewed() == STARRED then
				return locale.s("ds.model_picker.search_starred")
			end
			local agent = agents:get()[viewed()]
			return if agent then locale.s("ds.model_picker.search_agent", { agent = agent.label }) else locale.s("ds.model_picker.search_all")
		end,
		filter = function(item: Dropdown.Item): boolean
			if viewed() == STARRED then
				return starred(item.value)
			end
			local m = by_id:get()[item.value]
			return m ~= nil and m.harness == viewed()
		end,
		trailing_item = star,
		-- Starred and nothing starred: how to star one. A search that finds
		-- nothing: what it looked for, and where.
		empty = function(q: string): Dropdown.Empty
			local agent = agents:get()[viewed()]
			if q == "" then
				if viewed() == STARRED then
					return { title = locale.s("ds.model_picker.no_starred"), hint = locale.s("ds.model_picker.no_starred_hint") }
				end
				return {
					title = locale.s("ds.model_picker.no_models"),
					hint = if agent then locale.s("ds.model_picker.none_to_offer", { agent = agent.label }) else nil,
				}
			end
			local where = if viewed() == STARRED
				then locale.s("ds.model_picker.where_starred")
				elseif agent then agent.label
				else locale.s("ds.model_picker.where_models")
			return { title = locale.s("ds.model_picker.not_found"), hint = locale.s("ds.model_picker.not_found_hint", { where = where, q = q }) }
		end,
		on_open = function()
			tab:set(nil)
			expanded:set(false)
		end,
		expanded = function(): boolean
			return expanded:get()
		end,
		on_expand = function()
			expanded:set(true)
		end,
		keep_open = true,
		on_choose = function()
			expanded:set(false)
			tab:set(nil)
		end,
		on_escape = function(): boolean
			-- Opened out, Escape folds it back; folded, it closes.
			if expanded:peek() then
				expanded:set(false)
				return true
			end
			return false
		end,
		width = 300,
		open_with = props.open_with,
		-- The tabs scroll across when there are more than fit, each edge
		-- fading only where more runs past it. (36 tall, 4 into the head's
		-- inset by the list: the shown tab's bar meets the rule.)
		head = ui.scroll_x {
			-- Its tabs and buttons sit in the panel's corner: concentric with it.
			theme = { zeron = { icon_button = { radius = corner } } },
			w_full = true,
			h = 36,
			mt = if bottom_up then ui.raw(-4, "the tab bar meets the rule") else nil,
			mb = if bottom_up then nil else ui.raw(-4, "the tab bar meets the rule"),
			fade_x = { 20, 20 },
			fade_overflow = true,
			ui.row {
				h = 36,
				gap = "$zeron.space.hair",
				-- Its tabs to the side away from the list, so the bar (4 past
				-- them, on the rule by the list) stays inside the strip's clip.
				items = if bottom_up then "end" else "start",
				starred_tab,
				agent_tabs,
			},
		},
		-- (The slider's groove in the panel's foot corners, its fill 2 inside.)
		extra = ui.column {
			w_full = true,
			theme = { radius = { md = corner, sm = control.nest(corner, "$zeron.space.hair") } },
			table.unpack(foot),
		},
		extra_when = function(): boolean
			local m = current()
			return m ~= nil
				and ((m.efforts ~= nil and #m.efforts > 0) or (m.fast == true and fast ~= nil) or #ordered(m) > 0)
		end,
		placement = props.placement,
		bottom_up = bottom_up,
	}
end)
