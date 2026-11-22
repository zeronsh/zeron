-- A decision, whole: the overlay Needs you's rows and the board's cards
-- open (ds/orch_decide's `shown`). Over a scrim, about seven tenths of the
-- window; Escape or a click outside closes it.
--
-- Its head: what kind of ask, the question (markdown), whose (the thread,
-- opened at the message that asked), its orchestrator, how long ago, and
-- its links (a pull request, a branch). The context under it folds. Then
-- its options as cards in a grid as wide as there's room for: each one's
-- pictures (a click looks at one whole: ds/lightbox), its name and what it
-- means, its scores, and the recommended one edged in the accent saying
-- why. A click or its number (1–9) chooses one. At its foot: a reply
-- (Enter decides, Shift+Enter a new line), Decide (the option chosen and
-- the words), Defer (an hour, four, tomorrow, next week) and the thread.
-- Deciding goes through ds/orch_decide's pick, so the row and the card
-- say "Sending · Undo" for its moment.
--
-- It stays open until the person closes it (or decides): a frame of the
-- engine's state without its ask (an engine coming up again, a republish)
-- shows the ask as it last was rather than closing it, and a close the
-- modal makes on its own while it is still opening is let go.
--
--   Decision {
--     ask = function() return the shown ask or nil end,
--     name_of = function(id) end, hue_of = function(id) end,
--     on_answer = function(ask, { option?, text? }) end,
--     on_defer = function(ask, until_ms) end,
--     on_open = function(chat, message?) end,
--     on_link = function(link) end,
--   }
--
-- The same overlay holds a Q&A entry whole (`Decision.Answer`): its
-- question, the answer as the thread wrote it (markdown), who answered and
-- when, ways into the thread and where it was asked, read or unread and
-- dismiss, and at its foot a follow-up, asked of the same thread.
--
--   Decision.Answer {
--     entry = function() return the shown entry (ds/orch_qa's) or nil end,
--     on_close = function() end,
--     name_of = function(id) end, hue_of = function(id) end,
--     on_open = function(chat) end,              -- its thread
--     on_source = function(entry) end,           -- where it was asked
--     on_read = function(entry, read) end,
--     on_dismiss = function(entry) end,
--     on_follow_up = function(entry, text) end,  -- a promise, or nil
--   }

local Icon = require("@self/icon")
local IconButton = require("@self/icon_button").IconButton
local Button = require("@self/button")
local TextArea = require("@self/text_area")
local Markdown = require("@self/markdown")
local Lightbox = require("@self/lightbox")
local ContextMenu = require("@self/context_menu")
local SelectionMenu = require("@self/selection_menu")
local Kbd = require("@self/kbd")
local badge = require("@self/badge")
local status = require("@self/status")
local locale = require("@self/locale")
local control = require("@self/control")
local motion = require("@self/motion")
local glass = require("@self/glass")
local material = require("@self/material")
local hue = require("@self/orch_hue")
local decide = require("@self/orch_decide")

export type Props = {
	ask: () -> any,
	name_of: (id: string?) -> string?,
	hue_of: ((id: string?) -> number?)?,
	on_answer: (ask: any, choice: decide.Choice) -> any,
	on_defer: (ask: any, until_ms: number) -> (),
	on_open: (chat: string, message: string?) -> (),
	on_link: ((link: any) -> ())?,
}

local KIND_STATUS: { [string]: status.Kind } = {
	question = "input",
	decision = "input",
	approval = "approval",
	blocker = "failed",
}

-- An option card's least width, and the grid's gap.
local CARD_MIN = 248
local GAP = 12
-- Its inset, side to side.
local PAD_X = 24

local IN = { duration = 200, ease = "out(3)" }
local OUT = { duration = 130, ease = "in(2)" }

local function window_size(): { width: number, height: number }
	local window = (ui :: any).window
	local state = if type(window) == "table" or type(window) == "userdata" then window.size else nil
	local ok, v = pcall(function(): any
		return if state then state:get() else nil
	end)
	local w = if ok and type(v) == "table" and type(v.width) == "number" then v.width else 0
	local h = if ok and type(v) == "table" and type(v.height) == "number" then v.height else 0
	return { width = if w > 1 then w else 1280, height = if h > 1 then h else 800 }
end

local function basename(path: string): string
	return string.match(path, "([^/\\]+)$") or path
end

local function pictures(list: { string }): { Lightbox.Item }
	local out: { Lightbox.Item } = {}
	for i, src in list do
		table.insert(out, { src = src, name = basename(src), path = if string.match(src, "^https?://") then nil else src, key = tostring(i) })
	end
	return out
end

-- A picture that opens the lightbox: cropped to its box.
local function thumb(group: string, items: { Lightbox.Item }, i: number, w: any, h: number, radius: number): Node
	return Lightbox.Thumb {
		group = group,
		index = i,
		items = items,
		radius = radius,
		w = w,
		w_full = w == nil,
		h = h,
		rounded = ui.raw(radius, "the picture's own corner, inside the card's"),
		overflow = "hidden",
		class = "orch-decision-picture",
		ui.image { src = items[i].src, fit = "cover", w_full = true, h_full = true, rounded = ui.raw(radius, "as its box") },
	}
end

-- A score: its name quiet, its value in the text's ink.
local function score(s: { name: string, value: string }): Node
	return ui.row {
		class = "orch-decision-score",
		h = 20,
		px = "$space.2",
		gap = "$zeron.space.tight",
		items = "center",
		rounded = "$radius.full",
		bg = "$zeron.secondary.bg",
		shrink = 0,
		ui.text { s.name, literal = true, text_size = "$font.size.xs", color = "$text.tertiary" },
		ui.text { s.value, literal = true, tabular = true, text_size = "$font.size.xs", weight = "$font.weight.medium", color = "$text.primary" },
	}
end

-- A link of the ask's: a pull request as its badge, else its glyph and name.
local LINK_ICON: { [string]: Icon.Name } = { branch = "git-branch", chat = "chat-round-line", url = "arrow-up-right", pr = "git-branch" }
local function link(props: Props, l: any): Node
	local n = if l.kind == "pr" then tonumber(string.match(l.label or "", "%d+")) else nil
	return ui.button {
		class = "orch-decision-link",
		label = l.label or l.target or "",
		h = 22,
		px = if n then 0 else "$space.2",
		gap = "$zeron.space.tight",
		items = "center",
		shrink = 0,
		rounded = "$radius.md",
		bg = "#00000000",
		hover = { bg = "$hover" },
		active = { bg = "$pressed" },
		focus = control.ring,
		on_click = function()
			if props.on_link then
				props.on_link(l)
			end
		end,
		if n
			then badge.PullRequest { state = "open", number = n, size = "sm" }
			else ui.div { color = "$text.tertiary", Icon { name = LINK_ICON[l.kind] or "arrow-up-right", size = 12 } },
		if n then nil else ui.text { l.label or l.target or "", literal = true, truncate = true, max_w = 220, text_size = "$font.size.xs", color = "$text.secondary" },
	}
end

local Body = ui.component(function(args: { props: Props, phase: State<number> }): Node
	local props, phase = args.props, args.phase
	-- (The ask as it was opened, kept: answered under it, it doesn't vanish.)
	local first = props.ask()
	local function ask(): any
		return props.ask() or first
	end
	local selected = ui.use_state(nil :: number?)
	local reply = ui.use_state("")
	local context_open = ui.use_state(true)
	local defer_open = ui.use_state(false)
	local grid_w = ui.use_state(0)
	-- What the body holds, its height.
	local body_h = ui.use_state(0)
	-- A key the reply heard first is its own (a digit typed).
	local typing = false

	local opts = ui.computed(function(): { decide.Option }
		return decide.options(ask())
	end)
	local function ready(): boolean
		return selected:get() ~= nil or string.match(reply:get(), "%S") ~= nil
	end
	local function close()
		decide.hide()
	end
	local function go()
		local a = ask()
		if not a or not ready() then
			return
		end
		decide.pick(a, { option = selected:peek(), text = reply:peek() }, props.on_answer)
		close()
	end
	local function open_thread()
		local a = ask()
		if a and a.from_chat then
			close()
			props.on_open(a.from_chat, a.turn_id)
		end
	end
	local function choose(i: number)
		if opts:peek()[i] then
			selected:set(if selected:peek() == i then nil else i)
		end
	end

	local function cols(): number
		local w = grid_w:get()
		local n = #opts:get()
		if w <= 0 or n == 0 then
			return 1
		end
		return math.max(1, math.min(n, math.floor((w + GAP) / (CARD_MIN + GAP)), 3))
	end
	local function card_w(): number
		local c = cols()
		return math.floor((grid_w:get() - GAP * (c - 1)) / c)
	end

	local kind = function(): string
		local a = ask()
		return (a and a.kind) or "question"
	end
	local state = function(): status.Kind
		return KIND_STATUS[kind()] or "input"
	end

	-- One option, a card.
	local function option(o: decide.Option): Node
		local group = `decision:{(ask() or {}).id or ""}:{o.index}`
		local items = pictures(o.images)
		local function on(): boolean
			return selected:get() == o.index
		end
		return ui.button {
			class = if o.recommended then "orch-decision-option orch-decision-recommended" else "orch-decision-option",
			label = if o.recommended then locale.s("ds.orch.decide.recommended_named", { label = o.label }) else o.label,
			selected = on,
			w = card_w,
			p = 0,
			gap = 0,
			direction = "column",
			items = "stretch",
			justify = "start",
			self = "start",
			rounded = "$radius.lg",
			overflow = "hidden",
			bg = function(): string
				return if on() then "$selection.item" else material.pick("$zeron.settings.note", "$zeron.settings.card")()
			end,
			hover = { bg = "$hover" },
			active = { bg = "$pressed" },
			border = function(): number
				return if on() then 2 elseif o.recommended then 1 else 1
			end,
			border_color = function(): string
				return if on() or o.recommended then "$accent" else "$border.subtle"
			end,
			focus = control.ring,
			cursor = "pointer",
			-- Its words select on their own: a button that is its own
			-- selectable region clicks, and a drag from it selects its words
			-- and doesn't click (zgx). So only a click chooses.
			selectable = true,
			on_click = function()
				choose(o.index)
			end,
			-- Its pictures: the first across the card, any more under it.
			if #items > 0
				then ui.column {
					w_full = true,
					gap = "$zeron.space.tight",
					p = "$zeron.space.tight",
					pb = 0,
					thumb(group, items, 1, nil, 150, 8),
					if #items > 1
						then ui.row {
							gap = "$zeron.space.tight",
							table.unpack((function(): { Node }
								local out: { Node } = {}
								for i = 2, math.min(#items, 5) do
									table.insert(out, thumb(group, items, i, 64, 40, 6))
								end
								return out
							end)()),
						}
						else nil,
				}
				else nil,
			ui.column {
				class = "orch-decision-option-words",
				w_full = true,
				p = "$space.3",
				gap = "$space.2",
				ui.row {
					w_full = true,
					gap = "$space.2",
					items = "center",
					if o.index <= 9 then Kbd { text = tostring(o.index), size = "sm", tone = "quiet" } else nil,
					ui.text {
						o.label,
						literal = true,
						wrap = true,
						grow = 1,
						min_w = 0,
						text_size = "$font.size.md",
						line_height = "$font.line.md",
						weight = "$font.weight.semibold",
						color = "$text.primary",
					},
					ui.show(on, ui.div { class = "orch-decision-chosen", color = "$accent", shrink = 0, Icon { name = "check-circle", size = 16 } }),
				},
				if o.detail
					then ui.text { o.detail, literal = true, wrap = true, w_full = true, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.secondary" }
					else nil,
				if #o.scores > 0
					then ui.row {
						w_full = true,
						flex_wrap = true,
						gap = "$zeron.space.tight",
						table.unpack((function(): { Node }
							local out: { Node } = {}
							for _, s in o.scores do
								table.insert(out, score(s))
							end
							return out
						end)()),
					}
					else nil,
				if o.recommended
					then ui.row {
						class = "orch-decision-why",
						w_full = true,
						gap = "$zeron.space.tight",
						items = "start",
						ui.div { color = "$accent", shrink = 0, pt = 2, Icon { name = "star-bold", size = 11 } },
						ui.rich {
							wrap = true,
							grow = 1,
							min_w = 0,
							text_size = "$font.size.xs",
							line_height = "$font.line.xs",
							ui.span { locale.t("ds.orch.decide.recommended"), weight = 500, color = "$accent" },
							(function(): any
								local w = decide.why(ask())
								return if w then ui.span { ` · {w}`, literal = true, color = "$text.secondary" } else nil
							end)(),
						},
					}
					else nil,
			},
		}
	end

	local defer_items = function(): { any }
		local out = {}
		for _, d in decide.defer_times() do
			table.insert(out, {
				label = locale.s(`ds.orch.decide.defer_{d.key}`),
				on_select = function()
					local a = ask()
					if a then
						props.on_defer(a, d.at)
						close()
					end
				end,
			})
		end
		return out
	end

	local own_images = ui.computed(function(): { Lightbox.Item }
		return pictures(decide.images(ask()))
	end)

	return ui.column(glass.style({
		class = "orch-decision",
		w_full = true,
		p = 0,
		gap = 0,
		overflow = "hidden",
		rounded = "$zeron.dialog.radius",
		shadow = "$zeron.dialog.shadow",
		opacity = function()
			return phase:get()
		end,
		scale = function()
			return 0.98 + 0.02 * phase:get()
		end,
		y = function()
			return 6 * (1 - phase:get())
		end,
		on_key = function(e: any): boolean
			-- (Escape is the person's: closed at once, whenever.)
			if e.key == "Escape" and not (e.ctrl or e.alt or e.meta or e.cmd or e.shift) then
				close()
				return true
			end
			if typing then
				typing = false
				return false
			end
			if not (e.ctrl or e.alt or e.meta or e.cmd or e.shift) and string.match(e.key or "", "^[1-9]$") then
				choose(tonumber(e.key) :: number)
				return true
			end
			return false
		end,
		-- The head.
		ui.column {
			w_full = true,
			shrink = 0,
			px = PAD_X,
			pt = "$space.5",
			pb = "$space.4",
			gap = "$space.3",
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				status.StatusIcon { kind = state, size = 14 },
				ui.text {
					function(): string
						return locale.s(`ds.orch.ask.{kind()}`)
					end,
					text_size = "$font.size.xs",
					weight = "$font.weight.medium",
					color = function(): string
						return `$zeron.status.{state()}`
					end,
				},
				ui.show(function(): boolean
					local a = ask()
					return a ~= nil and (a.urgent == true or a.escalated ~= nil)
				end, badge.Badge { locale.s("ds.orch.ask.urgent"), tone = "danger", size = "sm" }),
				ui.div { grow = 1 },
				IconButton { icon = "close", label = locale.t("common.close"), size = "md", tone = "quiet", on_click = close },
			},
			ui.column {
				class = "orch-decision-question",
				w_full = true,
				text_size = "$font.size.lg",
				line_height = "$font.line.lg",
				weight = "$font.weight.semibold",
				color = "$text.primary",
				Markdown {
					text = function(): string
						local a = ask()
						return (a and a.question) or ""
					end,
				},
			},
			-- Whose, whose orchestrator, how long ago; its links.
			ui.row {
				class = "orch-decision-meta",
				w_full = true,
				gap = "$space.2",
				items = "center",
				ui.button {
					class = "orch-decision-thread",
					-- (Its name is the decision's words too: a selection takes it,
					-- as zgx leaves a button's label out unless it says so.)
					selectable = true,
					label = function(): string
						local a = ask()
						return locale.s("ds.orch.decide.open_thread_named", { name = props.name_of(a and a.from_chat) or "" })
					end,
					tooltip = locale.s("ds.orch.decide.open_at"),
					tooltip_delay = 400,
					h = 22,
					px = "$zeron.space.tight",
					mx = ui.raw(-4, "its words in line with the question's"),
					gap = "$zeron.space.tight",
					items = "center",
					rounded = "$radius.md",
					bg = "#00000000",
					hover = { bg = "$hover" },
					active = { bg = "$pressed" },
					focus = control.ring,
					on_click = open_thread,
					hue.Dot {
						hue = function(): number?
							local a = ask()
							return if props.hue_of and a then props.hue_of(a.from_chat) else nil
						end,
						size = 6,
					},
					ui.text {
						function(): string
							local a = ask()
							return props.name_of(a and a.from_chat) or ""
						end,
						text_size = "$font.size.sm",
						weight = "$font.weight.medium",
						color = "$text.secondary",
					},
				},
				ui.show(function(): boolean
					local a = ask()
					return a ~= nil and a.orchestrator_id ~= nil and a.orchestrator_id ~= a.from_chat and (props.name_of(a.orchestrator_id) or "") ~= ""
				end, ui.row {
					gap = "$space.2",
					items = "center",
					ui.text { "·", text_size = "$font.size.sm", color = "$text.tertiary" },
					ui.text {
						function(): string
							local a = ask()
							return props.name_of(a and a.orchestrator_id) or ""
						end,
						text_size = "$font.size.sm",
						color = "$text.tertiary",
					},
				}),
				ui.text { "·", text_size = "$font.size.sm", color = "$text.tertiary" },
				ui.text {
					function(): string
						local a = ask()
						return locale.s("ds.orch.decide.asked_ago", { ago = status.ago(status.since((a and a.created) or 0)) })
					end,
					tabular = true,
					text_size = "$font.size.sm",
					color = "$text.tertiary",
				},
				ui.div { w = ui.raw(4, "the links set a little apart") },
				ui.each(ui.computed(function(): { any }
					local a = ask()
					return (a and a.links) or {}
				end), function(l: any): Node
					return link(props, l)
				end, { key = function(l: any): string
					return `{l.kind}:{l.target}`
				end, direction = "row", gap = "$zeron.space.tight", items = "center" }),
			},
		},
		ui.div { w_full = true, h = 1, shrink = 0, bg = "$border.subtle" },
		-- What it's about, and the options.
		ui.scroll {
			class = "orch-decision-body",
			w_full = true,
			-- (As tall as what it holds, up to the window's share less the
			-- head and the foot; past that it scrolls.)
			h = function(): number
				local most = math.max(200, math.floor(window_size().height * 0.86) - 300)
				local own = body_h:get()
				return if own > 0 then math.min(own, most) else most
			end,
			ui.column {
				w_full = true,
				px = PAD_X,
				py = "$space.4",
				gap = "$space.4",
				on_resize = function(size: any)
					if math.abs((size.h or 0) - body_h:peek()) >= 1 then
						body_h:set(size.h or 0)
					end
				end,
				ui.show(function(): boolean
					local a = ask()
					return a ~= nil and type(a.context) == "string" and a.context ~= ""
				end, ui.column {
					class = "orch-decision-context",
					w_full = true,
					gap = "$space.2",
					ui.button {
						class = "orch-decision-context-toggle",
						label = locale.t("ds.orch.decide.context"),
						expanded = function(): boolean
							return context_open:get()
						end,
						h = 24,
						px = "$space.2",
						mx = ui.raw(-8, "its words in line with the text's"),
						gap = "$space.2",
						items = "center",
						self = "start",
						rounded = "$radius.md",
						bg = "#00000000",
						hover = { bg = "$hover" },
						focus = control.ring,
						on_click = function()
							context_open:set(not context_open:peek())
						end,
						ui.div {
							color = "$text.tertiary",
							Icon {
								name = function(): any
									return if context_open:get() then "alt-arrow-down" else "alt-arrow-right"
								end,
								size = 12,
							},
						},
						ui.text { locale.t("ds.orch.decide.context"), text_size = "$font.size.sm", weight = "$font.weight.medium", color = "$text.secondary" },
					},
					ui.show(function(): boolean
						return context_open:get()
					end, ui.column {
						w_full = true,
						max_w = 760,
						Markdown {
							text = function(): string
								local a = ask()
								return (a and a.context) or ""
							end,
							small = true,
							ink = "$text.secondary",
							copy_quotes = true,
						},
					}),
				}),
				-- The ask's own pictures (not an option's).
				ui.show(function(): boolean
					return #own_images:get() > 0
				end, ui.row {
					class = "orch-decision-shots",
					flex_wrap = true,
					gap = "$space.2",
					ui.each(own_images, function(_it: any, i: number): Node
						return thumb(`decision:{(ask() or {}).id or ""}:own`, own_images:peek(), i, 144, 90, 8)
					end, { direction = "row", gap = "$space.2" }),
				}),
				ui.div {
					class = "orch-decision-options",
					w_full = true,
					-- (Out of the overlay's own region: each option is its own,
					-- the outer one would keep a press on a button its button's.)
					selectable = false,
					on_resize = function(size: any)
						if math.abs((size.w or 0) - grid_w:peek()) >= 1 then
							grid_w:set(size.w or 0)
						end
					end,
					ui.each(opts, function(o: decide.Option): Node
						return option(o)
					end, {
						key = function(o: decide.Option): string
							return `{o.index}:{o.label}`
						end,
						direction = "row",
						flex_wrap = true,
						gap = GAP,
						items = "start",
						w_full = true,
					}),
				},
			},
		},
		ui.div { w_full = true, h = 1, shrink = 0, bg = "$border.subtle" },
		-- The foot: a reply, and the ways out.
		ui.column {
			class = "orch-decision-foot",
			w_full = true,
			shrink = 0,
			px = PAD_X,
			pt = "$space.3",
			pb = "$space.4",
			gap = "$space.3",
			TextArea {
				value = reply,
				name = locale.t("ds.orch.ask.reply"),
				placeholder = locale.t("ds.orch.decide.reply"),
				rows = 1,
				max_rows = 6,
				size = "md",
				w_full = true,
				on_key = function(e: any): boolean?
					if e.key == "Enter" and not (e.shift or e.cmd or e.alt or e.ctrl or e.meta) then
						go()
						return true
					end
					-- (A digit is the reply's, not a choice.)
					typing = true
					return false
				end,
			},
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				Button {
					tone = "ghost",
					size = "sm",
					on_click = open_thread,
					ui.div { color = "$text.tertiary", Icon { name = "chat-round-line", size = 14 } },
					locale.t("ds.orch.decide.open_thread"),
				},
				ui.div { grow = 1 },
				ContextMenu {
					items = function(): { any }
						return defer_items()
					end,
					open = defer_open,
					label = locale.t("ds.orch.decide.defer"),
					Button {
						tone = "secondary",
						size = "sm",
						on_click = function()
							defer_open:set(true)
						end,
						ui.div { color = "$text.tertiary", Icon { name = "clock-circle", size = 14 } },
						locale.t("ds.orch.decide.defer"),
					},
				},
				Button {
					tone = "primary",
					size = "sm",
					disabled = function(): boolean
						return not ready()
					end,
					on_click = go,
					locale.t("ds.orch.decide.decide"),
				},
			},
		},
	}, "menu") :: any)
end)

-- How long after opening a close isn't the person's (no one closes what
-- hasn't finished opening): the modal's own, its focus scope not taken.
local SETTLE_MS = 250

-- The overlay itself, one for every kind it holds: over a scrim, about
-- seven tenths of the window, eased in and out; open while `open()` holds,
-- closed by the person (Escape, a click outside: `on_close`).
type OverlayArgs = {
	open: () -> boolean,
	on_close: () -> (),
	label: any,
	body: (phase: State<number>) -> Node,
}
local Overlay = ui.component(function(args: OverlayArgs): Node
	local open = args.open
	-- When it last opened (`ui.now()`), and whether a close while it was
	-- still opening was let go already (once an opening).
	local opened_at = -math.huge
	local let_go = false
	local phase = ui.use_state(if open() then 1 else 0)
	local shown = ui.use_state(open())
	local moving: any = nil
	ui.effect(function()
		local want = open()
		if moving then
			moving:cancel()
			moving = nil
		end
		if want then
			if not shown:peek() then
				opened_at, let_go = ui.now(), false
			end
			shown:set(true)
			local opening: TableParams = { value = 1, duration = IN.duration, ease = IN.ease }
			moving = anime.animate(phase, motion.params(opening))
		elseif shown:peek() then
			local closing: TableParams = {
				value = 0,
				duration = OUT.duration,
				ease = OUT.ease,
				onComplete = function()
					shown:set(false)
				end,
			}
			moving = anime.animate(phase, motion.params(closing))
		end
	end)
	return ui.modal {
		open = function(): boolean
			return shown:get()
		end,
		on_close = function()
			-- (Closed by the modal itself as it opened, once: kept open.)
			if not let_go and ui.now() - opened_at < SETTLE_MS then
				let_go = true
				return
			end
			args.on_close()
		end,
		label = args.label,
		w = function(): number
			local s = window_size()
			return math.floor(math.clamp(s.width * 0.7, math.min(560, s.width - 32), 1180))
		end,
		max_w = function(): number
			return window_size().width - 32
		end,
		max_h = function(): number
			return math.floor(window_size().height * 0.9)
		end,
		p = 0,
		gap = 0,
		rounded = "$zeron.dialog.radius",
		bg = "#00000000",
		border = 0,
		shadow = "$zeron.dialog.stage",
		dismiss_on_backdrop = true,
		build = function()
			return ui.column {
				w_full = true,
				-- Its words select and copy as a transcript's do: a drag, a
				-- double click, Ctrl/Cmd+C, and the selection's own menu
				-- (Copy) on a secondary click (ds/selection_menu).
				SelectionMenu { w_full = true, args.body(phase) },
				Lightbox.Host(),
			}
		end,
	}
end)

local Decision = ui.component(function(props: Props): Node
	-- The shown ask as last seen: the overlay keeps it through a frame of
	-- the engine's state without it, rather than closing.
	local known: any = nil
	local function ask(): any
		local id = decide.shown:get()
		if not id then
			return nil
		end
		local a = props.ask()
		if a then
			known = a
			return a
		end
		return if known and known.id == id then known else nil
	end
	local body_props = setmetatable({ ask = ask }, { __index = props }) :: any
	return Overlay {
		open = function(): boolean
			return ask() ~= nil
		end,
		on_close = decide.hide,
		label = locale.t("ds.orch.decide.label"),
		body = function(phase: State<number>): Node
			return Body { props = body_props, phase = phase }
		end,
	}
end)

-- ── A Q&A entry, whole ─────────────────────────────────────────────────────

export type AnswerProps = {
	entry: () -> any,
	on_close: () -> (),
	name_of: (id: string?) -> string?,
	hue_of: ((id: string?) -> number?)?,
	on_open: (chat: string) -> (),
	on_source: ((entry: any) -> ())?,
	on_read: (entry: any, read: boolean) -> (),
	on_dismiss: (entry: any) -> (),
	on_follow_up: (entry: any, text: string) -> any,
}

-- How an entry stands, as Zeron's status says it.
local ENTRY_STATUS: { [string]: status.Kind } = { unread = "done", read = "done", waiting = "waiting" }

local AnswerBody = ui.component(function(args: { props: AnswerProps, entry: () -> any, phase: State<number> }): Node
	local props, phase = args.props, args.phase
	local entry = args.entry
	local follow = ui.use_state("")
	local body_h = ui.use_state(0)
	local function answered(): boolean
		local e = entry()
		return e ~= nil and e.answer ~= nil
	end
	local function unread(): boolean
		local e = entry()
		return e ~= nil and e.status == "unread"
	end
	local function state(): status.Kind
		local e = entry()
		if not e then
			return "waiting"
		end
		if e.answer == nil and e.answering then
			return "working"
		end
		return ENTRY_STATUS[e.status] or "waiting"
	end
	local function said(): string
		local e = entry()
		if e and e.answer ~= nil then
			return locale.s("ds.orch.qa.kind_answer")
		end
		return locale.s(if e and e.answering then "ds.orch.qa.answering" else "ds.orch.qa.waiting")
	end
	local function open_thread()
		local e = entry()
		if e and e.chat then
			props.on_close()
			props.on_open(e.chat)
		end
	end
	local function ready(): boolean
		return string.match(follow:get(), "%S") ~= nil
	end
	local function ask()
		local e = entry()
		local text = follow:peek()
		if not e or not string.match(text, "%S") then
			return
		end
		props.on_follow_up(e, (string.gsub(text, "^%s+", "")))
		follow:set("")
		props.on_close()
	end
	local function who(): string
		local e = entry()
		return (e and props.name_of(e.chat)) or ""
	end

	return ui.column(glass.style({
		class = "orch-decision orch-qa-dialog",
		w_full = true,
		p = 0,
		gap = 0,
		overflow = "hidden",
		rounded = "$zeron.dialog.radius",
		shadow = "$zeron.dialog.shadow",
		opacity = function()
			return phase:get()
		end,
		scale = function()
			return 0.98 + 0.02 * phase:get()
		end,
		y = function()
			return 6 * (1 - phase:get())
		end,
		on_key = function(e: any): boolean
			if e.key == "Escape" and not (e.ctrl or e.alt or e.meta or e.cmd or e.shift) then
				props.on_close()
				return true
			end
			return false
		end,
		-- The head: how it stands, its question, who answered and when.
		ui.column {
			w_full = true,
			shrink = 0,
			px = PAD_X,
			pt = "$space.5",
			pb = "$space.4",
			gap = "$space.3",
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				status.StatusIcon { kind = state, size = 14 },
				ui.text {
					said,
					class = "orch-qa-dialog-state",
					text_size = "$font.size.xs",
					weight = "$font.weight.medium",
					color = function(): string
						return `$zeron.status.{state()}`
					end,
				},
				ui.show(unread, ui.div {
					class = "orch-qa-dialog-unread",
					role = "img",
					label = locale.t("ds.orch.qa.unread"),
					w = 6,
					h = 6,
					rounded = "$radius.full",
					bg = "$accent",
				}),
				ui.div { grow = 1 },
				ui.show(answered, IconButton({
					class = "orch-qa-dialog-read",
					icon = function(): Icon.Name
						return if unread() then "eye" else "eye-closed"
					end,
					label = function(): string
						return locale.s(if unread() then "ds.orch.qa.mark_read" else "ds.orch.qa.mark_unread")
					end,
					size = "md",
					tone = "quiet",
					on_click = function()
						local e = entry()
						if e then
							props.on_read(e, unread())
						end
					end,
				} :: any)),
				IconButton {
					class = "orch-qa-dialog-dismiss",
					icon = "archive-minimalistic",
					label = locale.t("ds.orch.qa.dismiss"),
					size = "md",
					tone = "quiet",
					on_click = function()
						local e = entry()
						if e then
							props.on_close()
							props.on_dismiss(e)
						end
					end,
				},
				IconButton { icon = "close", label = locale.t("common.close"), size = "md", tone = "quiet", on_click = props.on_close },
			},
			ui.column {
				class = "orch-decision-question",
				w_full = true,
				text_size = "$font.size.lg",
				line_height = "$font.line.lg",
				weight = "$font.weight.semibold",
				color = "$text.primary",
				Markdown {
					text = function(): string
						local e = entry()
						return (e and e.question) or ""
					end,
					breaks = true,
				},
			},
			-- Who answered (a way into the thread), when; where it was asked.
			ui.row {
				class = "orch-decision-meta",
				w_full = true,
				gap = "$space.2",
				items = "center",
				ui.show(function(): boolean
					local e = entry()
					return e ~= nil and e.chat ~= nil
				end, ui.button {
					class = "orch-decision-thread orch-qa-dialog-thread",
					-- (Its name is the decision's words too: a selection takes it,
					-- as zgx leaves a button's label out unless it says so.)
					selectable = true,
					label = function(): string
						return locale.s("ds.orch.qa.open_in", { name = who() })
					end,
					h = 22,
					px = "$zeron.space.tight",
					mx = ui.raw(-4, "its words in line with the question's"),
					gap = "$zeron.space.tight",
					items = "center",
					rounded = "$radius.md",
					bg = "#00000000",
					hover = { bg = "$hover" },
					active = { bg = "$pressed" },
					focus = control.ring,
					on_click = open_thread,
					hue.Dot {
						hue = function(): number?
							local e = entry()
							return if props.hue_of and e then props.hue_of(e.chat) else nil
						end,
						size = 6,
					},
					ui.text { who, text_size = "$font.size.sm", weight = "$font.weight.medium", color = "$text.secondary" },
				}),
				ui.text { "·", text_size = "$font.size.sm", color = "$text.tertiary" },
				ui.text {
					function(): string
						local e = entry()
						if e and e.answered then
							return locale.s("ds.orch.qa.answered_ago", { ago = status.ago(status.since(e.answered)) })
						end
						return locale.s("ds.orch.decide.asked_ago", { ago = status.ago(status.since((e and e.asked) or 0)) })
					end,
					class = "orch-qa-dialog-when",
					tabular = true,
					text_size = "$font.size.sm",
					color = "$text.tertiary",
				},
				ui.show(function(): boolean
					return props.on_source ~= nil and entry() ~= nil
				end, ui.row {
					gap = "$space.2",
					items = "center",
					ui.div { w = ui.raw(4, "the link set a little apart") },
					ui.button {
						class = "orch-decision-link orch-qa-dialog-source",
						label = locale.t("ds.orch.qa.where_asked"),
						h = 22,
						px = "$space.2",
						gap = "$zeron.space.tight",
						items = "center",
						shrink = 0,
						rounded = "$radius.md",
						bg = "#00000000",
						hover = { bg = "$hover" },
						active = { bg = "$pressed" },
						focus = control.ring,
						on_click = function()
							local e = entry()
							if e and props.on_source ~= nil then
								local source = props.on_source :: (entry: any) -> ()
								props.on_close()
								source(e)
							end
						end,
						ui.div { color = "$text.tertiary", Icon { name = "arrow-up-right", size = 12 } },
						ui.text { locale.t("ds.orch.qa.where_asked"), text_size = "$font.size.xs", color = "$text.secondary" },
					},
				}),
			},
		},
		ui.div { w_full = true, h = 1, shrink = 0, bg = "$border.subtle" },
		-- The answer, whole, as the thread wrote it.
		ui.scroll {
			class = "orch-decision-body",
			w_full = true,
			h = function(): number
				local most = math.max(200, math.floor(window_size().height * 0.86) - 300)
				local own = body_h:get()
				return if own > 0 then math.min(own, most) else most
			end,
			ui.column {
				w_full = true,
				px = PAD_X,
				py = "$space.4",
				gap = "$space.4",
				on_resize = function(size: any)
					if math.abs((size.h or 0) - body_h:peek()) >= 1 then
						body_h:set(size.h or 0)
					end
				end,
				ui.show(answered, ui.column {
					class = "orch-qa-dialog-answer",
					w_full = true,
					max_w = 760,
					Markdown {
						text = function(): string
							local e = entry()
							return (e and e.answer) or ""
						end,
						copy_quotes = true,
					},
				}),
				ui.show(function(): boolean
					return not answered()
				end, ui.text {
					function(): string
						local e = entry()
						return locale.s(if e and e.answering then "ds.orch.qa.dialog_answering" else "ds.orch.qa.dialog_waiting")
					end,
					class = "orch-qa-dialog-none",
					text_size = "$font.size.sm",
					color = "$text.tertiary",
				}),
			},
		},
		ui.div { w_full = true, h = 1, shrink = 0, bg = "$border.subtle" },
		-- The foot: a follow-up, asked of the same thread.
		ui.column {
			class = "orch-decision-foot",
			w_full = true,
			shrink = 0,
			px = PAD_X,
			pt = "$space.3",
			pb = "$space.4",
			gap = "$space.3",
			TextArea {
				value = follow,
				name = locale.t("ds.orch.qa.follow_up_label"),
				placeholder = locale.t("ds.orch.qa.follow_up"),
				rows = 1,
				max_rows = 6,
				size = "md",
				w_full = true,
				on_key = function(e: any): boolean?
					if e.key == "Enter" and not (e.shift or e.cmd or e.alt or e.ctrl or e.meta) then
						ask()
						return true
					end
					return false
				end,
			},
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				ui.show(function(): boolean
					local e = entry()
					return e ~= nil and e.chat ~= nil
				end, Button {
					tone = "ghost",
					size = "sm",
					on_click = open_thread,
					ui.div { color = "$text.tertiary", Icon { name = "chat-round-line", size = 14 } },
					locale.t("ds.orch.decide.open_thread"),
				}),
				ui.div { grow = 1 },
				Button {
					class = "orch-qa-dialog-ask",
					tone = "primary",
					size = "sm",
					disabled = function(): boolean
						return not ready()
					end,
					on_click = ask,
					locale.t("ds.orch.qa.ask"),
				},
			},
		},
	}, "menu") :: any)
end)

local Answer = ui.component(function(props: AnswerProps): Node
	-- The entry as last seen: one dismissed or moved under it stays shown
	-- until it closes.
	local known: any = nil
	local function entry(): any
		local e = props.entry()
		if e then
			known = e
		end
		return e
	end
	local function open(): boolean
		return entry() ~= nil
	end
	return Overlay {
		open = open,
		on_close = props.on_close,
		label = locale.t("ds.orch.qa.dialog_label"),
		body = function(phase: State<number>): Node
			return AnswerBody {
				props = props,
				phase = phase,
				entry = function(): any
					return entry() or known
				end,
			}
		end,
	}
end)

-- (Called as the decision; `.Answer`, a Q&A entry's.)
return setmetatable({ Answer = Answer }, {
	__call = function(_self: any, props: Props): Node
		return Decision(props)
	end,
}) :: any
