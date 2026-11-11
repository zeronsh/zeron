-- Zeron's composer, from the design system's parts: a pill holding the
-- draft, + on the left to attach, the microphone and Send on the right;
-- under it a footer of chips the same size: the access mode (with plan
-- mode's switch in its panel) and where it works (a folder or worktree,
-- the project › the branch, its panel choosing the device and checkout too)
-- on the left; on the right the model, and a ring for how much of its
-- context is used (hovered, a card of the numbers). Over a wallpaper (a new
-- chat's art) the pill's glass continues under the footer as a tray, so
-- the chips sit on the composer rather than on the art. The
-- footer keeps to one line: short of room, it folds a step at a time:
-- "Plan" goes, then the access mode's name (leaving its icon, in the accent
-- while plan mode is on), then the branch's, leaving its icon at the left. Its geometry is Zeron's
-- (crates/ui/src/composer.rs): 26-point corners, 28-point round controls,
-- one 49-point line while the draft fits beside the controls.
--
-- Past that (or always, with `expanded`, as a new chat's is) it opens out:
-- the text on top, growing with it from 76 to 260 points and scrolling past
-- that, the controls in a row beneath. The pill grows from its bottom edge
-- (it sits at the foot of a column), the controls stay put, and only the
-- text moves.
--
-- Everything else is in the pill or behind it. In it, over the draft: the
-- cargo (ds/composer_cargo: what the message carries, as chips), or what
-- the agent asks (ds/composer_ask: a question's options, an approval), the
-- draft then being the answer. Over it, the deck (ds/composer_dock: the
-- alerts, the queue, subagents, what is running, the plan): folded, a pill
-- floating over it; opened, its own top. A notice (offline, a failed send,
-- a draft restored) is the first of the alerts, there till it's over.
-- Mentions (ds/composer_mentions) open at the caret. Dictating, a track
-- unrolls from the microphone over the controls: a dot, "Listening", the
-- voice, a clock.
--
-- `Composer { value = draft, model = model, models = { … }, on_send = … }`.

local Button = require("@self/button")
local Icon = require("@self/icon")
local control = require("@self/control")
local FadeText = require("@self/fade_text")
local Tile = require("@self/tile")
local Dropdown = require("@self/dropdown")
local ModelPicker = require("@self/model_picker")
local Switch = require("@self/switch")
local Menu = require("@self/menu")
local IconButton = require("@self/icon_button").IconButton
local status = require("@self/status")
local markdown = require("@self/markdown_edit")
local material = require("@self/material")
local DropOverlay = require("@self/drop_overlay")
local cargo = require("@self/composer_cargo")
local Lightbox = require("@self/lightbox")
local dock = require("@self/composer_dock")
local ask = require("@self/composer_ask")
local mentions = require("@self/composer_mentions")
local locale = require("@self/locale")
local VoiceWave = require("@self/voice_wave").VoiceWave
local context_usage = require("@self/context_usage")
local motion = require("@self/motion")
local EditorMenu = require("@self/editor_menu")

-- What the model's context holds, as its agent reports it: the tokens in
-- it and the window's size (either may be missing).
export type Usage = context_usage.Usage

-- A device it can run on: its name; Cloud (a machine of its own), this
-- device ("You"), or one out of reach.
-- `disabled`: shown but not to be picked, `description` saying why.
export type Device = {
	value: string,
	label: string,
	cloud: boolean?,
	you: boolean?,
	offline: boolean?,
	description: string?,
	disabled: boolean?,
}
-- A line above the composer: its tone (a dot in its ink; danger, an
-- "Error" badge), what it says, a spinner in place of the dot while
-- something is on its way, an action at its end (its name; the composer's
-- `on_notice_action` does it), and whether it can be put away
-- (`on_dismiss_notice`). Data alone: a state holds no functions.
export type Notice = {
	tone: ("neutral" | "info" | "success" | "warning" | "danger")?,
	text: string,
	spinner: boolean?,
	action: string?,
	dismissable: boolean?,
	-- Worth knowing, not worth raising: read with the deck's alerts once
	-- they're open, never putting up the pill on its own.
	quiet: boolean?,
}
export type Answer = { values: { string }, text: string }
-- Where an existing chat works (`branch_fixed`).
export type Fixed = { device: string?, name: string?, worktree: boolean, project: string?, hue: string?, src: string?, emoji: string? }
export type Props = {
	value: State<string>,
	placeholder: (string | Message)?,
	-- What sends (the shipping app's setting): "enter", ↩ sends and ⇧↩
	-- breaks the line; "mod_enter", ⌘↩ sends and ↩ breaks the line. ⌘↩
	-- sends either way. Enter by default.
	send_with: (() -> string)?,
	-- A name to keep its measures and footer fold under, across the
	-- composer being made again (a reload), so it is laid out as it was
	-- from its first frame rather than growing into place. Without one,
	-- each composer measures afresh.
	keep: string?,
	-- Always opened out (a new chat's), rather than one line while it fits.
	expanded: (boolean | () -> boolean)?,
	-- Moved as the composer is shown for something else (a new chat's
	-- page or a session's, as the route switches): its form and size are
	-- there at once for a moment after, not eased. Growing in place (more
	-- lines, a tray, chips) still eases.
	snap: State<number>?,
	-- A turn is running: Send becomes Stop while the draft is empty, and
	-- queues what is typed.
	running: State<boolean>?,
	-- What the agent may do unasked: "supervised", "edits" or "full"; and
	-- plan mode (explore and propose before changing anything).
	access: State<string?>?,
	plan: State<boolean>?,
	-- The model (a ModelPicker: its agents' tabs, its models, its reasoning,
	-- Fast mode and its other options' picks).
	model: State<string?>,
	effort: State<string?>,
	fast: State<boolean>?,
	options: State<{ [string]: string }>?,
	-- The agents and their models (ds.ModelPicker's: lists, or states or
	-- functions of them, followed in place as a catalog comes and changes).
	harnesses: { ModelPicker.Harness } | State<{ ModelPicker.Harness }> | () -> { ModelPicker.Harness },
	models: { ModelPicker.Model } | State<{ ModelPicker.Model }> | () -> { ModelPicker.Model },
	models_loading: (() -> boolean)?,
	-- The model's name while the catalog doesn't list it (ds.ModelPicker's
	-- `model_label`).
	model_label: (() -> string?)?,
	-- What runs while no model is chosen, shown and not sent
	-- (ds.ModelPicker's `default_model`).
	default_model: (() -> string?)?,
	-- A model's agent's accounts, putting one in use, and asking after them
	-- again (ds.ModelPicker's).
	accounts: ((harness: string, model: string) -> { ModelPicker.Account }?)?,
	on_account: ((harness: string, id: string) -> ())?,
	on_accounts_open: (() -> ())?,
	-- How much of the model's context is used (0 to 1): a ring beside its
	-- chip.
	context: (number | () -> number)?,
	-- Or what it holds (tokens and window, as the agent reports them): the
	-- ring shows only once a window is reported (an agent that never says
	-- one has none), and hovering or focusing it opens a card saying the
	-- numbers, as Zeron's does (crates/ui/src/context_usage.rs).
	usage: (() -> Usage?)?,
	-- The footer stands over a wallpaper (a new chat's art): the pill's glass
	-- continues under it as a tray, the chips on it rather than on the art,
	-- easing in and out as the art comes and goes. Without, it stays bare.
	over_art: (() -> boolean)?,
	-- Where it works: the device, the checkout ("local" or "worktree") and
	-- the branch, one chip; its panel lists the branches under rows
	-- choosing the device and the checkout.
	device: State<string?>?,
	devices: ({ Device } | State<{ Device }> | () -> { Device })?,
	checkout: State<string?>?,
	branch: State<string?>?,
	branches: ({ Dropdown.Item } | State<{ Dropdown.Item }> | () -> { Dropdown.Item })?,
	-- What the branches' list says with none to show (still loading, none
	-- found, none matching what is searched for).
	branches_empty: ((query: string) -> Dropdown.Empty)?,
	-- Where an existing chat works, fixed (its branch, live, whether its
	-- folder is a worktree, and its project's name, hue and artwork): the
	-- chip says so and opens nothing, as a chat can't move. Nil (a new
	-- chat): the picker.
	branch_fixed: (() -> Fixed?)?,
	on_send: ((text: string) -> ())?,
	on_stop: (() -> ())?,
	-- A bare Escape no one else took (no mention list, question or
	-- dictation): true if it did something with it.
	on_escape: (() -> boolean)?,
	on_attach: (() -> ())?,
	-- Its menus open below it (a composer with room under it: a new
	-- session's, floating mid-page); by default, above (one at the window's
	-- foot). A function follows where the composer is (one travelling
	-- between the two).
	menus_below: (boolean | () -> boolean)?,
	-- The project it works in ("none": no project), the projects to pick
	-- from, and making a new one.
	project: State<string?>?,
	projects: ({ Project } | State<{ Project }> | () -> { Project })?,
	on_new_project: (() -> ())?,

	-- What the message carries (chips over the draft), and what to do about
	-- one: take it off, try its upload again, open it.
	cargo: State<{ cargo.Item }>?,
	on_remove_cargo: ((id: string) -> ())?,
	on_retry_cargo: ((id: string) -> ())?,
	on_open_cargo: ((id: string) -> ())?,
	-- Text pasted in: true takes it (the caller made a chip of it).
	on_paste_text: ((text: string) -> boolean)?,
	-- A paste with no text (a picture on the clipboard): the caller's to
	-- read, true taking it. Files copied in a file manager go to `on_files`.
	on_paste_image: (() -> boolean)?,
	-- Files dropped on the pill.
	on_files: ((files: { any }) -> ())?,
	-- Files are being dragged over it (the caller's, for a state of its
	-- own; otherwise the pill's own sense of it).
	dropping: State<boolean>?,

	-- The trays behind it (ds/composer_dock's).
	queue: State<{ dock.Queued }>?,
	can_steer: (boolean | () -> boolean)?,
	on_queue_send: ((id: string) -> ())?,
	on_queue_edit: ((id: string) -> ())?,
	on_queue_remove: ((id: string) -> ())?,
	on_queue_reorder: ((from: number, to: number) -> ())?,
	on_queue_save: ((id: string) -> ())?,
	on_queue_cancel: ((id: string) -> ())?,
	todos: State<{ dock.Todo }>?,
	on_dismiss_todos: (() -> ())?,
	tasks: State<{ dock.Task }>?,
	on_stop_task: ((id: string) -> ())?,
	on_stop_all: (() -> ())?,
	on_open_task: ((id: string) -> ())?,
	tray: State<dock.Tray?>?,
	-- What to know about the chat (ds/composer_dock's alerts): each in its
	-- tone's colours, its action, and letting one go.
	alerts: State<{ dock.Alert }>?,
	on_alert_action: ((id: string) -> ())?,
	on_dismiss_alert: ((id: string) -> ())?,
	-- Another pill over its middle ("Scroll to end"): its width while it
	-- shows, so the folded trays' pill moves aside for it.
	aside: (() -> number?)?,
	-- The trays' pill's width while it shows (for "Scroll to end" to make
	-- room in turn).
	on_pill_width: ((w: number?) -> ())?,

	-- What the agent asks: a question (its chosen options kept by the
	-- caller), or an approval.
	question: State<ask.Question?>?,
	chosen: State<{ [string]: boolean }>?,
	on_pick: ((value: string) -> ())?,
	on_answer: ((answer: Answer) -> ())?,
	on_back: (() -> ())?,
	on_close_question: (() -> ())?,
	approval: State<ask.Approval?>?,
	on_approve: ((scope: "once" | "always") -> ())?,
	on_deny: (() -> ())?,
	-- Whether an approval can be given for good ("Always allow"); by
	-- default, yes. An agent that only asks each time hides it.
	can_always: (boolean | () -> boolean)?,

	-- The line above it, what its action does, and putting it away.
	notice: State<Notice?>?,
	on_notice_action: (() -> ())?,
	on_dismiss_notice: (() -> ())?,
	-- Dictating: the track over the controls; the mic starts and stops it,
	-- Escape or the × cancels.
	dictating: State<boolean>?,
	on_dictate: (() -> ())?,
	on_cancel_dictation: (() -> ())?,
	-- Dictation for real (screens/chat/dictation's `bind`): its microphone,
	-- the editor's events it follows, and Send waiting for its words. With
	-- its `levels` and `phase`, the track's waveform follows the microphone
	-- (ds/voice_wave); without, it moves from the time alone.
	dictation: any?,
	-- Nothing can be written (a chat archived, an agent gone): dimmed, the
	-- notice saying why.
	disabled: (boolean | () -> boolean)?,
	-- What `@`, `/` and `$` offer.
	mentions: any?,
	-- Short of room (the caller knows; else the pill's own width says).
	narrow: (() -> boolean)?,
	-- Bumped, it takes the keys, the caret at the draft's end (a shortcut
	-- that goes to the composer).
	focus: State<number>?,
	-- Its controls' row's id (the window's one composer's, for selectors).
	toolbar_id: string?,
	-- False: files over it show no overlay on the pill (the caller draws
	-- one over a wider area).
	drop_overlay: boolean?,
	-- Bumped, the model picker opens (a shortcut to it).
	open_model: State<number>?,
	-- Set to whether the model picker is open.
	model_open: State<boolean>?,
	-- Which of `@`, `/`, `$` open their list (an agent's completion
	-- settings), and what an empty list says, by trigger.
	mention_enabled: ((trigger: string) -> boolean)?,
	-- A mention's row taken (after its token lands in the draft): its item
	-- and trigger (a command's or skill's reference, remembered for the
	-- send).
	on_mention: ((item: any, trigger: string, token: string) -> ())?,
	mention_empty: { [string]: string }?,
	-- Whose draft it shows (a composer that moves between chats: the
	-- chat), so a mention list put away is that draft's alone.
	draft_key: (() -> any)?,
	-- A trigger whose list is a launcher, by trigger: its sections, in order
	-- (ds/composer_mentions `sections`).
	mention_sections: { [string]: { { id: string, label: any } } }?,
}

-- Zeron's geometry. The one line: 47 points inside the hairline, the text
-- centred in it. Opened out: the text box (16 above the text, 4 below)
-- between its floor and cap, then the controls' row (2, a 32-point chip
-- zone, 8). Round controls are 28 across, concentric with the pill's
-- corner: on one line (a pill, its corner half its height) centred in it,
-- as far from its sides as from its foot; opened out, as far in from the
-- side and the foot as the pill's corner (inside its hairline) is round
-- less theirs.
local LINE = 22
local ONE_LINE = 47
local TEXT_TOP, TEXT_BOTTOM, TEXT_MIN, TEXT_MAX = 16, 4, 76, 260
local ACTIONS = 42
local HAIRLINES = 2
local CONTROL = 28
local INSET_LINE = (ONE_LINE - CONTROL) / 2
local function open_inset(): number
	return control.nest("$zeron.composer.radius", 1) - CONTROL / 2
end
-- The controls' row opened out: its 42 points, or more where a rounder
-- pill (a template's) sets its controls further in from the foot, so they
-- keep the room over them they have at Zeron's own corner (its controls'
-- top 3 under the row's) rather than rising into the text.
local ACTIONS_OVER = 3
local function actions(): number
	return math.max(ACTIONS, open_inset() + CONTROL + ACTIONS_OVER)
end
-- The draft's own inset either side on one line, and opened out.
local TEXT_SIDE = 8
local TEXT_OPEN_SIDE = 16
-- Opened out, the pill folds back to one line only once the text is this
-- much narrower than the line holds, so a width at the boundary cannot
-- flicker between the two.
local HYSTERESIS = 32
-- How wide where-it-works stays before the access mode folds: its project's
-- name (to PROJECT_MAX) and BRANCH_KEEP of the branch's, with the chip's
-- own room (its padding, the glyph, the gaps, the chevron); until
-- the name is measured, BRANCH_MIN. And the footer's side padding and gap
-- (snug, space.1).
local BRANCH_MIN = 156
local BRANCH_KEEP, PLACE_CHROME = 72, 50
-- The project's name in that chip: at least this much of it stays, and it
-- grows to at most this. The branch's at most, and the sliver of it that
-- stays (fading) once the access mode has folded: the branch gives way
-- first, to that, then the name to its least, then the chip folds to its
-- glyph. (Never a bare "›" with nothing after it.)
local PROJECT_MIN, PROJECT_MAX, BRANCH_MAX, BRANCH_FLOOR = 44, 140, 180, 36
-- The device's name before them: how wide at most, kept before the access
-- mode folds, and least.
local DEVICE_MAX, DEVICE_KEEP, DEVICE_MIN = 120, 72, 36
-- What a chevron between two of them takes, with its gap.
local PLACE_CHEVRON = 14
-- The footer's chips: one height, one corner (the menus' row corner),
-- each lead (a glyph's 12) 6 in all round, and as far from its words.
local CHIP_H, CHIP_RADIUS = 24, "$radius.lg"
-- Its words lifted a point, as the footer's other chips' are (ds/dropdown's
-- chip): centred by their capitals, lowercase reads low beside a glyph. (A
-- margin under them of twice that: their box stays centred, a point up.)
local LIFT = "$zeron.space.hair"
type Folded = { level: number, access_at: { [string]: number }, where_icon: number?, model_at: { [string]: number }? }
local FOOTER_SIDES, FOOTER_GAP = 20, 4
-- With cargo or a question over the draft, the draft keeps the strip's
-- `below` over it (the theme's; read as the composer is built) rather than
-- the open text box's own top.
local function head_gap(): number
	local v = ui.theme.get("zeron.composer.chip.below")
	return if type(v) == "number" then v else 12
end
-- A pill narrower than this is short of room: the trays' rows fold their
-- thumbnails and actions.
local NARROW = 480
-- Dictation's limit, and when its clock warns.
local VOICE_LIMIT, VOICE_WARN = 60, 10

local function read<T>(v: T | () -> T): T
	return if type(v) == "function" then (v :: () -> T)() else v :: T
end

-- A table some of whose fields are words: a copy of `t` with each of
-- `words` (field → key) its message's text now. Made where it's read (a
-- list given as a function, read as it's shown), it follows the locale.
local function worded(t: { [string]: any }, words: { [string]: string }): any
	local out = table.clone(t)
	for field, key in words do
		out[field] = locale.s(key)
	end
	return out
end

-- The access modes, as Zeron offers them; their icons (Central Icons,
-- centralicons.com, round outlined, radius 2, stroke 1.5) say what each
-- does: it waits for you (a raised hand), edits go through (a pencil with
-- sparkles), nothing is held back (an open padlock). (Made as they're
-- read, in the locale's words.)
local function ACCESS(): { Dropdown.Item }
	return {
	worded({ value = "supervised", icon = "access-supervised" }, {
		label = "ds.composer.access.supervised",
		description = "ds.composer.access.supervised_hint",
	}),
	worded({ value = "edits", icon = "access-edits" }, {
		label = "ds.composer.access.edits",
		description = "ds.composer.access.edits_hint",
	}),
	worded({ value = "full", icon = "access-full" }, {
		label = "ds.composer.access.full",
		description = "ds.composer.access.full_hint",
	}),
	}
end

-- Plan mode's row at the access panel's foot: an item's shape, its icon on
-- its title's line, its switch (centred) where the check would be. The row
-- (or the switch) flips it.
local function plan_row(plan: State<boolean>): Node
	local row: ContainerProps = {
		on_click = function()
			plan:toggle()
		end,
		Menu.glyph("map-arrow-right"),
		ui.column {
			grow = 1,
			min_w = 0,
			gap = "$zeron.space.hair",
			ui.text {
				locale.t("ds.composer.plan.mode"),
				text_size = "$font.size.md",
				line_height = "$font.line.md",
				weight = "$font.weight.medium",
				color = "$text.primary",
			},
			ui.text {
				locale.t("ds.composer.plan.hint"),
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = "$text.tertiary",
				wrap = true,
			},
		},
		ui.div { self = "center", Switch { value = plan, label = locale.t("ds.composer.plan.mode") :: any } },
	}
	Menu.look(row, true)
	return ui.row(row)
end

-- A choice in a nested menu: an item of a dropdown (its name and icon, a
-- line under its name saying what it is, a muted tag, a warning that it is
-- out of reach).
type Choice = Dropdown.Item

-- The checkouts, as Zeron offers them (made as they're read).
local function CHECKOUTS(): { Choice }
	return {
	worded({ value = "local", icon = "folder" }, {
		label = "ds.composer.checkout.local",
		description = "ds.composer.checkout.local_hint",
	}),
	worded({ value = "worktree", icon = "worktree" }, {
		label = "ds.composer.checkout.worktree",
		description = "ds.composer.checkout.worktree_hint",
	}),
	}
end

-- A menu row, the menus' own: compact, full width, washed under the
-- pointer.
local function menu_row(props: ContainerProps): Node
	local row = table.clone(props)
	Menu.look(row)
	return ui.row(row)
end

-- A row's glyph, in an item tile's box: the menus'.
local glyph = Menu.glyph

-- A row of the panel's head that opens its choices beside the panel: a
-- dropdown (its search, keys and list) whose trigger is the row, showing
-- the choice made (its icon, its name, a warning, a chevron). A press
-- anywhere else in the panel closes it, as zgx closes any popover on a
-- press outside it.
-- A list, however it was given (a list, or a state or function of one).
local function list_of(given: any): any
	if type(given) == "function" then
		local call: () -> any = given
		return call()
	elseif type(given) == "userdata" then
		local state: State<any> = given
		return state:get()
	end
	return given or {}
end

local function submenu(
	siblings: { State<boolean> },
	label: Message,
	value: State<string?>,
	choices: { Choice } | () -> { Choice },
	fallback: number,
	-- (A string: Dropdown names its field by it only so.)
	search: string?,
	actions: { Dropdown.Action }?,
	-- Choices named by the row but not listed (one an action picks).
	unlisted: ({ Choice } | () -> { Choice })?
): Node
	local function current(): Choice
		local v = value:get()
		local listed = list_of(choices) :: { Choice }
		for _, list in { listed, list_of(unlisted) :: { Choice } } do
			for _, c in list do
				if c.value == v then
					return c
				end
			end
		end
		-- (None yet, as a list still coming: a blank row.)
		return listed[fallback] or listed[1] or { value = "", label = "" }
	end
	local own: State<boolean>? = nil
	local function close_siblings()
		for _, open in siblings do
			if open ~= own then
				open:set(false)
			end
		end
	end
	return Dropdown {
		on_open = close_siblings,
		value = value,
		items = choices,
		actions = actions,
		-- (Dropdown hands its label on, as the trigger's name.)
		label = label :: any,
		search = search or false,
		width = 260,
		submenu = true,
		-- Close beside the panel, as a submenu sits: 2 from its edge (the
		-- row it opens from sits 5 inside it, its inset and border).
		placement = { side = "right", align = "start", offset = 7 },
		trigger = function(menu: Dropdown.Menu): Node
			own = menu.open
			table.insert(siblings, menu.open)
			return ui.menu_item(Menu.look {
				label = label,
				bg = function()
					return if menu.open:get() then "$hover" else "#00000000"
				end,
				-- A press opens it, as hovering does; one already open stays
				-- open (a hover opened it just before), as a system menu's row.
				on_click = function()
					close_siblings()
					if not menu.open:peek() then
						menu.toggle()
					end
				end,
				on_hover = function(inside: boolean)
					if menu.on_hover then
						menu.on_hover(inside)
					end
					if inside then
						close_siblings()
						if not menu.open:peek() then
							menu.toggle()
						end
					end
				end,
				-- The choice's own tile (a project's hue or artwork), else its
				-- icon: one or the other, following the choice.
				ui.presence(function(): string
					local c = current()
					return if c.hue or c.src or c.emoji then "tile" else "icon"
				end, function(kind: string): Node
					if kind == "tile" then
						return Tile {
							label = function(): string?
								return current().label
							end,
							hue = function(): string?
								return current().hue
							end,
							src = function(): string?
								return current().src
							end,
							emoji = function(): string?
								return current().emoji
							end,
							-- (A project's choice: its tile `.project-icon`.)
							project = function(): string?
								return (current() :: any).project
							end,
							size = 18,
						}
					end
					return glyph(function(): Icon.Name
						return current().icon or "monitor"
					end)
				end),
				FadeText {
					function()
						return current().label
					end,
					text_size = "$font.size.md",
					color = "$text.primary",
					grow = 1,
				},
				ui.show(function()
					return current().warn == true
				end, ui.div {
					self = "center",
					shrink = 0,
					color = "$warning",
					Icon { name = "wifi-off", size = 12 },
				}),
				Menu.chevron(),
			})
		end,
	}
end

-- The project a chat works in (or none), and the projects offered.
-- `warn` and `disabled` (with `description` saying why): one out of reach
-- (a project no longer where it was).
export type Project = {
	value: string,
	label: string,
	hue: string?,
	src: string?,
	emoji: string?,
	warn: boolean?,
	disabled: boolean?,
	description: string?,
	-- A folder with no git: it can't make a worktree.
	plain: boolean?,
}
-- What working in no project is, among the projects.
local NO_PROJECT = "none"

-- Whether the device chosen is Cloud (a device row marked `cloud`): a
-- machine of its own, so no checkout to choose.
local function on_cloud(device: State<string?>?, devices: any): boolean
	if not device or not devices then
		return false
	end
	local chosen = device:get()
	for _, d in list_of(devices) :: { Device } do
		if d.value == chosen then
			return d.cloud == true
		end
	end
	return false
end

-- The head of where-it-works's panel: "Work in", then the device it runs
-- on, the project there and the checkout, each a row opening its choices
-- beside the panel (the project's with "New project" under them).
local function work_in(
	project: State<string?>?,
	projects: ({ Project } | State<{ Project }> | () -> { Project })?,
	on_new_project: (() -> ())?,
	checkout: State<string?>?,
	device: State<string?>?,
	devices: ({ Device } | State<{ Device }> | () -> { Device })?
): Node
	local siblings: { State<boolean> } = {}
	local rows: { Node } = {
		Menu.Heading { locale.t("ds.composer.work_in") },
	}
	if device and devices then
		-- Following the devices as they come and go; with none, no row.
		local function choices(): { Choice }
			local out: { Choice } = {}
			for _, d in list_of(devices) :: { Device } do
				local icon: Icon.Name = if d.cloud then "cloud" else "monitor"
				table.insert(out, {
					value = d.value,
					label = d.label,
					icon = icon,
					tag = if d.you then locale.s("ds.composer.device.you") else nil,
					warn = d.offline,
					description = d.description,
					disabled = d.disabled,
				})
			end
			return out
		end
		table.insert(
			rows,
			ui.show(function(): boolean
				return #choices() > 0
			end, submenu(siblings, locale.t("ds.composer.device.label"), device, choices, 1, locale.s("ds.composer.device.search")))
		)
	end
	if project and projects then
		-- Following the projects as they come and go.
		local function choices(): { Choice }
			local out: { Choice } = {}
			for _, p in list_of(projects) :: { Project } do
				local icon: Icon.Name? = if p.hue or p.src or p.emoji then nil else "folder"
				table.insert(out, {
					value = p.value,
					label = p.label,
					hue = p.hue,
					src = p.src,
					emoji = p.emoji,
					icon = icon,
					warn = p.warn,
					disabled = p.disabled,
					description = p.description,
				})
			end
			return out
		end
		-- Under the projects, together: working in none, and making one. (Not
		-- none on Cloud: its machine clones a project's repository.)
		local p = project
		table.insert(
			rows,
			submenu(siblings, locale.t("ds.composer.project.label"), project, choices, 1, locale.s("ds.composer.project.search"), {
				worded({
					icon = "close",
					on_click = function()
						p:set(NO_PROJECT)
					end,
					shown = function(): boolean
						return not on_cloud(device, devices)
					end,
				}, { label = "ds.composer.project.none_action" }),
				worded({ icon = "plus", on_click = on_new_project }, { label = "ds.composer.project.new" }),
			}, function(): { Choice }
				return { worded({ value = NO_PROJECT, icon = "folder" }, { label = "ds.composer.project.none" }) }
			end)
		)
	end
	if checkout then
		-- Whether the project chosen is a folder with no git.
		local function plain(): boolean
			if not project or not projects then
				return false
			end
			local v = project:get()
			for _, pr in list_of(projects) :: { Project } do
				if pr.value == v then
					return pr.plain == true
				end
			end
			return false
		end
		-- A checkout is a project's: with none, no row; nor in a folder with
		-- no git, with only itself to work in; nor on Cloud, whose machine
		-- checks the repository out on its own.
		table.insert(
			rows,
			ui.show(function(): boolean
				return (project == nil or project:get() ~= NO_PROJECT) and not plain() and not on_cloud(device, devices)
			end, submenu(siblings, locale.t("ds.composer.checkout.label"), checkout, CHECKOUTS, 2))
		)
	end
	return ui.column { w_full = true, table.unpack(rows) }
end

-- The ring, in a chip-high box of its own, washed under the pointer as the
-- chips beside it are. Given what the context holds, it opens the card
-- above (below, with the menus below) as the pointer rests on it or the
-- keys reach it.
local function ring(used: number | () -> number, usage: (() -> Usage?)?, side: () -> string): Node
	local function fraction(): number
		if usage then
			return context_usage.fraction(usage()) or 0
		end
		return math.clamp(if type(used) == "function" then used() else used, 0, 1)
	end
	local function build(at: string?): Node
		return ui.row {
			id = "composer-context",
			h = CHIP_H,
			px = "$zeron.space.tight",
			items = "center",
			rounded = CHIP_RADIUS,
			hover = { bg = "$hover" },
			focusable = usage ~= nil,
			label = locale.t("ds.composer.context_used"),
			tooltip = if usage then context_usage.Card { usage = usage } else nil,
			tooltip_placement = if at then { side = at, align = "end", offset = 8 } else nil,
			tooltip_delay = 150,
			context_usage.Ring { fraction = fraction },
		}
	end
	if not usage then
		return build(nil)
	end
	-- (A placement is read as it's built: one following the composer is
	-- built again as it changes sides.)
	return ui.presence(side, build)
end

-- Where it works, as a line: where it runs (a worktree's glyph, Cloud's,
-- or a folder: a local checkout's, or no project's), the project's name ›
-- the branch. No project: the folder and "No project". The branch gives
-- way first as room runs short, to a sliver, then the name, to
-- PROJECT_MIN; folded, the glyph alone. Its ink is the chip's own (the
-- chevron a step quieter), so the chip's hover lifts all of it.
type Place = {
	-- The device it runs on, by name (nil: none to say).
	device: (() -> string?)?,
	-- The project (nil: none named), and whether it works in one at all.
	project: () -> Project?,
	in_project: () -> boolean,
	branch: () -> string,
	-- Where it runs off the branch: "worktree", "cloud", or nil (a local
	-- checkout).
	glyph: () -> Icon.Name?,
	folded: () -> boolean,
	-- Told the name's own width as it's laid out (the footer's fold reads it).
	-- What the line takes of the name and the branch: their own widths, or 0
	-- for one it doesn't show.
	on_measures: ((name: number, branch: number, device: number) -> ())?,
}
local function place_line(p: Place): { Child }
	local function has_project(): boolean
		return p.project() ~= nil
	end
	local function name(): string
		local pr = p.project()
		return if pr then pr.label else locale.s("ds.composer.project.none")
	end
	local function unfolded(): boolean
		return not p.folded()
	end
	-- The name's and the branch's own widths, measured unseen: each keeps
	-- its least of itself, or all of a shorter one (never padded out).
	local natural, branch_natural, device_natural = ui.ref(), ui.ref(), ui.ref()
	local function device_name(): string
		return if p.device then p.device() or "" else ""
	end
	local function has_device(): boolean
		return device_name() ~= ""
	end
	local function branch_min(): number
		return math.min(BRANCH_FLOOR, math.ceil(branch_natural.width or BRANCH_FLOOR))
	end
	if p.on_measures then
		local on_measures = p.on_measures
		ui.effect(function()
			local name_w, branch_w = natural.width, branch_natural.width
			if name_w and branch_w then
				on_measures(
					if has_project() or not p.in_project() then name_w else 0,
					if p.in_project() then branch_w else 0,
					if has_device() then (device_natural.width or 0) + PLACE_CHEVRON else 0
				)
			end
		end)
	end
	-- What leads: where it runs (a worktree's glyph, Cloud's, or a folder:
	-- a local checkout's, or no project's).
	local function lead(): string
		return if p.in_project() then p.glyph() or "folder" else "folder"
	end
	return {
		ui.text {
			name,
			ref = natural,
			absolute = true,
			left = 0,
			top = 0,
			opacity = 0,
			pointer_events = "none",
			aria_hidden = true,
		},
		ui.text {
			p.branch,
			ref = branch_natural,
			absolute = true,
			left = 0,
			top = 0,
			opacity = 0,
			pointer_events = "none",
			aria_hidden = true,
		},
		ui.text {
			device_name,
			ref = device_natural,
			absolute = true,
			left = 0,
			top = 0,
			opacity = 0,
			pointer_events = "none",
			aria_hidden = true,
		},
		ui.presence(lead, function(kind: string): Node
			-- (Its kind named, for tests: "place-lead-worktree".)
			return ui.div { class = `place-lead-{kind}`, Icon { name = kind :: Icon.Name, size = 12 } }
		end, { shrink = 0, w = 12, h = 12, items = "center", justify = "center", id = "composer-place-lead" }),
		ui.show(function(): boolean
			return unfolded() and has_device()
		end, ui.row {
			shrink = 1,
			min_w = function(): number
				return PLACE_CHEVRON + math.min(DEVICE_MIN, math.ceil(device_natural.width or DEVICE_MIN))
			end :: any,
			gap = "$space.1",
			items = "center",
			FadeText {
				device_name,
				roll = true,
				id = "composer-place-device",
				mb = LIFT,
				max_w = DEVICE_MAX,
				shrink = 1,
				fade = 16,
			},
			ui.div {
				shrink = 0,
				color = "$text.disabled",
				aria_hidden = true,
				Icon { name = "alt-arrow-right-bold", size = 10 },
			},
		}),
		ui.show(function(): boolean
			return unfolded() and (has_project() or not p.in_project())
		end, FadeText {
			name,
			roll = true,
			id = "composer-place-project",
			mb = LIFT,
			min_w = function(): number
				return math.min(PROJECT_MIN, math.ceil(natural.width or PROJECT_MIN))
			end :: any,
			max_w = PROJECT_MAX,
			shrink = 1,
			fade = 16,
		}),
		ui.show(function(): boolean
			return unfolded() and p.in_project()
		end, ui.row {
			shrink = 1000,
			-- (Its chevron, its glyph and the branch's sliver: under that it
			-- doesn't go, the name giving way instead.)
			min_w = function(): number
				return (if has_project() then 14 else 0) + branch_min()
			end :: any,
			overflow = "hidden",
			gap = "$space.1",
			items = "center",
			ui.show(has_project, ui.div {
				shrink = 0,
				color = "$text.disabled",
				aria_hidden = true,
				-- (A heavier stroke: at 10 pt the plain one read as a typed "›".)
				Icon { name = "alt-arrow-right-bold", size = 10 },
			}),
			FadeText {
				p.branch,
				roll = true,
				id = "composer-place-branch",
				mb = LIFT,
				min_w = branch_min :: any,
				max_w = BRANCH_MAX,
				shrink = 1,
				fade = 16,
			},
		}),
	}
end

-- The voice, while dictating with no levels to follow (the gallery, fixtures:
-- ds/voice_wave draws the microphone): bars across the track, each breathing on
-- its own, from the time alone (no script per frame). params[0]: the ink.
local VOICE = ui.shader([[
fn fill(input: FillInput) -> vec4<f32> {
 let ink = input.params[0];
 let n = floor(input.size.x / 4.0);
 let x = input.p.x / input.size.x * n;
 let i = floor(x);
 let fx = fract(x);
 if (fx < 0.2 || fx > 0.8) { return vec4(0.0); }
 let t = input.time;
 let a1 = 0.5 + 0.5 * sin(i * 1.71 + t * 7.3);
 let a2 = 0.5 + 0.5 * sin(i * 0.37 - t * 2.9 + 1.3);
 let env = 0.55 + 0.45 * sin(t * 1.9 + i * 0.11);
 let h = 0.12 + 0.88 * a1 * a2 * env;
 let y = abs(input.p.y / input.size.y - 0.5) * 2.0;
 let a = clamp((h - y) * input.size.y * 0.5 * input.scale, 0.0, 1.0);
 return vec4(ink.rgb, ink.a * a);
}
]], { fps = 30 })

-- Composers' lightbox groups, each its own.
local lightboxes = 0

return ui.component(function(props: Props): Node
	-- Opened out, the controls' inset: from the theme's corner, as this one
	-- is built.
	local INSET_OPEN = open_inset()
	-- On the tray, the chips' inset from the pill's edge above them, the
	-- tray's foot and its sides: the pill's own controls' (+ and Send sit
	-- this far in from its edge, its hairline and INSET_OPEN), so the row
	-- keeps the pill's spacing and its chips line up with those buttons.
	local TRAY_INSET = INSET_OPEN + HAIRLINES / 2
	-- On the tray, the step under the pill and above the tray's foot (one
	-- token: the footer's margins and the tray's height both read it), and
	-- how far the tray tucks up behind the pill, its top hidden there.
	local TRAY_GAP, TRAY_TUCK = "zeron.space.tight", 40
	local function tray_gap(): number
		local _ = ui.theme.current:get()
		return (ui.theme.get(TRAY_GAP) :: number?) or 6
	end
	local function tray_h(): number
		return CHIP_H + tray_gap() * 2 + TRAY_TUCK
	end
	local value = props.value
	local running = props.running
	local spoken = props.dictation
	if spoken then
		props.dictating, props.on_cancel_dictation = spoken.dictating, spoken.cancel
	end
	local field = if spoken then spoken.editor else ui.ref()
	local pill = ui.ref()

	-- What it is laid out from, as layout last measured it (0: not yet):
	-- written as each node is laid out (`on_resize`), so what reads them
	-- follows, and kept under `keep` so a composer made again starts from
	-- them rather than from nothing.
	local function kept<T>(key: string, initial: T): State<T>
		return if props.keep then ui.state(`composer:{props.keep}:{key}`, initial) else ui.use_state(initial)
	end
	local pill_w = kept("pill_w", 0)
	local line_w = kept("line_w", 0)
	local wrapped_h = kept("wrapped_h", 0)
	local head_h_seen = kept("head_h", 0)
	local trailing_seen = kept("trailing_w", 0)
	local footer_w = kept("footer_w", 0)
	local right_w = kept("right_w", 0)
	local model_w = kept("model_w", 0)
	local access_w = kept("access_w", 0)
	local where_w = kept("where_w", 0)
	local name_w = kept("name_w", 0)
	local branch_w = kept("branch_w", 0)
	local device_w = kept("device_w", 0)
	-- (The line has said what it takes: until then, BRANCH_MIN.)
	local place_known = kept("place_known", false)
	local folded = kept("fold", { level = 0, access_at = {} } :: Folded)
	local function measure(into: State<number>, axis: "w" | "h"): (size: Size, _el: Element) -> ()
		return function(size: Size, _el: Element)
			local v = if axis == "w" then size.w else size.h
			if math.abs(into:peek() - v) >= 0.5 then
				into:set(v)
			end
		end
	end

	local function place_measures(name: number, branch: number, device: number)
		if math.abs(name_w:peek() - name) >= 0.5 then
			name_w:set(name)
		end
		if math.abs(device_w:peek() - device) >= 0.5 then
			device_w:set(device)
		end
		if math.abs(branch_w:peek() - branch) >= 0.5 then
			branch_w:set(branch)
		end
		if not place_known:peek() then
			place_known:set(true)
		end
	end

	local function has_text(): boolean
		return string.find(value:get(), "%S") ~= nil
	end
	local function disabled(): boolean
		return read(props.disabled) == true
	end
	local function dictating(): boolean
		return props.dictating ~= nil and props.dictating:get()
	end
	local function question(): ask.Question?
		return if props.question then props.question:get() else nil
	end
	local function approval(): ask.Approval?
		return if props.approval then props.approval:get() else nil
	end
	local function asking(): boolean
		return question() ~= nil or approval() ~= nil
	end
	local function has_cargo(): boolean
		return props.cargo ~= nil and #props.cargo:get() > 0
	end
	local function picked(): number
		local n = 0
		if props.chosen then
			for _, on in props.chosen:get() do
				if on then
					n += 1
				end
			end
		end
		return n
	end
	-- A question can go on once something is picked or typed.
	local function can_answer(): boolean
		return picked() > 0 or has_text()
	end
	local function answer()
		local q = question()
		if not q or not can_answer() then
			return
		end
		local values: { string } = {}
		if props.chosen then
			for _, o in q.options do
				if props.chosen:peek()[o.value] then
					table.insert(values, o.value)
				end
			end
		end
		if props.on_answer then
			props.on_answer({ values = values, text = value:peek() })
		end
		value:set("")
	end
	local function send()
		if spoken and spoken.before_send() then
			return
		end
		local text = value:peek()
		if asking() then
			answer()
			return
		end
		if not string.find(text, "%S") and not has_cargo() then
			return
		end
		if props.on_send then
			props.on_send(text)
		end
		value:set("")
	end
	if spoken then
		spoken.send = send
	end
	local function stopping(): boolean
		return running ~= nil and running:get() and not has_text() and not has_cargo() and not asking() and not dictating()
	end

	-- Mentions at the caret.
	local m = mentions.use {
		field = field,
		value = value,
		sources = props.mentions or {},
		enabled = props.mention_enabled,
		empty = props.mention_empty,
		on_choose = props.on_mention,
		key = props.draft_key,
		sections = props.mention_sections,
		in_view = function(): boolean
			return (pill_w:get() or 0) > 0
		end,
	}
	-- Asked to: the keys, the caret at the end.
	if props.focus then
		local focus = props.focus
		ui.effect(function()
			if focus:get() <= 0 then
				return
			end
			local el = field:peek()
			if el then
				el:focus()
				local n = #value:peek()
				el:set_selection_bytes(n, n)
			end
		end)
	end

	-- Short of room: the caller's word, else the pill's width.
	local narrow = props.narrow or function(): boolean
		local w = pill_w:get()
		return (if w > 0 then w else 1000) < NARROW
	end

	-- One line or opened out. On one line the text has the pill less the
	-- leading control and the trailing ones; opened out, it folds back only
	-- once comfortably short again. Anything over the draft opens it out.
	-- The controls at the right, as measured; until they are (a composer
	-- just made, a reload's), their usual two (the microphone and Send),
	-- so the text is laid out where it rests from the first frame rather
	-- than easing in from the full width.
	local function trailing_w(): number
		local w = trailing_seen:get()
		return if w > 0 then w else CONTROL * 2 + 8
	end
	local function expanded(): boolean
		return read(props.expanded) == true
	end
	local was_open = expanded()
	local open = ui.computed(function(): boolean
		-- Dictating, the words land in the draft over the track.
		if expanded() or has_cargo() or asking() or dictating() then
			return true
		end
		local text = value:get()
		local width = pill_w:get()
		local tail = trailing_w()
		local need = line_w:get()
		if width <= 0 then
			return false
		end
		local room = width - HAIRLINES - (INSET_LINE + CONTROL + TEXT_SIDE) - (TEXT_SIDE + tail + INSET_LINE)
		local want
		if string.find(text, "\n") then
			want = true
		elseif was_open then
			want = need > room - HYSTERESIS
		else
			want = need > room
		end
		was_open = want
		return want
	end)

	-- What is over the draft: nothing, or its measured height.
	local head_h = ui.computed(function(): number
		if not has_cargo() and not asking() then
			return 0
		end
		return head_h_seen:get()
	end)
	-- The text box opened out: the wrapped text with its insets, between
	-- the floor and the cap.
	local box = ui.computed(function(): number
		local h = math.max(wrapped_h:get(), LINE)
		return math.clamp(h + TEXT_TOP + TEXT_BOTTOM, TEXT_MIN, TEXT_MAX)
	end)
	local HEAD_GAP = head_gap()
	local height = ui.computed(function(): number
		if open:get() then
			local head = head_h:get()
			-- With something over it, the draft's box starts under that
			-- with the strip's own gap rather than the pill's top.
			return head + (if head > 0 then box:get() - TEXT_TOP + HEAD_GAP else box:get()) + actions() + HAIRLINES
		end
		return ONE_LINE + HAIRLINES
	end)
	-- Where the text sits: beside the controls on one line, over them
	-- opened out, under whatever is over it.
	local text_left = ui.computed(function(): number
		return if open:get() then TEXT_OPEN_SIDE else INSET_LINE + CONTROL + TEXT_SIDE
	end)
	local text_right = ui.computed(function(): number
		return if open:get() then TEXT_OPEN_SIDE else TEXT_SIDE + trailing_w() + INSET_LINE
	end)
	local text_top = ui.computed(function(): number
		if not open:get() then
			return (ONE_LINE - LINE) / 2
		end
		local head = head_h:get()
		return if head > 0 then head + HEAD_GAP else TEXT_TOP
	end)
	local text_h = ui.computed(function(): number
		return if open:get() then box:get() - TEXT_TOP - TEXT_BOTTOM else LINE
	end)
	local EASE: TweenOptions = { duration = 220, ease = "out(3)" }
	-- Each of the pill's measures as it shows: eased (EASE) as it grows or
	-- shrinks in place; at once for a moment after `snap` moves (what
	-- settles its size, measured, lands over a frame or two), and with
	-- motion reduced.
	local SNAP_MS = 400
	local snap_seen = if props.snap then props.snap:peek() else nil
	local snap_until = 0
	local function at_once(): boolean
		if props.snap then
			local n = props.snap:get()
			if n ~= snap_seen then
				snap_seen = n
				snap_until = ui.now() + SNAP_MS
			end
		end
		return motion.reduced_now() or ui.now() < snap_until
	end
	local function eased(src: ReadOnlyState<number>): State<number>
		local value = ui.use_state(src:peek())
		local run: any = nil
		ui.effect(function()
			local v = src:get()
			local now = at_once()
			if run then
				run:cancel()
				run = nil
			end
			if now or math.abs(value:peek() - v) < 0.01 then
				value:set(v)
				return
			end
			run = anime.animate(value, { value = v, duration = EASE.duration, ease = EASE.ease })
		end)
		return value
	end
	-- (One on a line, another opened out.)
	local function by_open(line: number, opened: number): State<number>
		return eased(ui.computed(function(): number
			return if open:get() then opened else line
		end))
	end

	-- +, as a secondary button: a fill of its own, so it reads as a control
	-- beside the draft (the microphone stays ghost). Dictating, it is the
	-- way out.
	local attach = ui.show(dictating, Button {
		tone = "secondary",
		shape = "pill",
		size = "sm",
		icon = true,
		label = locale.t("ds.composer.cancel_dictation"),
		on_click = props.on_cancel_dictation,
		Icon { name = "close", size = 16 },
	}, Button {
		tone = "secondary",
		shape = "pill",
		size = "sm",
		icon = true,
		label = locale.t("ds.composer.attach"),
		disabled = disabled,
		on_click = props.on_attach,
		Icon { name = "plus", size = 18 },
	})
	-- Up from the footer: the composer sits at the window's foot, so the
	-- menu opens (and opens out, browsing) upward from a still bottom edge,
	-- never flipping across the chip as it grows. With room below
	-- (`menus_below`), down from it, its top edge still. (A placement is read
	-- as the picker is built: one following the composer is built again as
	-- it changes sides, its chip the same either way.)
	local function side(): string
		local below = props.menus_below
		local on = if type(below) == "function" then below() else below == true
		return if on then "bottom" else "top"
	end
	-- The footer folds a step at a time as it runs short of room, the
	-- fullest that fits: 0, all of it; 1, the model without its effort; 2,
	-- the access mode without "Plan" too; 3, the access mode's icon alone; 4,
	-- where it works as its icon too; 5, the model's icon alone. Until 4 where
	-- it works keeps the project's name and BRANCH_KEEP of the branch's
	-- (`where_min`, fading past it).
	--
	-- Its measures: the footer's width, the right group's, and each chip's.
	-- A chip's width at each of its folds is kept from when it was last at
	-- that fold; a fold not yet measured is tried, and measured.
	--
	-- Kept (`folded`), so one made again (a reload) starts at the fold it
	-- was at, its measures known, rather than unfolded until its chips are
	-- measured again.
	-- What where-it-works keeps before the access mode folds: the project's
	-- name (to its most) and some of the branch (none without a project).
	-- Folded to the access mode's icon (2), it may go further: the branch
	-- to its sliver, the name to its least.
	-- What a footer's fold makes of the chips on the left, and of the model.
	local function left_fold(lv: number): number
		return math.clamp(lv - 1, 0, 3)
	end
	local function model_fold(lv: number): number
		return if lv == 0 then 0 elseif lv >= 5 then 2 else 1
	end
	local function where_min(lv: number): number
		if not place_known:get() then
			return BRANCH_MIN
		end
		local n, b, d = name_w:get(), branch_w:get(), device_w:get()
		if lv >= 2 then
			return PLACE_CHROME + math.min(n, PROJECT_MIN) + math.min(b, BRANCH_FLOOR) + math.min(d, DEVICE_MIN + PLACE_CHEVRON)
		end
		return PLACE_CHROME + math.min(n, PROJECT_MAX) + math.min(b, BRANCH_KEEP) + math.min(d, DEVICE_KEEP + PLACE_CHEVRON)
	end
	local was = folded:peek()
	local access_at: { [number]: number } = {}
	for k, v in was.access_at do
		access_at[tonumber(k) :: number] = v
	end
	local where_icon: number? = was.where_icon
	local model_at: { [number]: number } = {}
	for k, v in was.model_at or {} do
		model_at[tonumber(k) :: number] = v
	end
	-- (What the model's chip says: another pick, its other folds measure again.)
	local model_says: string? = nil
	local level = was.level
	-- Bumped a frame after trying a fold not yet measured: its chip may lay
	-- out no wider or narrower (the access mode without "Plan" when plan
	-- mode is off), so no measure changes to look again by.
	local tried = ui.use_state(0)
	-- (Asked by the fold, started by the effect after it: a timer made in a
	-- computed lasts only until its next run.)
	local retry = false
	local fold = ui.computed(function(): number
		tried:get()
		local f, r, m = footer_w:get(), right_w:get(), model_w:get()
		-- (Not laid out yet: as it was.)
		if f <= 0 or r <= 0 or m <= 0 then
			return level
		end
		local a = if props.access then access_w:get() else 0
		local w = if props.branch then where_w:get() else 0
		if (props.access and a <= 0) or (props.branch and w <= 0) then
			return level
		end
		local says = `{props.model:get()}/{props.effort:get()}`
		if says ~= model_says then
			if model_says ~= nil then
				table.clear(model_at)
			end
			model_says = says
		end
		access_at[math.min(left_fold(level), 2)] = a
		-- (Its widest: with no room at all it's squeezed under the icon's width.)
		if left_fold(level) >= 3 then
			where_icon = math.max(where_icon or 0, w)
		end
		model_at[model_fold(level)] = m
		-- The footer's room: less its sides, what's on the right but the model,
		-- and the gaps.
		local chips = (if props.branch then 1 else 0) + (if props.access then 1 else 0)
		local room = f - FOOTER_SIDES - FOOTER_GAP * math.max(chips, 1) - (r - m)
		-- What fold `lv` takes; nil, not yet measured.
		local function needs(lv: number): number?
			local left = left_fold(lv)
			local access_w = access_at[math.min(left, 2)]
			local where_w = if not props.branch then 0 elseif left >= 3 then where_icon else where_min(left)
			local chip_w = model_at[model_fold(lv)]
			if access_w == nil or where_w == nil or chip_w == nil then
				return nil
			end
			return access_w + where_w + chip_w
		end
		-- Unfolding needs a little more than folding took, so a width at
		-- the edge cannot flicker.
		local next = 5
		for lv = 0, 4 do
			local need = needs(lv)
			if need == nil or need + (if lv < level then 8 else 0) <= room then
				next = lv
				break
			end
		end
		if next ~= level and needs(next) == nil then
			retry = true
		end
		level = next
		return level
	end)
	ui.effect(function()
		local now = fold:get()
		if retry then
			retry = false
			ui.after(16, function()
				tried:set(tried:peek() + 1)
			end)
		end
		local at: { [string]: number } = {}
		for k, v in access_at do
			at[tostring(k)] = v
		end
		local chip_at: { [string]: number } = {}
		for k, v in model_at do
			chip_at[tostring(k)] = v
		end
		folded:set { level = now, access_at = at, where_icon = where_icon, model_at = chip_at }
	end)

	local model = ui.presence(side, function(at: string): Node
		return ModelPicker {
			model = props.model,
			effort = props.effort,
			fast = props.fast,
			options = props.options,
			harnesses = props.harnesses :: any,
			models = props.models :: any,
			loading = props.models_loading,
			model_label = props.model_label,
			default_model = props.default_model,
			accounts = props.accounts,
			on_account = props.on_account,
			on_accounts_open = props.on_accounts_open,
			placement = { side = at, align = "end", offset = 6 },
			open_with = props.open_model,
			showing = props.model_open,
			fold = function(): number
				return model_fold(fold:get())
			end,
		}
	end)

	-- The access mode: its icon and name; with plan mode on, "Plan" after it
	-- and the icon in the accent (all that tells of it, folded).
	local access: Node? = nil
	if props.access then
		local plan = props.plan
		access = Dropdown {
			fold = function()
				return math.min(left_fold(fold:get()), 2)
			end,
			accent = if plan
				then function()
					return plan:get()
				end
				else nil,
			value = props.access,
			items = ACCESS,
			tone = "chip",
			label = locale.t("ds.composer.access.label") :: any,
			search = false,
			width = 300,
			detail = if plan
				then function()
					return if plan:get() then locale.s("ds.composer.plan.short") else ""
				end
				else nil,
			extra = if plan then plan_row(plan) else nil,
			placement = { side = "bottom", align = "start", offset = 6 },
		}
	end
	local mic = ui.show(dictating, Button {
		tone = "accent",
		shape = "pill",
		size = "sm",
		icon = true,
		label = locale.t("ds.composer.stop_dictating"),
		on_click = props.on_dictate,
		ui.div { w = 10, h = 10, rounded = ui.raw(3, "the stop glyph's corner"), bg = "$accent.text" },
	}, Button {
		tone = "ghost",
		shape = "pill",
		size = "sm",
		icon = true,
		label = locale.t("ds.composer.dictate"),
		disabled = disabled,
		on_click = props.on_dictate,
		Icon { name = "microphone", size = 18 },
	})
	local send_button = Button {
		shape = "pill",
		size = "sm",
		icon = true,
		label = function()
			return locale.s(if running and running:get() then "ds.composer.queue_message" else "ds.composer.send_message")
		end,
		disabled = function()
			return disabled() or not (has_text() or has_cargo() or dictating())
		end,
		on_press = if spoken then spoken.on_send_press else nil,
		on_click = send,
		Icon { name = "arrow-up", size = 14 },
	}
	local stop_button = Button {
		shape = "pill",
		size = "sm",
		icon = true,
		label = locale.t("ds.composer.stop"),
		on_click = props.on_stop,
		ui.div { w = 10, h = 10, rounded = ui.raw(3, "the stop glyph's corner"), bg = "$zeron.primary.text" },
	}
	-- A question's way through: Back (from the second page), then Submit
	-- (or Next, with pages to come), dimmed until something is picked or
	-- typed. An approval's: Deny, Always allow, Allow.
	local function ask_controls(): Node
		return ui.row {
			gap = "$space.2",
			items = "center",
			ui.show(function()
				local q = question()
				return q ~= nil and (q.page or 1) > 1
			end, Button { locale.t("ds.composer.back"), size = "sm", tone = "ghost", on_click = props.on_back }),
			ui.show(function()
				return question() ~= nil
			end, Button {
				size = "sm",
				shape = "pill",
				label = function()
					local q = question()
					return locale.s(if q and q.pages and (q.page or 1) < q.pages then "ds.composer.next" else "ds.composer.submit")
				end,
				disabled = function()
					return not can_answer()
				end,
				on_click = answer,
				ui.text {
					function()
						local q = question()
						return locale.s(if q and q.pages and (q.page or 1) < q.pages then "ds.composer.next" else "ds.composer.submit")
					end,
				},
			}),
			ui.show(function()
				return approval() ~= nil
			end, ui.row {
				-- What an agent may do is the person's to say: no plugin
				-- reaches these.
				hook = false,
				gap = "$space.2",
				items = "center",
				Button { locale.t("ds.composer.deny"), size = "sm", tone = "ghost", on_click = props.on_deny },
				ui.show(
					function(): boolean
						local always = props.can_always
						if type(always) == "function" then
							return always()
						end
						return always ~= false
					end,
					Button {
						locale.t("ds.composer.always_allow"),
						size = "sm",
						tone = "secondary",
						on_click = function()
							if props.on_approve then
								props.on_approve("always")
							end
						end,
					}
				),
				Button {
					locale.t("ds.composer.allow"),
					size = "sm",
					shape = "pill",
					on_click = function()
						if props.on_approve then
							props.on_approve("once")
						end
					end,
				},
			}),
		}
	end

	-- Dictation's clock: seconds since it began; warning in its last ten.
	local voice_seconds = ui.use_state(0)
	if props.dictating then
		local since = 0
		local stop: (() -> ())? = nil
		ui.effect(function()
			if props.dictating:get() then
				since = os.time()
				voice_seconds:set(0)
				if stop then
					stop()
				end
				stop = ui.every(500, function()
					voice_seconds:set(os.time() - since)
				end)
			elseif stop then
				stop()
				stop = nil
			end
		end)
	end
	local function clock(): string
		local s = voice_seconds:get()
		return `{s // 60}:{string.format("%02d", s % 60)}`
	end
	-- The track, unrolling from the microphone over the controls (and, on
	-- one line, the draft): the room it has is from the attach's right edge
	-- to the microphone's left; it is revealed from the right.
	-- The track is there only while dictating (its dot's pulse and its
	-- voice would otherwise ask for frames forever, unseen): it comes in
	-- from the microphone's side and leaves the same way.
	local function voice_track(): Node
		local track_w = ui.ref()
		return ui.div {
			absolute = true,
			-- (Insets take points: the control, the sides, and a breath.)
			left = by_open(INSET_LINE + CONTROL + 8, INSET_OPEN + CONTROL + 8),
			right = by_open(INSET_LINE + CONTROL * 2 + 8 + 8, INSET_OPEN + CONTROL * 2 + 8 + 8),
			bottom = by_open(INSET_LINE, INSET_OPEN),
			h = "$zeron.composer.voice.h",
			ref = track_w,
			overflow = "hidden",
			rounded = "$radius.full",
			pointer_events = "none",
			ui.presence(function(): string?
				return if dictating() then `on:{ui.theme.current:get()}` else nil
			end, function(_k: string): Node
				return ui.row {
					absolute = true,
					right = 0,
					top = 0,
					bottom = 0,
					w = function(): number
						return track_w.width or 0
					end,
					pl = "$space.3",
					pr = "$space.3",
					gap = "$space.2",
					items = "center",
					rounded = "$radius.full",
					bg = "$zeron.composer.voice.bg",
					ui.div {
						w = 8,
						h = 8,
						shrink = 0,
						rounded = "$radius.full",
						bg = "$zeron.composer.voice.ink",
						opacity = ui.loop { from = 0.35, to = 1, duration = 900, fps = 30 },
					},
					if spoken and spoken.levels and spoken.phase then VoiceWave { levels = spoken.levels, phase = spoken.phase } else ui.div {
						grow = 1,
						h = 16,
						min_w = 24,
						bg_shader = VOICE,
						-- (The engine steps its time: without it, it holds its first frame.)
						shader_clock = true,
						-- (Its ink as a colour, not a token: a clocked shader's parameters
						-- drop theme tokens, zgx 4809bcce. Its presence's key holds the
						-- theme, so the bar is made again as that changes.)
						shader_params = { ui.theme.get("zeron.composer.voice.ink"), { 0, 0, 0, 0 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 } },
					},
					ui.text {
						clock,
						shrink = 0,
						tabular = true,
						font = "$font.mono",
						text_size = "$font.size.xs",
						color = function()
							return if VOICE_LIMIT - voice_seconds:get() <= VOICE_WARN then "$warning" else "$text.secondary"
						end,
					},
				}
			end, {
				enter = { opacity = { 0, 1 }, x = { 40, 0 }, duration = 320, ease = "out(3)" },
				exit = { opacity = 0, x = 40, duration = 200, ease = "out(3)" },
			}),
		}
	end

	-- Files over the pill: its own sense of it, or the caller's.
	local dropping = props.dropping or ui.use_state(false)

	-- What sits over the draft: the agent's ask, or the cargo strip.
	local head: Node = ui.column {
		on_resize = measure(head_h_seen, "h"),
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		w_full = true,
		ui.presence(function(): string?
			local q = question()
			if q then
				-- A new question (or its next page) is built afresh.
				return `question:{q.text}:{q.page or 1}:{if q.multi then 1 else 0}`
			elseif approval() then
				return "approval"
			elseif has_cargo() then
				return "cargo"
			end
			return nil
		end, function(key: string): Node
			local kind = string.match(key, "^(%a+)")
			if kind == "question" then
				local q = question() :: ask.Question
				return ask.Question {
					question = q,
					chosen = props.chosen or ui.use_state({} :: { [string]: boolean }),
					on_pick = props.on_pick,
					on_close = props.on_close_question,
					closes_on_escape = function()
						return (q.page or 1) <= 1 and not can_answer()
					end,
				}
			elseif kind == "approval" then
				return ask.Approval { approval = approval() :: ask.Approval }
			end
			-- An image chip opens its picture in the lightbox (unless the
			-- caller opens chips itself), growing from the chip; ← → go
			-- through the chips' pictures.
			lightboxes += 1
			local group = `composer{lightboxes}`
			local items = props.cargo :: State<{ cargo.Item }>
			local function open_image(id: string)
				local images: { Lightbox.Item } = {}
				local at = nil
				for _, c in items:peek() do
					if (c.kind == "image" or c.kind == "appshot") and c.src and c.src ~= "" then
						table.insert(images, { src = c.src, name = c.name, path = (c :: any).path, key = c.id })
						if c.id == id then
							at = #images
						end
					end
				end
				if at then
					Lightbox.open(images, at, group)
				end
			end
			return ui.column {
				w_full = true,
				cargo.Strip {
					items = items,
					on_remove = props.on_remove_cargo,
					on_retry = props.on_retry_cargo,
					on_open = props.on_open_cargo or open_image,
					source = function(item: cargo.Item): { [string]: any }?
						if props.on_open_cargo then
							return nil
						end
						local radius = ui.theme.get("zeron.composer.chip.radius")
						return Lightbox.source(group, item.id, if type(radius) == "number" then radius else 0)
					end,
				},
				Lightbox.Host(),
			}
		end, {
			w_full = true,
			enter = { opacity = { 0, 1 }, duration = 200, ease = "outCubic" },
			exit = { opacity = 0, duration = 120 },
		}),
	}

	local function placeholder(): string
		if question() then
			return locale.s(if picked() > 0 then "ds.composer.placeholder.answer_picked" else "ds.composer.placeholder.answer")
		elseif approval() then
			return locale.s("ds.composer.placeholder.instead")
		end
		-- (A message follows the language: read here, as it's drawn.)
		local own = props.placeholder :: any
		if own == nil then
			return locale.s("ds.composer.placeholder.default")
		end
		return if type(own) == "string" then own else own:get()
	end

	-- A press on the composer's blank room (the pill's padding, its controls'
	-- rows between them, under the draft): the draft takes the keys, its
	-- caret at its end.
	local function to_draft()
		local el = field:peek()
		if el then
			el:focus()
			local n = #value:peek()
			el:set_selection_bytes(n, n)
		end
	end

	local surface = ui.div {
		ref = pill,
		on_resize = measure(pill_w, "w"),
		relative = true,
		w_full = true,
		h = eased(height),
		overflow = "hidden",
		-- (Its glass, edge and shadow are the frame's, round it and the deck
		-- over it: one surface. Its own corner only clips what's in it, and
		-- its edge, clear, keeps the room the frame's takes, so everything in
		-- it sits where it always has.)
		rounded = "$zeron.composer.radius",
		border = 1,
		border_color = "#00000000",
		-- (Neither `on_files` nor its overlay: files are what's around it to
		-- take, the drop going on up to it.)
		on_file_hover = if props.on_files or props.drop_overlay ~= false
			then function(inside: boolean)
				dropping:set(inside)
			end
			else nil,
		on_file_drop = if props.on_files or props.drop_overlay ~= false
			then function(files: { any })
				dropping:set(false)
				if props.on_files then
					props.on_files(files)
				end
			end
			else nil,
		-- A press anywhere but a control goes to the draft (its caret at
		-- its end). (Under the pill's controls and draft, which take their
		-- own; `focus_on_press = false`, so zgx doesn't move focus off it
		-- to what's under the press after.)
		ui.div {
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = 0,
			cursor = "text",
			focus_on_press = false,
			on_press = to_draft,
		},
		-- The measures: the draft on one line, and wrapped as it would be
		-- opened out (a mark after it, so a last empty line counts).
		ui.text {
			on_resize = measure(line_w, "w"),
			absolute = true,
			left = 0,
			top = 0,
			opacity = 0,
			pointer_events = "none",
			text_size = "$zeron.composer.text",
			line_height = "$zeron.composer.line",
			function()
				return value:get()
			end,
		},
		ui.text {
			on_resize = measure(wrapped_h, "h"),
			absolute = true,
			left = TEXT_OPEN_SIDE,
			right = TEXT_OPEN_SIDE,
			top = 0,
			opacity = 0,
			pointer_events = "none",
			wrap = true,
			text_size = "$zeron.composer.text",
			line_height = "$zeron.composer.line",
			function()
				return value:get() .. "\u{200B}"
			end,
		},
		head,
		-- The draft.
		ui.div {
			absolute = true,
			left = eased(text_left),
			right = eased(text_right),
			top = eased(text_top),
			h = eased(text_h),
			ui.textarea(EditorMenu.on({
				ref = field,
				value = value,
				label = locale.t("ds.composer.message"),
				placeholder = placeholder,
				placeholder_color = "$text.tertiary",
				read_only = disabled,
				w_full = true,
				h_full = true,
				px = 0,
				py = 0,
				border = 0,
				rounded = 0,
				bg = "#00000000",
				focus = { border = 0, outline = 0 },
				wrap = true,
				text_size = "$zeron.composer.text",
				line_height = "$zeron.composer.line",
				color = "$text.primary",
				-- The caret: the accent, two points wide, as Zeron's.
				caret_color = "$zeron.composer.caret",
				caret_width = ui.theme.get("zeron.composer.caret_width"),
				-- Markdown as it is typed: its markers greyed, bold, italic
				-- and struck text in their faces, code washed; and a wash
				-- behind each file, command or skill mentioned.
				highlight = mentions.highlight,
				active_descendant = m.descendant,
				on_caret = if spoken
					then function(line, column, anchor, focus)
						m.on_caret(line, column, anchor, focus)
						spoken.on_caret()
					end
					else m.on_caret,
				on_change = if spoken then spoken.on_change else nil,
				on_focus = function()
					m.on_focus(true)
					if spoken and spoken.on_focus then
						spoken.on_focus()
					end
				end,
				on_blur = function()
					m.on_focus(false)
					if spoken and spoken.on_blur then
						spoken.on_blur()
					end
				end,
				on_key_up = if spoken then spoken.on_key_up else nil,
				on_paste = function(e)
					-- (Files first: a copied file puts its name in `text` too.)
					if e.files and #e.files > 0 and props.on_files then
						props.on_files(e.files)
						return true
					end
					if e.text and props.on_paste_text then
						return props.on_paste_text(e.text)
					end
					if e.text == nil and props.on_paste_image then
						return props.on_paste_image()
					end
					return false
				end,
				-- Enter sends; Shift+Enter breaks the line, continuing a
				-- list (or undoing an empty item). The mentions' keys come
				-- first, then a question's (1 to 9 pick, Escape goes back
				-- or closes), then ⌘Enter sending the newest queued row.
				on_key = function(e)
					if dictating() and e.key == "Escape" then
						if props.on_cancel_dictation then
							props.on_cancel_dictation()
						end
						return true
					end
					if m.keys(e) then
						return true
					end
					local q = question()
					if q then
						local d = string.match(e.key, "^(%d)$")
						if d and not has_text() and not e.cmd and not e.ctrl and not e.alt then
							local o = q.options[tonumber(d) :: number]
							if o and props.on_pick then
								props.on_pick(o.value)
							end
							return true
						end
						if e.key == "Escape" then
							if (q.page or 1) > 1 and props.on_back then
								props.on_back()
							elseif not can_answer() and props.on_close_question then
								props.on_close_question()
							end
							return true
						end
					end
					-- A bare Escape: the caller's first (stopping the agent, where
					-- settings say Escape does); else the keyboard goes back to
					-- the window, whose bare letters (the panel's B, F, T…, I
					-- to come back) work again, as vim's normal mode.
					if e.key == "Escape" and not e.shift and not e.cmd and not e.alt then
						if props.on_escape and props.on_escape() then
							return true
						end
						local el = field:peek()
						if el then
							el:blur()
							return true
						end
						return false
					end
					-- (⌘↩ under "mod_enter"; plain ↩ there breaks the line.)
					local enter_breaks = props.send_with ~= nil and props.send_with() == "mod_enter" and not e.cmd
					if e.key == "Enter" and not e.shift and not enter_breaks then
						if e.cmd and not has_text() and not has_cargo() and props.queue and props.on_queue_send then
							local list = props.queue:peek()
							local newest = list[#list]
							if newest then
								props.on_queue_send(newest.id)
								return true
							end
						end
						if stopping() then
							return true
						end
						send()
						return true
					end
					if e.key == "Enter" and (e.shift or enter_breaks) then
						local el = field:peek()
						if el then
							local a, b = el:selection_bytes()
							local from, to, text = nil, nil, nil
							if a and b then
								from, to, text = markdown.newline(value:peek(), a, b)
							end
							if from and to and text then
								el:set_selection_bytes(from, to)
								el:replace_selection(text)
								return true
							end
						end
					end
					return false
				end,
			}, { takes_paste = props.on_paste_image ~= nil or props.on_files ~= nil })),
		},
		-- The controls, on the pill's foot.
		ui.div {
			absolute = true,
			left = by_open(INSET_LINE, INSET_OPEN),
			bottom = by_open(INSET_LINE, INSET_OPEN),
			ui.show(function()
				return not asking()
			end, attach),
		},
		voice_track(),
		ui.row {
			on_resize = measure(trailing_seen, "w"),
			absolute = true,
			right = by_open(INSET_LINE, INSET_OPEN),
			-- (Its row is 32: the control centred in it.)
			bottom = by_open(INSET_LINE - 2, INSET_OPEN - 2),
			h = 32,
			items = "center",
			gap = "$space.2",
			ui.show(asking, ask_controls()),
			ui.show(function()
				return not asking()
			end, ui.row {
				items = "center",
				gap = "$space.2",
				if spoken then spoken.mic() else mic,
				ui.show(stopping, stop_button),
				ui.show(function()
					return not stopping()
				end, send_button),
			}),
		},
		-- Files over it: the pill says what a drop does, over everything, as
		-- Zeron's drop overlay does (shell.rs `attachment_drop_overlay`): the
		-- scrim, the words in the text's ink, there while files are over it.
		-- (Not when the caller covers more than the pill with its own.)
		if props.drop_overlay ~= false
			then DropOverlay {
				shown = function(): boolean
					return dropping:get()
				end,
			}
			else nil,
	}

	local footer: Node? = nil
	-- (Whether the footer sits on its tray: set as it's made, read by the
	-- tray behind the pill.)
	local footer_on_tray: (() -> boolean)? = nil
	do
		local left: { Child } = {}
		local over_art = props.over_art
		-- Over art, the footer sits on a tray (the pill's glass under it).
		local function on_tray(): boolean
			return over_art ~= nil and over_art()
		end
		footer_on_tray = on_tray
		-- The access mode first, then where it works.
		if access then
			-- (Out of plugins' reach: it says what the agent may do unasked.)
			table.insert(left, ui.div { on_resize = measure(access_w, "w"), shrink = 0, hook = false, access })
		end
		if props.branch then
			local checkout = props.checkout
			-- Built for each chip it heads (a node lives in one place).
			local function head_node(): Node?
				return if checkout or props.device or props.project
					then work_in(props.project, props.projects :: any, props.on_new_project, checkout, props.device, props.devices :: any)
					else nil
			end
			-- In a project, its refs: the branch chip, its search and its
			-- branches. With none, no refs to pick: the chip says so (a folder,
			-- "No project"), and opens on where to work alone. One dropdown
			-- either way, so its panel stays open as the project changes.
			local project = props.project
			local function in_project(): boolean
				return project == nil or project:get() ~= NO_PROJECT
			end
			-- The project chosen, as the projects list it (nil: none, or
			-- none named: a caller with no projects).
			local function chosen_project(): Project?
				if not project or not props.projects or not in_project() then
					return nil
				end
				local v = project:get()
				for _, pr in list_of(props.projects) :: { Project } do
					if pr.value == v then
						return pr
					end
				end
				return nil
			end
			-- The branch's name as its row has it, else as it is (its list
			-- still coming), else "No ref".
			local branch_state = props.branch :: State<string?>
			local function branch_name(): string
				local v = branch_state:get()
				for _, item in list_of(props.branches) :: { Dropdown.Item } do
					if item.value == v then
						return item.label
					end
				end
				return v or locale.s("ds.composer.branch.no_ref")
			end
			-- Where it runs off the branch: Cloud's machine, a worktree, or
			-- (nil) the project's own checkout.
			local function runs_in(): Icon.Name?
				if on_cloud(props.device, props.devices) then
					return "cloud"
				end
				local chosen = chosen_project()
				if chosen and chosen.plain then
					return nil
				end
				return if checkout and checkout:get() == "local" then nil else "worktree"
			end
			local function folded(): boolean
				return left_fold(fold:get()) >= 3
			end
			-- The device chosen, by name, leading the chip.
			local function chosen_device(): string?
				if not props.device or not props.devices then
					return nil
				end
				local v = props.device:get()
				for _, d in list_of(props.devices) :: { Device } do
					if d.value == v then
						return d.label
					end
				end
				return nil
			end
			local picker = Dropdown {
				value = props.branch,
				label = locale.t("ds.composer.branch.label") :: any,
				-- (A string: Dropdown names its field by it only so.)
				search = locale.s("ds.composer.branch.search"),
				items = (props.branches or {}) :: any,
				empty = props.branches_empty,
				list_when = in_project,
				width = 280,
				head = head_node(),
				-- The chip: [tile] project › [glyph] branch, washed under the
				-- pointer and while its panel is open, as a chip is.
				trigger = function(menu: Dropdown.Menu): Node
					return ui.button {
						label = locale.t("ds.composer.branch.label"),
						h = CHIP_H,
						px = "$zeron.space.tight",
						gap = "$zeron.space.tight",
						items = "center",
						min_w = 0,
						rounded = CHIP_RADIUS,
						text_size = "$font.size.sm",
						weight = "$font.weight.medium",
						bg = function(): string
							return if menu.open:get() then "$hover" else "#00000000"
						end,
						color = "$text.tertiary",
						hover = { bg = "$hover", color = "$text.secondary" },
						active = { bg = "$pressed", color = "$text.secondary" },
						on_click = menu.toggle,
						table.unpack(place_line {
							device = chosen_device,
							project = chosen_project,
							in_project = in_project,
							branch = branch_name,
							glyph = runs_in,
							folded = folded,
							on_measures = place_measures,
						}),
					}
				end,
			}
			-- An existing chat's: where it works, said, opening nothing.
			local fixed = props.branch_fixed
			local static: Node? = if fixed
				then ui.row {
					h = CHIP_H,
					px = "$zeron.space.tight",
					gap = "$zeron.space.tight",
					items = "center",
					min_w = 0,
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
					color = "$text.tertiary",
					label = function(): string
						local f = fixed()
						return locale.s("ds.composer.branch.fixed", { name = if f and f.name then f.name else "" })
					end,
					table.unpack(place_line {
						device = function(): string?
							local f = fixed()
							return if f then f.device else nil
						end,
						project = function(): Project?
							local f = fixed()
							if f and f.project then
								return { value = f.project, label = f.project, hue = f.hue, src = f.src, emoji = f.emoji }
							end
							return nil
						end,
						in_project = function(): boolean
							return true
						end,
						branch = function(): string
							local f = fixed()
							return if f and f.name then f.name else locale.s("ds.composer.branch.no_ref")
						end,
						glyph = function(): Icon.Name?
							local f = fixed()
							return if f and f.worktree then "worktree" else nil
						end,
						folded = folded,
						on_measures = place_measures,
					}),
				}
				else nil
			table.insert(
				left,
				ui.row {
					on_resize = measure(where_w, "w"),
					min_w = 0,
					if static
						then ui.show(function(): boolean
							return (fixed :: any)() ~= nil
						end, static, picker)
						else picker,
				}
			)
		end
		footer = ui.row {
			id = props.toolbar_id,
			on_resize = measure(footer_w, "w"),
			relative = true,
			h = 24,
			-- On the tray: a tight step under the pill and above the tray's
			-- foot (the pill's full control inset read too airy there), and
			-- the pill's controls' inset from its sides, lined up with + and
			-- Send.
			mt = function(): any
				return if on_tray() then `${TRAY_GAP}` else "$space.2"
			end :: any,
			mb = function(): any
				return if on_tray() then `${TRAY_GAP}` else 0
			end :: any,
			px = function(): any
				return if on_tray() then ui.raw(TRAY_INSET, "the pill's controls' inset") else "$zeron.space.snug"
			end :: any,
			items = "center",
			gap = "$space.1",
			-- Its blank room, between and beside its controls: the draft's,
			-- as the pill's is.
			ui.div {
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				focus_on_press = false,
				on_press = to_draft,
			},
			-- Clipped: past every fold, it gives way under the right group
			-- rather than running beneath it.
			ui.row {
				hook = "chat.composer.toolbar.start",
				-- (Its blank room passes the press on, to the draft's;
				-- its controls take their own.)
				pointer_events = "none",
				gap = "$space.1",
				items = "center",
				min_w = 0,
				grow = 1,
				overflow = "hidden",
				table.unpack(left),
			},
			ui.row {
				hook = "chat.composer.toolbar.end",
				pointer_events = "none",
				on_resize = measure(right_w, "w"),
				items = "center",
				shrink = 0,
				ui.row { on_resize = measure(model_w, "w"), items = "center", shrink = 0, model },
				if props.usage
					then ui.show(function(): boolean
						local u = (props.usage :: () -> Usage?)()
						return u ~= nil and u.window ~= nil and u.window > 0
					end, ring(0, props.usage, side))
					elseif props.context then ring(props.context, nil, side)
					else nil,
			},
		}
	end

	-- The trays, behind the pill.
	local trays: Node? = nil
	-- (Folded, what waits is a pill floating over the conversation.)
	local tray_pill: Node? = nil
	-- The notice (not connected, a send that failed, a draft restored) is
	-- an alert of the deck's, first, in its tone; it goes only as what it
	-- says is over, unless it says it may be let go.
	local NOTICE = "\0notice"
	local alerts: any = props.alerts
	if props.notice then
		local notice = props.notice
		alerts = ui.computed(function(): { dock.Alert }
			local out: { dock.Alert } = {}
			local n = notice:get()
			if n then
				local tone: dock.AlertTone = if n.tone == "danger" then "danger" elseif n.tone == "warning" then "warning" else "info"
				table.insert(out, {
					id = NOTICE,
					tone = tone,
					title = n.text,
					action = n.action,
					spinner = n.spinner,
					dismissable = n.dismissable == true,
					quiet = n.quiet == true,
				})
			end
			if props.alerts then
				for _, x in props.alerts:get() do
					table.insert(out, x)
				end
			end
			return out
		end)
	end
	if props.queue or props.todos or props.tasks or alerts then
		trays, tray_pill = dock.parts {
			queue = props.queue,
			can_steer = props.can_steer,
			on_queue_send = props.on_queue_send,
			on_queue_edit = props.on_queue_edit,
			on_queue_remove = props.on_queue_remove,
			on_queue_reorder = props.on_queue_reorder,
			on_queue_save = props.on_queue_save,
			on_queue_cancel = props.on_queue_cancel,
			todos = props.todos,
			live = function()
				return running ~= nil and running:get()
			end,
			on_dismiss_todos = props.on_dismiss_todos,
			tasks = props.tasks,
			on_stop_task = props.on_stop_task,
			on_stop_all = props.on_stop_all,
			on_open_task = props.on_open_task,
			open = props.tray,
			narrow = narrow,
			aside = props.aside,
			alerts = alerts,
			on_alert_action = function(id: string)
				if id == NOTICE then
					if props.on_notice_action then
						props.on_notice_action()
					end
				elseif props.on_alert_action then
					props.on_alert_action(id)
				end
			end,
			on_dismiss_alert = function(id: string)
				if id == NOTICE then
					if props.on_dismiss_notice then
						props.on_dismiss_notice()
					end
				elseif props.on_dismiss_alert then
					props.on_dismiss_alert(id)
				end
			end,
			on_pill_width = props.on_pill_width,
		}
	end

	-- The pill (the mentions' popover anchored on it: its panel opens at
	-- the caret inside), its trays behind, the footer under.
	local body = ui.column {
		w_full = true,
		relative = true,
		opacity = function(): number
			return if disabled() then (ui.theme.get("zeron.disabled") :: number?) or 0.4 else 1
		end,
		-- The tray (over a wallpaper): the pill's glass continued under the
		-- footer, tucked behind the pill (its top hidden under it, its sides
		-- the pill's), so the chips sit on the composer's surface rather than
		-- on the art. They sit as far in from its sides as the pill's own
		-- controls sit in it (TRAY_INSET), a tight step (TRAY_GAP) below the
		-- pill and above its foot; its height is theirs (tray_h), its corner
		-- the trays'.
		ui.presence(function(): string?
			return if footer_on_tray and footer_on_tray() then "tray" else nil
		end, function(): Node
			return ui.div {
				class = "composer-tray",
				absolute = true,
				left = 0,
				right = 0,
				bottom = 0,
				h = tray_h,
				pointer_events = "none",
				-- Its corner at its foot only, its top square, and no edge of
				-- its own anywhere: it is the pill's glass continued (the pill
				-- keeps its edge). A rounded top, or a hairline, showed a second
				-- curve against the pill's own.
				rounded = "$zeron.composer.tray.radius",
				rounded_tl = 0,
				rounded_tr = 0,
				bg = function(): string
					return if material.see_through() then "$zeron.composer.tint" else "$zeron.composer.solid"
				end,
				backdrop_blur = function(): number
					return if material.see_through() then 16 else 0
				end,
			}
		end, {
			absolute = true,
			left = 0,
			right = 0,
			bottom = 0,
			h = tray_h,
			pointer_events = "none",
			enter = { opacity = { 0, 1 }, duration = 220, ease = "out(3)" },
			exit = { opacity = 0, duration = 160 },
		}),
		-- The frame: the pill's glass, edge and shadow, round the deck (what
		-- waits on the agent, the composer's own top) and the pill, so the two
		-- read as one surface growing upward.
		ui.column {
			w_full = true,
			shrink = 0,
			relative = true,
			-- (The glass is a layer behind what's in it, not this column's own:
			-- a backdrop blur on the column took in its children too, blurring
			-- the draft and its controls.)
			ui.div {
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				pointer_events = "none",
				rounded = "$zeron.composer.radius",
				border = 1,
				border_color = "$zeron.composer.edge",
				bg = function()
					return if material.see_through() then "$zeron.composer.tint" else "$zeron.composer.solid"
				end,
				backdrop_blur = function()
					return if material.see_through() then 16 else 0
				end,
				shadow = function()
					return if material.see_through() then "$zeron.composer.rest" else "$zeron.composer.shadow"
				end,
			},
			trays,
			ui.popover(m.popover(surface)),
		},
		footer,
		-- Folded, what waits on the agent: a pill floating just over the
		-- composer's middle, out of the layout (the conversation shows under
		-- it, as under "Scroll to end", which it moves aside for).
		if tray_pill
			then ui.row {
				absolute = true,
				left = 0,
				right = 0,
				justify = "center",
				top = ui.raw(-(dock.PILL_H + dock.PILL_GAP), "the pill's height and its gap over the composer"),
				h = dock.PILL_H,
				pointer_events = "none",
				tray_pill,
			}
			else nil,
	}

	return ui.column {
		w_full = true,
		gap = 0,
		body,
	}
end)
