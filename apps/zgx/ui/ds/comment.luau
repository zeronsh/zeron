-- Comments: what people say about a change, and where they say more. A
-- comment card (its author's face and name, when, a badge for what kind
-- of comment it is, the words, and its actions under the pointer); a
-- thread (comments one under another on a hairline, a reply at its foot);
-- and the reply composer, which rests as a one-line "Reply…" and opens
-- into a field with its buttons when touched.
--
--   CommentCard { author = { id = "ada", name = "Ada" }, at = t, body = "…" }
--   Thread { location = "ds/badge.luau:42", comments = { … }, on_reply = … }
--   ReplyComposer { value = draft, on_submit = send, on_cancel = … }
--
-- Inline in a diff, a thread sits on the quiet card; on a pull request's
-- page, plain on the page.

local Button = require("@self/button")
local IconButton = require("@self/icon_button").IconButton
local Icon = require("@self/icon")
local TextArea = require("@self/text_area")
local people = require("@self/people")
local SelectionMenu = require("@self/selection_menu")
local locale = require("@self/locale")

local M = {}

export type Comment = {
	author: people.Who,
	-- When it was said; none for a draft not sent yet (the code editor's).
	at: number?,
	body: string,
	-- A badge after the time (a review's verdict, "Author", "Pending").
	badge: Node?,
	-- Yours: its edit and delete under the pointer.
	mine: boolean?,
	edited: boolean?,
	-- What its corner offers under the pointer (or the keys), in place of
	-- edit, delete and More: each drawn by the design system (`Action`), or
	-- a node of the caller's own.
	actions: { Action | Node }?,
	-- Its words drawn otherwise (their markdown), in place of `body`.
	content: Node?,
}

-- One of a comment's actions: a glyph and what it's called (its tooltip,
-- under it), `danger` for what can't be taken back (delete), `role`
-- "link" for one that goes elsewhere (open on GitHub).
export type Action = {
	icon: Icon.Name,
	label: string | Message,
	on_click: (() -> ())?,
	danger: boolean?,
	role: string?,
	id: string?,
}

-- A comment's actions as its corner wears them: quiet 24-point icon
-- buttons, tooltips under them, on a small flat group, a chip's fill, as
-- tall as they are: no inset, so its corner is theirs (concentric), and
-- it sits on the comment's head line. `focused(inside)` hears the keys come
-- and go, so the group shows while one of them has focus.
-- `box`: where it stands and how it shows (its caller's: a corner, a fade).
function M.Actions(list: { Action | Node }, focused: ((inside: boolean) -> ())?, box: { [string]: any }?): Node
	local buttons: { Node } = {}
	for _, a in list do
		if type(a) == "table" and rawget(a :: any, "icon") ~= nil then
			local act = a :: Action
			table.insert(buttons, IconButton {
				id = act.id,
				icon = act.icon,
				label = act.label :: any,
				role = act.role,
				size = "md",
				tone = if act.danger then "danger" else "quiet",
				tooltip_side = "bottom",
				on_click = act.on_click,
				on_focus = if focused then function()
					focused(true)
				end else nil,
				on_blur = if focused then function()
					focused(false)
				end else nil,
			})
		else
			table.insert(buttons, a :: Node)
		end
	end
	local group: { [any]: any } = {
		class = "comment-actions",
		role = "toolbar",
		label = locale.t("ds.comment.actions"),
		gap = 0,
		items = "center",
		rounded = "$zeron.icon_button.radius",
		bg = "$zeron.secondary.bg",
		table.unpack(buttons),
	}
	for k, v in box or {} do
		group[k] = v
	end
	return ui.row(group)
end

M.CommentCard = ui.component(function(props: Comment & { on_edit: (() -> ())?, on_delete: (() -> ())?, compact: boolean? }): Node
	local over = ui.use_state(false)
	local focus = ui.use_state(false)
	local size = if props.compact then 20 else 24
	local actions: { Action | Node } = {}
	if props.actions then
		actions = props.actions
	else
		if props.mine then
			table.insert(actions, { icon = "pen", label = locale.t("common.edit"), on_click = props.on_edit } :: Action)
			table.insert(actions, { icon = "trash-bin-minimalistic", label = locale.t("common.delete"), danger = true, on_click = props.on_delete } :: Action)
		end
		table.insert(actions, { icon = "more-horizontal", label = locale.t("common.more") } :: Action)
	end
	local function shown(): boolean
		return over:get() or focus:get()
	end
	return ui.row {
		role = "article",
		label = `{props.author.name}: {props.body}`,
		w_full = true,
		-- (Its own box: its actions sit in its corner, never the thread's.)
		relative = true,
		gap = "$space.3",
		items = "start",
		on_hover = function(inside: boolean)
			over:set(inside)
		end,
		people.face(props.author, size),
		ui.column {
			grow = 1,
			min_w = 0,
			gap = "$space.1",
			-- Who, when and what kind, wrapping in a narrow pane.
			ui.row {
				w_full = true,
				min_h = size,
				flex_wrap = true,
				gap = "$space.2",
				items = "center",
				people.name(props.author, "$font.size.md"),
				if props.at then people.Timestamp { at = props.at } else nil,
				if props.edited then ui.text { "· edited", shrink = 0, text_size = "$font.size.sm", color = "$text.tertiary" } else nil,
				props.badge,
			},
			props.content or SelectionMenu { w_full = true, ui.text {
				props.body,
				wrap = true,
				text_size = "$font.size.md",
				line_height = 20,
				color = "$text.primary",
			} },
		},
		-- Its actions, in its own top-right corner, on its head's line: there while the pointer is on it or the keys are in them
		-- (Tab reaches them unseen, and they show as it does). They float,
		-- so they never take its words' room.
		if #actions > 0
			then M.Actions(actions, function(inside: boolean)
				focus:set(inside)
			end, {
				absolute = true,
				top = 0,
				right = 0,
				opacity = ui.tween(ui.computed(function(): number
					return if shown() then 1 else 0
				end), { duration = 120 }),
			})
			else nil,
	}
end)

-- The reply composer: resting, a one-line prompt in the field's frame
-- (a face before it, yours); touched, it opens into a multi-line field
-- with Cancel and the way on, ⌘↵ sending, Escape folding it again.
export type ComposerProps = {
	value: State<string>,
	placeholder: string?,
	-- What assistive technology calls the field ("Reply").
	name: string?,
	-- Whose face goes before it.
	me: people.Who?,
	submit: string?,
	on_submit: ((text: string) -> ())?,
	on_cancel: (() -> ())?,
	-- Open from the start (an inline comment just begun).
	open: boolean?,
	-- A second way on before the first (Start a review).
	secondary: { label: string, on_click: () -> () }?,
}

M.ReplyComposer = ui.component(function(props: ComposerProps): Node
	local open = ui.use_state(props.open == true or props.value:peek() ~= "")
	local value = props.value
	local function send()
		local text = value:peek()
		if string.match(text, "%S") == nil then
			return
		end
		if props.on_submit then
			props.on_submit(text)
		end
		value:set("")
		open:set(false)
	end
	local function cancel()
		value:set("")
		open:set(false)
		if props.on_cancel then
			props.on_cancel()
		end
	end
	return ui.row {
		w_full = true,
		gap = "$space.3",
		items = "start",
		if props.me then ui.div { pt = "$space.1", people.face(props.me, 24) } else nil,
		ui.show(
			function()
				return open:get()
			end,
			ui.column {
				grow = 1,
				min_w = 0,
				gap = "$space.2",
				TextArea {
					value = value,
					placeholder = props.placeholder or locale.t("ds.comment.reply_placeholder"),
					name = props.name or locale.t("ds.comment.reply"),
					rows = 3,
					max_rows = 12,
					w_full = true,
					autofocus = true,
					on_submit = function()
						send()
					end,
					on_key = function(e)
						if e.key == "Escape" then
							cancel()
							return true
						end
						return false
					end,
				},
				ui.row {
					gap = "$space.2",
					items = "center",
					ui.text { locale.t("ds.comment.send_hint", { key = ui.shortcut_text("cmd+enter") }), grow = 1, text_size = "$font.size.xs", color = "$text.tertiary" },
					Button { locale.t("common.cancel"), tone = "ghost", size = "sm", on_click = cancel },
					if props.secondary then Button { props.secondary.label, tone = "secondary", size = "sm", on_click = props.secondary.on_click } else nil,
					Button {
						props.submit or locale.t("ds.comment.comment"),
						size = "sm",
						disabled = function()
							return string.match(value:get(), "%S") == nil
						end,
						on_click = send,
					},
				},
			},
			-- Resting: a field-shaped prompt that opens it.
			ui.row {
				grow = 1,
				h = 32,
				px = "$zeron.space.snug",
				gap = "$space.2",
				items = "center",
				rounded = "$radius.lg",
				border = 1,
				bg = "$zeron.field.bg",
				border_color = "$zeron.field.edge",
				role = "button",
				label = props.placeholder or locale.t("ds.comment.reply"),
				focusable = true,
				cursor = "text",
				hover = { border_color = "$zeron.field.edge_hover" },
				focus = { outline = "$zeron.focus.width", outline_color = "$zeron.focus.ring", outline_offset = "$zeron.focus.offset" },
				on_click = function()
					open:set(true)
				end,
				ui.text { props.placeholder or locale.t("ds.comment.reply_placeholder"), text_size = "$font.size.md", color = "$zeron.field.placeholder" },
			}
		),
	}
end)

-- Where a comment is: a file and line, on a quiet chip, heading it. Short of
-- room its text fades at its start, so the file and line always read.
function M.Location(text: string): Node
	local clip, words = ui.ref(), ui.ref()
	local function over(): boolean
		return (words.width or 0) > (clip.width or 0) + 0.5
	end
	return ui.row {
		h = 22,
		px = "$space.2",
		gap = "$space.1",
		min_w = 0,
		items = "center",
		rounded = "$radius.md",
		bg = "$zeron.secondary.bg",
		color = "$text.secondary",
		ui.div { y = -0.5, shrink = 0, Icon { name = "document", size = 12 } },
		ui.row {
			ref = clip,
			min_w = 0,
			overflow = "hidden",
			items = "center",
			justify = function(): Justify
				return if over() then "end" else "start"
			end,
			fade_x = function()
				return { if over() then 16 else 0, 0 }
			end,
			ui.text { text, ref = words, literal = true, shrink = 0, font = "$font.mono", text_size = "$font.size.xs" },
		},
	}
end

-- A thread: comments one under another, a hairline down from the first
-- face to the last, and the reply composer at its foot. `location` heads
-- it (an inline thread's file and line), with whether it is resolved.
export type ThreadProps = {
	comments: { Comment },
	location: string?,
	resolved: (State<boolean>)?,
	draft: State<string>,
	me: people.Who?,
	on_reply: ((text: string) -> ())?,
	-- Asked to resolve it (true) or open it again (false), in place of
	-- `resolved` changing here: a thread kept elsewhere (GitHub's) says so
	-- when it has, and `resolved` follows.
	on_resolve: ((resolve: boolean) -> ())?,
	-- On the quiet card (inline in a diff), or plain.
	card: boolean?,
}

function M.Thread(props: ThreadProps): Node
	local items: { Node } = {}
	for _, c in props.comments do
		table.insert(items, M.CommentCard(c :: any))
	end
	local head: Node? = nil
	if props.location or props.resolved then
		local resolved = props.resolved
		head = ui.row {
			gap = "$space.2",
			items = "center",
			if props.location then M.Location(props.location) else nil,
			ui.div { grow = 1 },
			if resolved
				then Button {
					tone = "ghost",
					size = "sm",
					-- Its label on the thread's edge, its wash past it.
					mr = ui.raw(-10, "a small button's padding"),
					on_click = function()
						if props.on_resolve then
							props.on_resolve(not resolved:peek())
						else
							resolved:toggle()
						end
					end,
					ui.div {
						y = -0.5,
						Icon {
							name = function()
								return if resolved:get() then "check-circle" else "check"
							end,
							size = 14,
						},
					},
					function()
						return if resolved:get() then locale.s("ds.comment.resolved") else locale.s("ds.comment.resolve")
					end,
				}
				else nil,
		}
	end
	return ui.column {
		w_full = true,
		gap = "$space.4",
		p = if props.card then "$space.4" else nil,
		rounded = if props.card then "$radius.xl" else nil,
		bg = if props.card then "$zeron.settings.card" else nil,
		border = if props.card then 1 else nil,
		border_color = if props.card then "$zeron.settings.edge" else nil,
		head,
		ui.column { gap = "$space.4", table.unpack(items) },
		M.ReplyComposer { value = props.draft, me = props.me, on_submit = props.on_reply },
	}
end

return M
