-- A conversation: what a person sent and what the agent did and said, as
-- Zeron's transcript shows it (crates/ui/src/transcript.rs), virtualized so
-- a chat of thousands of turns costs what the few in view cost.
--
-- Each message is a row of a `ui.list`, measured as it mounts and keyed by
-- its id; only the rows in view (and a couple past each edge) are built. A
-- reply's row is built once and follows its message as it grows: only the
-- block being written is parsed and drawn again (`ui.markdown`'s `key`),
-- and no other row runs.
--
-- Reading:
-- - Both sides keep to one measure (`$zeron.prose.measure`), centred, so a
--   wide window doesn't stretch a reply's lines past easy reading; the
--   person's bubbles hold its right edge.
-- - A turn opens with more room above it than lies between a reply's parts,
--   so the conversation reads in turns.
-- - Text selects across messages, as a page's does, and copies rows scrolled
--   away (`row_text`).
--
-- Scrolling:
-- - Resting at the end, it stays there as a reply grows and rows arrive;
--   scrolled up, it stays where it was (a reply streaming below doesn't
--   pull the reader down).
-- - Away from the end, "Jump to latest" floats over its foot ("New reply"
--   while one is being written); taking it goes to the end and follows again.
-- - Rows measured above the view (scrolling up into rows not yet seen, a
--   bubble opening) don't move the rows in it.
--
-- Quoting:
-- - A selection (one message or several; a code block, a tool's row) offers
--   Mention and Attach under its last line, clear of the text: above its
--   first line if the view has no room below, and at the view's foot if the
--   selection fills it. It goes on a scroll, or when the selection does. zgx
--   gives scripts no selection, so the host reads it (`read_selection`), each
--   time a press lets go in the list. Its context menu (ds/selection_menu)
--   offers the same two.
--
-- `Transcript { messages = state, offset = state }`; `latest()`
-- goes to the end (a person sending goes there too, whatever they were
-- reading).

-- When a transcript was last made: a reply already streaming then keeps what it shows,
-- only what comes after fading in.
local opened_at = 0
local Icon = require("@self/icon")
local Markdown = require("@self/markdown")
local UserMessage = require("@self/user_message")
local Lightbox = require("@self/lightbox")
local ReplyPicture = require("@self/reply_picture")
local Visual = require("@self/visual")
local Activity = require("@self/activity")
local status = require("@self/status")
local Working = require("@self/working")
local Mark = require("@self/mark")
local glass = require("@self/glass")
local control = require("@self/control")
local Quote = require("@self/quote")
local SelectionMenu = require("@self/selection_menu")
local Rail = require("@self/rail")
local motion = require("@self/motion")
local locale = require("@self/locale")

export type Tool = Activity.Tool
export type Message = {
	id: string,
	-- A person's, a reply, or the seam where the chat moved to another
	-- agent (`[mark] From → [mark] To` over the prompt that moved it).
	role: "user" | "assistant" | "switch",
	text: string,
	-- A person's: what came with it.
	attachments: { UserMessage.Attachment }?,
	-- When it was sent, or a reply settled ("Jul 1, 4:12 PM"), and the
	-- words its Copy copies: under it, shown on hover. A reply being
	-- written has neither yet.
	at: string?,
	copy: string?,
	-- A reply's model, for naming who said a selection.
	model: string?,
	tools: { Tool }?,
	-- Its parts in order, as Zeron lays a reply out: its text, and between
	-- the text each run of tools as a group of its own (keyed by its first
	-- part), open while it is the tail of a reply being written. Given,
	-- they are what shows (`text` and `tools` then only say what it holds).
	blocks: { Block }?,
	-- Being written: the reply grows, its tools open.
	streaming: boolean?,
	-- When it began (`ui.now() / 1000`), and how long it worked, once done.
	started: number?,
	worked: number?,
	-- A person's newest, its reply not yet begun: on its way ("sending"),
	-- its turn under way ("working", since `since`), or not delivered
	-- ("failed", to send again). The line under it says so at once, in its
	-- reply's place.
	waiting: ("sending" | "working" | "failed")?,
	since: number?,
	-- Its blocks laid out compact (Settings' compact transcript): its work
	-- folded into one group, the reply it ends on shown.
	compact: { Block }?,
	-- A switch's sides: each agent's name and mark.
	from: string?,
	to: string?,
	from_mark: Mark.Name?,
	to_mark: Mark.Name?,
	-- A person's that another chat sent: which, and its title when it was sent.
	sender: { chat: string, title: string }?,
}

export type Props = {
	messages: State<{ Message }>,
	offset: State<number>?,
	-- Its name, for a transcript shown beside the chat page's (a side
	-- chat's): its kept states are its own, and `send` and `latest` reach
	-- it by name. None: the chat page's.
	name: string?,
	-- The list, for `latest`.
	ref: Ref?,
	-- The text of the reply being written, apart from the list: a reply
	-- growing writes here, a small state, rather than into the whole list
	-- on every word; its message takes the text once it's done.
	draft: State<Draft>?,
	-- What a selection's Mention and Attach do with it. With neither, a
	-- selection offers nothing.
	on_mention: ((q: Quote.Quote) -> ())?,
	on_attach: ((q: Quote.Quote) -> ())?,
	-- A prompt that wasn't delivered, sent again (by its id).
	on_retry: ((id: string) -> ())?,
	-- What an unfinished turn's foot says in its working word's place, when
	-- there's no turn to speak of (its host gone, its machine asleep): the
	-- caller's, for a message (nil: the working word). `sending`: a prompt
	-- whose reply hasn't begun.
	turn_state: ((m: Message, sending: boolean) -> TurnLine?)?,
	-- "Jump to latest" pressed. (It has the keys, and leaves as the view
	-- arrives: this says where they go instead of the next control's, which
	-- may be another chat's.)
	on_jump: (() -> ())?,
	-- The way back's pill: its width while it shows, nil once it goes (the
	-- composer's own pill moves aside for it).
	on_jump_width: ((w: number?) -> ())?,
	-- The composer's own pill beside it: its width while it shows. The two
	-- are then a centred pair, the way back on the left.
	beside: (() -> number?)?,
	-- Kept outside the view (shared state), it brings a view made again
	-- back where it was: the message at the view's top, and how far above
	-- the top it started, written as the view goes; nil at the end.
	keep: State<Anchor?>?,
	-- Replies laid out compact (each one's work folded into one group),
	-- as the setting says.
	compact: (() -> boolean)?,
	-- The widest the conversation runs (points; a function follows a
	-- setting as it changes); by default the theme's measure.
	measure: (number | () -> number)?,
	-- Its rows' inset each side, in points; by default the theme's.
	side: number?,
	-- How far over what floats at its foot its fade starts. (A fade's first fifth
	-- hardly shows: this has it show from where it should start.)
	fade_lead: number?,
	-- What floats over its foot (a composer, docked), in points: the
	-- conversation runs under it, its end held clear of it, fading out as
	-- it passes behind (Zeron's band, from the composer's top to the
	-- window's foot), and the way back to the latest stands over it.
	foot: (() -> number)?,
	-- Whether the view this one is made for rested at its end (a chat
	-- switched back to: the session page keeps each chat's place); else the
	-- last transcript's, as a reload's rebuild has it.
	was_at_end: boolean?,
	-- What a reply's pictures (`![alt](path)`) are, read as they're asked
	-- for (screens/chat/reply_images): without it, their alt text.
	pictures: ReplyPicture.Pictures?,
	-- Where the pictures sent with its messages show from (a chat on
	-- another device reads them from there: screens/chat/reply_images'
	-- `for_attachments`); without it, each path as it is.
	sent_pictures: ReplyPicture.Pictures?,
	-- A link in the conversation that isn't the web's (a file a reply
	-- names, a prompt's file mention): what opens it. Without it, it's
	-- styled only.
	open_link: ((target: string) -> ())?,
	-- A chat as it's listed now: its title and agent's mark. Nil for one that
	-- isn't listed, which a message from it can't open.
	chat_of: ((id: string) -> { title: string, mark: Mark.Name? }?)?,
	-- What pressing `Message from Chat` does: opens that chat beside this one.
	open_sender: ((chat: string, title: string) -> ())?,
	-- Those links' context menus where they aren't the web's (ds/markdown's
	-- `link_menu`).
	link_menu: ((target: string) -> { Markdown.LinkEntry }?)?,
	-- What a link is drawn as: one of the chat's files or its agent's
	-- skills, as a chip (ds/markdown `link_chip`).
	link_chip: ((target: string) -> { file: boolean?, skill: string? }?)?,
	-- A find over the conversation (screens/chat/find): its matches marked
	-- in every message, the current one's message scrolled to and its block
	-- into view. `current().row` is its message's place in `messages`.
	find: TranscriptFind?,
	-- A subagent's spawn (an agent row naming its doc): what opens it (a
	-- lane of its own). Without it, the row opens its call as any does.
	on_open_subagent: ((sub: { ref: string, title: string?, status: string? }) -> ())?,
}
export type Anchor = { id: string, dy: number }
export type TranscriptFind = {
	query: () -> string,
	current: () -> { key: string, block: number, k: number, row: number }?,
}
-- The words being written: the whole reply's (`text`), and, with blocks,
-- the text block they grow (`part`) and its words alone (`part_text`).
-- (`thought`, `thought_text`: the thought the reply is at, and its words so far.)
export type Draft = { id: string, text: string, part: string?, part_text: string?, thought: string?, thought_text: string? }
-- A block of a reply: a text, or a group of tools (its line, and whether
-- it is open while the reply is written).
-- In a compact transcript, a reply's work is one group ("work") holding
-- its own blocks, opened.
export type Block = {
	id: string,
	kind: "text" | "tools" | "work" | "visual" | "display_failed" | "images",
	text: string?,
	-- Pictures an agent showed (`show_image`, a generated image), one after
	-- another: each a path on the chat's host and its name (ds/reply_picture).
	images: { { id: string, path: string, name: string? } }?,
	-- A visual's (ds/visual): its script on the chat's host, its title, and
	-- that host's device when it isn't this one; or its script itself, when
	-- the message carries it (a fixture's).
	path: string?,
	source: string?,
	title: string?,
	device: string?,
	tools: { Tool }?,
	summary: string?,
	live: boolean?,
	blocks: { Block }?,
}

-- Rows not yet measured: about a short message.
local ESTIMATE = 96
-- A selection's action: its words' height and its height round them (the
-- bar's even inset and its border, each side), what it keeps from the
-- selection and from the view's edges, and the room the way back to the end
-- takes at the foot.
local WORD, ACTION, CLEAR, MARGIN, FOOT = 26, 32, 6, 8, 46
-- How often a shown action looks whether the selection is still there.
local WATCH = 200

-- The room left under the conversation's last row, over what floats at its
-- foot: Zeron's (transcript.rs, the fade band and 8).
local TAIL = 32
-- Zeron's gaps over a message (transcript.rs `top_gap_for`): a turn's, and
-- a prompt's under the switch seam over it.
local TURN_GAP = 16
local SEAM_GAP = 6

-- `ZERON_ZGX_TRACE_TRANSCRIPT=1`: the transcript's hover and layout, a line
-- each to the app's output (off, nothing is made or printed).
local TRACING = ui.shared("debug.trace_transcript", false):peek() == true
local function trace(what: string, ...: any)
	if TRACING then
		print(`[transcript] {math.floor(ui.now())} {what}`, ...)
	end
end

-- The lane under a message, as Zeron's (transcript.rs, the hover strip):
-- always there, so nothing moves as it shows; under the pointer, the
-- message's time and Copy, then what plugins add (`chat.message.actions`).
-- A person's sits under their bubble's right edge, a reply's on its text's
-- left. 8 above its 24-point line.
local LANE = 32
local LANE_TOP = 8

-- The messages copied a moment ago (each for 1.2 s, then gone): one map,
-- holding only those, not a state per message.
local COPIED = ui.state("transcript_copied", {} :: { [string]: boolean })

local function strip(m: Message, row: State<Message>, hovered: () -> boolean): Node
	local mine = m.role == "user"
	local function set_copied(on: boolean)
		local all = table.clone(COPIED:peek())
		all[m.id] = if on then true else nil
		COPIED:set(all)
	end
	local function is_copied(): boolean
		return COPIED:get()[m.id] == true
	end
	local stop: (() -> ())? = nil
	local function copy()
		local words = row:peek().copy
		if not words then
			return
		end
		ui.clipboard.write(words)
		set_copied(true)
		if stop then
			stop()
		end
		-- (Zeron clears it after 1.2 s.)
		stop = ui.after(1200, function()
			stop = nil
			set_copied(false)
		end)
	end
	return ui.row {
		w_full = true,
		h = LANE,
		pt = ui.raw(LANE_TOP, "Zeron's 8 over the lane's 24-point line"),
		items = "center",
		justify = if mine then "end" else "start",
		selectable = false,
		ui.presence(function(): string?
			-- (Never while a reply is written: it has no time yet.)
			local now = row:get()
			return if hovered() and now.at and not now.streaming then "shown" else nil
		end, function(): Node
			return ui.row {
				class = "message-actions",
				gap = "$space.2",
				items = "center",
				ui.text {
					function(): string
						return row:get().at or ""
					end,
					text_size = "$font.size.sm",
					color = "$text.secondary",
					opacity = ui.raw(0.55, "Zeron's timestamp: the muted ink at 55%"),
				},
				ui.show(
					function(): boolean
						return row:get().copy ~= nil
					end,
					ui.button {
						label = locale.t("ds.transcript.copy_message"),
						tooltip = locale.s("ds.transcript.copy_message"),
						w = 24,
						h = 24,
						p = 0,
						items = "center",
						justify = "center",
						rounded = "$radius.md",
						bg = "#00000000",
						color = "$text.secondary",
						hover = { bg = "$zeron.secondary.bg" },
						active = { bg = "$zeron.secondary.hover" },
						focus = control.ring,
						cursor = "pointer",
						on_click = copy,
						Icon {
							name = function()
								return if is_copied() then "check" else "copy"
							end,
							size = 14,
						},
					}
				),
				-- Where plugins add a message's actions: beside Copy.
				ui.row { hook = "chat.message.actions", gap = "$space.2", items = "center" },
			}
		end, { enter = { opacity = { 0, 1 }, duration = 150 } }),
	}
end

-- Where the chat moved to another agent: `[mark] From → [mark] To`, muted,
-- on the right over the prompt that moved it (Zeron's `switch_marker`).
local function switch_side(name: string?, mark: Mark.Name?): Node
	return ui.row {
		gap = ui.raw(5, "Zeron's caption mark-to-name gap"),
		items = "center",
		min_w = 0,
		if mark then ui.div { aria_hidden = true, Mark { name = mark, size = 14, bare = true } } else nil,
		ui.text { name or "", literal = true, truncate = true, weight = "$font.weight.medium" },
	}
end

local function seam(m: Message): Node
	return ui.row {
		role = "group",
		label = locale.t("ds.transcript.switched", { from = m.from or "?", to = m.to or "?" }),
		w_full = true,
		justify = "end",
		ui.row {
			gap = ui.raw(6, "Zeron's caption gap"),
			items = "center",
			min_w = 0,
			text_size = "$font.size.md",
			color = "$text.secondary",
			selectable = false,
			switch_side(m.from, m.from_mark),
			ui.div {
				aria_hidden = true,
				opacity = ui.raw(0.7, "Zeron's caption arrow, fainter than its names"),
				Icon { name = "arrow-right", size = 12 },
			},
			switch_side(m.to, m.to_mark),
		},
	}
end

-- A message another chat sent: `Message from [mark] Chat`, muted, on the right
-- over its bubble (Zeron's `render_agent_sender`). The chat's title as it is
-- now, and a click opens it in the panel; one that isn't listed reads as it
-- did when sent.
local function sender_line(m: Message, props: Props): Node
	local from = m.sender :: { chat: string, title: string }
	local function now(): { title: string, mark: Mark.Name? }?
		return if props.chat_of then props.chat_of(from.chat) else nil
	end
	return ui.presence(function(): string
		local c = now()
		return if c then `known:{c.mark or ""}` else "unknown"
	end, function(kind: string): Node
		local known = kind ~= "unknown"
		local c = now()
		local mark = if c then c.mark else nil
		local line: { [any]: any } = {
			gap = ui.raw(6, "Zeron's caption gap"),
			items = "center",
			min_w = 0,
			max_w = "80%",
			text_size = "$font.size.md",
			color = "$text.secondary",
			selectable = false,
			ui.text { locale.t("ds.transcript.message_from"), shrink = 0 },
			ui.row {
				gap = ui.raw(5, "Zeron's caption mark-to-name gap"),
				items = "center",
				min_w = 0,
				ui.div {
					aria_hidden = true,
					if mark then Mark { name = mark, size = 14, bare = true } else Icon { name = "chat-round-line", size = 14 },
				},
				ui.text {
					function(): string
						local live = now()
						return if live then live.title else from.title
					end,
					literal = true,
					truncate = true,
					weight = "$font.weight.medium",
				},
			},
		}
		if known then
			line.label = function(): string
				local live = now()
				return `{locale.s("ds.transcript.message_from")} {if live then live.title else from.title}`
			end
			line.cursor = "pointer"
			-- (A line of text that opens, not a control: no box of its own.)
			line.h, line.px, line.py, line.bg = "auto" :: any, 0, 0, "#00000000"
			line.hover = { color = "$text.primary" }
			line.active = { color = "$text.primary" }
			line.on_click = function()
				if props.open_sender then
					local live = now()
					props.open_sender(from.chat, if live then live.title else from.title)
				elseif props.open_link then
					props.open_link(`zeron-chat:{from.chat}`)
				end
			end
			return ui.row { w_full = true, justify = "end", ui.button(line) }
		end
		return ui.row { w_full = true, justify = "end", ui.row(line) }
	end, { w_full = true, pb = ui.raw(6, "Zeron's USER_CAPTION_GAP") })
end

-- A reply's row: its tools, its text, and while it waits for either, that it
-- is working.
-- Where a turn's working words start: from its message's id, so the word
-- shown is the same however often the line is made (a reload, a row
-- scrolled back), as Zeron seeds it per chat (transcript.rs
-- `flavour_seed`).
local function seed_of(id: string): number
	local n = 0
	for i = 1, #id do
		n = (n * 31 + string.byte(id, i)) % 9973
	end
	return n
end

-- Under a person's newest prompt while its reply hasn't begun, in that
-- reply's place: "Sending…" while it is on its way, then the turn's working
-- word and clock, as Zeron's working trailer (transcript.rs); not
-- delivered, a line to send it again. The reply's own row takes over once
-- it comes.
local function waiting_line(row: State<Message>, on_retry: ((id: string) -> ())?, turn_state: ((m: Message, sending: boolean) -> TurnLine?)?): Node
	local function said(): TurnLine?
		local m = row:get()
		return if turn_state and m.waiting and m.waiting ~= "failed" then turn_state(m, m.waiting == "sending") else nil
	end
	return ui.presence(function(): string?
		local w = row:get().waiting
		if w == "failed" then
			return "failed"
		end
		local s = if w then said() else nil
		return if s and s.step then "step" elseif s then "said" elseif w then "waiting" else nil
	end, function(kind: string): Node
		if kind == "said" then
			return ui.row { w_full = true, mt = "$space.4", turn_line(said) }
		end
		-- Its Cloud machine coming up for it: the machine's first setup step,
		-- a tool row as the reply's own are, in the reply's place, under the
		-- id the machine's own step comes with (which takes over from it).
		if kind == "step" then
			local id = row:peek().id
			return ui.column {
				w_full = true,
				mt = "$space.4",
				Activity {
					key = `{id}.cloud-setup:g:{id}.cloud-machine`,
					live = function(): boolean
						return true
					end,
					tools = function(): { Tool }
						local s = said()
						return {
							{
								id = `{id}.cloud-machine`,
								kind = "cloud",
								verb = if s then s.text else "",
							} :: Tool,
						}
					end,
					summary = function(): string?
						local s = said()
						return s and s.summary
					end,
				},
			}
		end
		if kind == "failed" then
			return ui.button {
				label = locale.t("ds.transcript.not_delivered"),
				self = "start",
				mt = "$space.4",
				p = 0,
				bg = "#00000000",
				active = { bg = "#00000000" },
				hover = { bg = "#00000000", underline = true },
				cursor = "pointer",
				selectable = false,
				text_size = "$font.size.sm",
				color = "$danger",
				on_click = function()
					if on_retry then
						on_retry(row:peek().id)
					end
				end,
				on_press = function() end,
				on_release = function() end,
				locale.t("ds.transcript.not_delivered"),
			}
		end
		return ui.row {
			w_full = true,
			mt = "$space.4",
			Working {
				since = function(): number
					return row:get().since or ui.now() / 1000
				end,
				sending = function(): boolean
					return row:get().waiting == "sending"
				end,
				seed = seed_of(row:peek().id),
			},
		}
	end, { w_full = true, exit = { opacity = 0, duration = 120 } })
end

-- A reply's blocks in order, each its own row (keyed by its id): a group
-- of tools stays the same node as tools join it and the reply streams on,
-- and a new group or text joins after the last. A compact reply's work is
-- one group whose fold holds its blocks, laid out the same way.
-- Each reply's top-level blocks as they are laid out (a paragraph, a list,
-- a code block), by message: where a reader is, finer than a whole reply,
-- for keeping that place as the width changes and lines rewrap.
local BLOCKS: { [string]: { [Ref]: boolean } } = {}
local function track(id: string): (block: Ref) -> () -> ()
	return function(block: Ref): () -> ()
		local set = BLOCKS[id]
		if not set then
			set = {}
			BLOCKS[id] = set
		end
		set[block] = true
		return function()
			local now = BLOCKS[id]
			if now then
				now[block] = nil
				if next(now) == nil then
					BLOCKS[id] = nil
				end
			end
		end
	end
end

-- Where a reply's words are windowed: the transcript's list and its scroll
-- (Markdown's `window`), so a long reply builds only what's near the view.
-- (And the pictures its replies show: Props' `pictures`.)
-- (`at_end`: whether the list rests at its end, the end then what it holds.)
-- (`step`: said as a setup step's tool row, `summary` its group's line.)
export type TurnLine = { text: string, spinner: boolean?, step: string?, summary: string? }

-- The line said in a working word's place: a ring while it's still on its
-- way, then its words, in the working word's quiet ink.
local function turn_line(line: () -> TurnLine?): Node
	return ui.row {
		gap = "$space.2",
		items = "center",
		role = "status",
		ui.show(function(): boolean
			local l = line()
			return l ~= nil and l.spinner == true
		end, status.StatusIcon { kind = "working", size = 14 }),
		ui.text {
			function(): string
				local l = line()
				return if l then l.text else ""
			end,
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			color = "$text.tertiary",
		},
	}
end

type View = {
	turn_state: ((m: Message, sending: boolean) -> TurnLine?)?,
	view: Ref,
	offset: State<number>,
	at_end: State<boolean>?,
	-- The list's row holding the reply (Markdown's `window`).
	row: Ref?,
	pictures: ReplyPicture.Pictures?,
	-- Where the pictures an agent showed are read from (Props'
	-- `sent_pictures`: they're files on the chat's host, as sent ones are).
	shown_pictures: ReplyPicture.Pictures?,
	open: ((target: string) -> ())?,
	link_menu: ((target: string) -> { Markdown.LinkEntry }?)?,
	open_subagent: ((sub: any) -> ())?,
	-- A message text's find (its key: `<id>:u`, `<id>:<block>`, `<id>:r`).
	find: ((key: string) -> any)?,
}

-- A reply's thought being written, for its row (ds/activity's `thought`): the
-- draft's words while it's the one the reply is at.
local function thought_of(m: Message, now: () -> Message, draft: State<Draft>?): Activity.Writing
	return {
		words = function(id: string): string?
			if not (draft and now().streaming) then
				return nil
			end
			local d = draft:get()
			return if d.id == m.id and d.thought == id then d.thought_text else nil
		end,
		seeded = function(): boolean
			return ui.now() - opened_at < 300
		end,
	}
end

local function blocks_of(list: () -> { Block }, m: Message, now: () -> Message, draft: State<Draft>?, view: View?): Node
	local shown = ui.computed(list)
	return ui.each(shown, function(b: Block, _i: number, brow: State<Block>): Node
		if b.kind == "tools" or b.kind == "work" then
			return Activity {
				key = `{m.id}:{b.id}`,
				open_subagent = if view then view.open_subagent else nil,
				-- (A long group builds only what's near the view, as a long
				-- reply does.)
				window = if view
					then { key = `{m.id}:{b.id}`, view = view.view, offset = view.offset, at_end = view.at_end, row = view.row }
					else nil,
				thought = thought_of(m, now, draft),
				tools = function()
					return brow:get().tools or {}
				end,
				live = function()
					return brow:get().live == true
				end,
				summary = function()
					return brow:get().summary
				end,
				body = if b.kind == "work"
					then function(): Node
						return ui.column {
							w_full = true,
							pt = "$space.2",
							blocks_of(function(): { Block }
								return brow:get().blocks or {}
							end, m, now, draft, view),
						}
					end
					else nil,
			}
		end
		-- An agent's visual: built once, followed by the host (src/visuals.rs).
		if b.kind == "visual" then
			return Visual { id = b.id, path = b.path, source = b.source, title = b.title, device = b.device }
		end
		-- Pictures an agent showed: one alone, or several on a tray, among
		-- the reply's pictures in the lightbox. More arriving while it works
		-- join the same tray (keyed by their parts, nothing built again).
		if b.kind == "images" and view and view.shown_pictures then
			local shown = view.shown_pictures :: ReplyPicture.Pictures
			local group = `reply:{m.id}`
			local function entries(): { ReplyPicture.Entry }
				local out = {}
				for _, it in brow:get().images or {} do
					table.insert(out, { src = it.path, alt = it.name or "" })
				end
				return out
			end
			return ui.presence(function(): string
				return if #(brow:get().images or {}) > 1 then "run" else "one"
			end, function(kind: string): Node
				if kind == "run" then
					return ReplyPicture.Run { pictures = shown, items = entries, group = group, set = entries }
				end
				local first = entries()[1] or { src = "", alt = "" }
				return ReplyPicture.Single { pictures = shown, src = first.src, alt = first.alt, group = group, set = entries }
			end, { w_full = true, min_w = 0 })
		end
		-- A display tool (`show_visual`, `show_image`) that failed: no tool
		-- row, only a quiet line saying why (src/chat.rs DISPLAY_TOOLS).
		if b.kind == "display_failed" then
			-- (ds/reply_picture's caption line: which it was, in a few words;
			-- why, in plain words, on hover, never the script's own text.)
			local tool = (b :: any).tool
			return ReplyPicture.Line {
				icon = if tool == "visual" then "widget" else "gallery",
				words = locale.s(if tool == "visual"
					then "ds.transcript.visual_failed"
					elseif tool == "image" then "ds.transcript.image_failed"
					else "ds.transcript.display_failed"),
				reason = ReplyPicture.plain(b.text),
			}
		end
		-- Its words: the draft's while this is the text being written.
		local function words(): string
			local n = now()
			if n.streaming and draft then
				local d = draft:get()
				if d.id == m.id and d.part == b.id and d.part_text then
					return d.part_text
				end
			end
			return brow:get().text or ""
		end
		return Markdown {
			text = words,
			-- Parsed incrementally only while it can grow.
			key = if m.streaming then `reply:{m.id}:{b.id}` else nil,
			veil = m.streaming == true,
			veil_seed = ui.now() - opened_at < 300,
			track = track(m.id),
			window = if view
				then {
					key = `{m.id}:{b.id}`,
					view = view.view,
					offset = view.offset,
					live = function(): boolean
						return now().streaming == true
					end,
					at_end = view.at_end,
					row = view.row,
				}
				else nil,
			pictures = if view then view.pictures else nil,
			open = if view then view.open else nil,
			link_menu = if view then view.link_menu else nil,
			link_chip = if view then view.link_chip else nil,
			find = if view and view.find then view.find(`{m.id}:{b.id}`) else nil,
			group = `reply:{m.id}`,
		}
	end, {
		key = function(b: Block): string
			return b.id
		end,
		w_full = true,
		gap = "$space.3",
	})
end

local function reply(
	m: Message,
	row: State<Message>,
	draft: State<Draft>?,
	hovered: () -> boolean,
	compact: (() -> boolean)?,
	view: View?
): Node
	local function now(): Message
		return row:get()
	end
	-- Its text: the draft's while it's being written, then its own.
	local function text(): string
		local n = now()
		if n.streaming and draft then
			local d = draft:get()
			if d.id == m.id then
				return d.text
			end
		end
		return n.text
	end
	return ui.column {
		role = "article",
		label = locale.t("ds.transcript.reply"),
		hook = "chat.message.reply",
		class = "message message-assistant",
		provide = { message = m.id },
		w_full = true,
		gap = "$space.3",
		if m.blocks ~= nil
			then blocks_of(function(): { Block }
				local n = now()
				if compact and compact() and n.compact then
					return n.compact
				end
				return n.blocks or {}
			end, m, now, draft, view)
			else ui.show(
			function()
				local tools = now().tools
				return tools ~= nil and #tools > 0
			end,
			Activity {
				key = m.id,
				open_subagent = if view then view.open_subagent else nil,
				thought = thought_of(m, now, draft),
				tools = function()
					return now().tools or {}
				end,
				live = function()
					return now().streaming == true and text() == ""
				end,
				worked = function()
					return now().worked
				end,
			}
		),
		if m.blocks ~= nil then nil else Markdown {
			text = text,
			-- Parsed incrementally only while it can grow.
			key = if m.streaming then `reply:{m.id}` else nil,
			veil = m.streaming == true,
			veil_seed = ui.now() - opened_at < 300,
			pictures = if view then view.pictures else nil,
			open = if view then view.open else nil,
			link_menu = if view then view.link_menu else nil,
			link_chip = if view then view.link_chip else nil,
			find = if view and view.find then view.find(`{m.id}:r`) else nil,
			group = `reply:{m.id}`,
			track = track(m.id),
		},
		-- Its foot, one lane: while the turn runs, that it's working and how
		-- long; once it settles, its time and Copy on hover. The lane keeps
		-- its height through the change, so the end of a turn moves nothing.
		ui.overlay {
			w_full = true,
			h = LANE,
			mt = ui.raw(-12, "the lane sits under the text, not a block gap below it"),
			ui.presence(function()
				if not now().streaming then
					return nil
				end
				-- (No turn to speak of: why, in the working word's place.)
				local said = view and view.turn_state and view.turn_state(now(), false)
				return if said then "said" else "working"
			end, function(kind: string)
				if kind == "said" then
					return ui.column {
						mt = ui.raw(8, "the block gap less the indicator's own line spacing"),
						turn_line(function(): TurnLine?
							return view and view.turn_state and view.turn_state(now(), false)
						end),
					}
				end
				return ui.column {
					-- (Where it stood as a slot of its own: the block gap less
					-- the 4 its own line height already gives.)
					mt = ui.raw(8, "the block gap less the indicator's own line spacing"),
					Working {
						since = function(): number
							return now().started or ui.now() / 1000
						end,
						seed = seed_of(m.id),
					},
				}
			end, { exit = { opacity = 0, duration = 160 } }),
			strip(m, row, hovered),
		},
	}
end

-- The end: its last rows measured as they land (`scroll_to_row`), then the
-- offset past the end, which clamps to it, the list's foot included; a list
-- at its end follows again.
local PAST_END = 1e9
-- A send's glide: how long the prompt takes from the composer to the view's
-- top (Zeron's send, about this), and how far clear of the end the list
-- steps first, so it doesn't follow the new rows to the end at once.
local GLIDE_MS = 380
local GLIDE_CLEAR = 1
-- How long a steer's glide waits for the host's copy of its prompt.
local LANDING_MS = 2000
local function to_end(list: Ref, offset: State<number>, rows: number)
	local el = list:peek()
	if el and rows > 0 then
		el:scroll_to_row(rows, { align = "end" })
	end
	offset:set(PAST_END)
end
-- The prompts sent from this window (kept across a reload): only a turn
-- sent here holds the runway under its prompt, as Zeron reserves the view
-- for a turn sent locally (transcript.rs `OwnTurnAnchor`); a conversation
-- opened as it stands shows its end.
local SENT = ui.state("transcript_sent_here", {} :: { [string]: boolean })
local function sent_here(id: string): boolean
	return SENT:get()[id] == true
end
local function mark_sent(id: string)
	if SENT:peek()[id] then
		return
	end
	local all = table.clone(SENT:peek())
	all[id] = true
	SENT:set(all)
end
-- The transcript shown last, for `latest` and `send`.
-- (`landing`: the id of a prompt the host adds after `push`, a steer's.)
local shown: (() -> ())? = nil
local sending: ((push: () -> (), landing: string?) -> ())? = nil
-- A named transcript's (`name`: a side chat's), by its name.
local named_shown: { [string]: () -> () } = {}
local named_sending: { [string]: (push: () -> (), landing: string?) -> () } = {}

local function Transcript(props: Props): Node
	opened_at = ui.now()
	local messages = props.messages
	-- (Its own kept states, by its name: a side chat's beside the chat
	-- page's never shares the chat page's glide, runway or hover.)
	local kept = if props.name then `.{props.name}` else ""
	local function foot(): number
		return if props.foot then props.foot() else 0
	end
	local offset = props.offset or ui.state(`transcript_offset{kept}`, 0)
	local list = props.ref or ui.ref()
	-- Whether the list rests at its end, as the list itself says (its
	-- `at_end`, written as it changes). (It was a mark at the last row's
	-- foot reporting when it showed; a virtualized row going out of view
	-- didn't always say so, and the way back never appeared.)
	local at_end = ui.state(`transcript_at_end{kept}`, true)
	-- A find's: each text's marks, and its current match brought into view
	-- (its message scrolled to, if it's away; then the block holding it).
	local find_for: ((key: string) -> any)? = nil
	local find = props.find
	if find then
		local f = find :: TranscriptFind
		local function reveal(block: Ref)
			local el, view = block:peek(), list:peek()
			if not el or not view or el.y == nil or view.y == nil or el.height == nil or view.height == nil then
				return
			end
			local top, h = view.y :: number, view.height :: number
			local y, bh = el.y :: number, el.height :: number
			-- In view (clear of the composer's room at the foot): stays.
			local foot = if props.foot then props.foot() else 0
			if y >= top + 8 and y + math.min(bh, 120) <= top + h - foot - 8 then
				return
			end
			offset:set(math.max(0, offset:peek() + (y - top) - h / 3))
		end
		find_for = function(key: string): any
			return { key = key, query = f.query, current = f.current, reveal = reveal }
		end
	end
	-- Whether the transcript this one replaces rested at its end (kept
	-- state: a reload's rebuild knows it; `keep` brings a view back only if
	-- it didn't). Then a new one starts at its end.
	local was_at_end = if props.was_at_end ~= nil then props.was_at_end else at_end:peek()
	-- A new one starts at its end; one made again where the last was
	-- scrolled up (a reload, `keep` holding its place) stays away from it,
	-- its way back to the latest still out. (The list writes the state only
	-- as it changes, so it must start right.)
	local restoring = props.keep ~= nil and props.keep:peek() ~= nil and not was_at_end
	-- (Away from it, putting itself back: a launch's state, at its end by
	-- default, would hold it at the end through the window's first resizes.)
	at_end:set(not restoring)
	-- Putting itself back where it was (`keep`, after a reload): the list
	-- can say it's at its end a moment, its rows above not yet measured;
	-- the way back to the latest stays out meanwhile rather than blinking.
	local placing = ui.use_state(restoring)
	-- Where the reader is, as last noted (`remember`): the message at the
	-- view's top, how far above the top it starts, and its place in the
	-- list then. Rows coming or going above it (a chat's history arriving
	-- after its newest rows, as the engine sends a chat it opens) move it
	-- in the list but not on the screen: the view goes back to it.
	local here: { id: string, dy: number, index: number }? = nil
	-- A place to go back to whose message hasn't come yet (a chat opened
	-- with its newest rows first, the rest on their way): gone back to as
	-- it comes, unless the reader goes somewhere first.
	local waiting: Anchor? = nil
	-- (The last message alone: reading the whole list would copy every
	-- message into the script on each word a reply writes.)
	local writing = ui.computed(function(): boolean
		local n = messages:len()
		local last = if n > 0 then messages:at(n) else nil
		return last ~= nil and last.streaming == true
	end)
	local count = ui.computed(function(): number
		return messages:len()
	end)

	-- How long the conversation is, for its scrollbar: the rail over the
	-- list, not the list's own bar, which its edge fades would fade with the
	-- rows. As the list itself knows it: each row's height once it has been
	-- measured (kept as it scrolls out of view), the estimate until then.
	local sizes: { [string]: number } = {}
	local known, seen_n = 0, 0
	local sized = ui.state(`transcript_sized{kept}`, 0)
	local function size_row(id: string, h: number)
		local was = sizes[id]
		if was == h then
			return
		end
		if was then
			known += h - was
		else
			known += h
			seen_n += 1
		end
		sizes[id] = h
		sized:set(sized:peek() + 1)
	end
	local function extent(): number
		sized:get()
		return known + math.max(0, count:get() - seen_n) * ESTIMATE
	end

	-- The runway. The last reply's row (its foot's room included) is always
	-- at least as tall as the view less its prompt's row, so those two rows
	-- fill the view exactly: at the end the last prompt's row starts at the
	-- view's top, nothing before it showing. A reply being written
	-- grows into that room with the view still, rather than scrolling a line
	-- at a time. `pin`: the last prompt's row and its height.
	local pin = ui.state(`transcript_pin{kept}`, { id = "", h = 0 })
	-- Each mounted reply's runway (how tall it must be) and its own height,
	-- by id: the last one's room above what it fills is what a send takes
	-- back.
	local runways: { [string]: { need: () -> number, own: () -> number } } = {}
	-- Each mounted prompt's row, by id: where a send's glide lands.
	local tops: { [string]: Ref } = {}

	local function show_end()
		waiting = nil
		to_end(list, offset, count:peek())
	end
	if props.name then
		named_shown[props.name] = show_end
	else
		shown = show_end
	end
	-- A send, as a person sees it: their prompt rises from the foot to the
	-- top of the view, its reply's room opening under it, in one glide.
	-- While it glides the old last reply keeps its runway (`held`), so the
	-- content above the new prompt doesn't change under the glide; once it
	-- lands, that room above the view is let go, which the list absorbs
	-- without moving what shows. The glide starts a point off the end, so
	-- the list takes it as a scroll of its own and doesn't jump to the new
	-- end first (its `follow`); landing at the end, it follows again. Away
	-- from the end, a send jumps there (a glide across a long way would
	-- build every row on it).
	local held = ui.state(`transcript_held{kept}`, { id = "", h = 0 })
	-- A send's glide under way (from its step off the end to its landing).
	local gliding = ui.state(`transcript_gliding{kept}`, false)
	local glide: any = nil
	-- (Each send's turn: a later one takes over from one still on its way.)
	local glides = 0
	local glide_from: (push: () -> (), from: number, last: Message, r: any, landing: string?) -> ()
	local function send_here(push: () -> (), landing: string?)
		waiting = nil
		local n = count:peek()
		local from = offset:peek()
		local last = if n > 0 then messages:at(n) else nil
		local r = last and runways[last.id]
		if not at_end:peek() or not r or not last then
			push()
			to_end(list, offset, count:peek())
			return
		end
		if from < PAST_END / 2 then
			glide_from(push, from, last, r, landing)
			return
		end
		-- At the end by the sentinel alone (as a conversation opens): the
		-- list scrolled to its last row says where that is, a frame on.
		local el = list:peek()
		if el then
			el:scroll_to_row(n, { align = "end" })
		end
		ui.after(16, function()
			local real = offset:peek()
			if real >= PAST_END / 2 then
				push()
				to_end(list, offset, count:peek())
			else
				glide_from(push, real, last :: Message, r, landing)
			end
		end)
	end
	if props.name then
		named_sending[props.name] = send_here
	else
		sending = send_here
	end
	glide_from = function(push: () -> (), from: number, last: Message, r: any, landing: string?)
		if glide then
			glide:pause()
		end
		glides += 1
		local turn = glides
		-- First a step clear of the end (the old last reply holding its
		-- runway meanwhile, so nothing above moves): the list then no
		-- longer rests at its end, and doesn't follow the new rows there.
		-- Added at its end, it would: the prompt, holding the view's height
		-- of runway, would be born at the top, with no glide (zgx's `follow`
		-- is fixed as the list is made, so it can't be held off otherwise).
		local start = math.max(0, from - GLIDE_CLEAR)
		ui.batch(function()
			gliding:set(true)
			held:set { id = last.id, h = r.need() }
			offset:set(start)
		end)
		-- Nothing to glide to: the old reply lets go of its room, and the
		-- view goes to the end.
		local function let_go()
			ui.batch(function()
				held:set { id = "", h = 0 }
				gliding:set(false)
			end)
			to_end(list, offset, count:peek())
		end
		-- A frame later the new rows are laid out under the old end: the
		-- glide goes to where the prompt's row really starts, the list's
		-- end now its runway is under it. It lands on that end exactly,
		-- so the list follows a reply that outgrows the view; a scroll
		-- taken meanwhile stays where it went.
		local function rise(prompt: Message?)
			ui.after(16, function()
				local row_el = prompt and tops[prompt.id] and tops[prompt.id]:peek()
				local list_el = list:peek()
				if not row_el or not list_el or row_el.y == nil or list_el.y == nil then
					let_go()
					return
				end
				local target = offset:peek() + (row_el.y :: number) - (list_el.y :: number)
				glide = anime.animate(offset, {
					value = { offset:peek(), target },
					duration = motion.ms(GLIDE_MS),
					ease = "outQuad",
					onComplete = function()
						-- Landed: the old reply lets go of the room it held
						-- (above the view, so nothing seen moves), and the
						-- offset says where the list now rests, a real place
						-- (the next send glides from it), less that room.
						local h = held:peek()
						local old = runways[h.id]
						local excess = if old then math.max(0, h.h - old.own()) else 0
						ui.batch(function()
							held:set { id = "", h = 0 }
							gliding:set(false)
							if math.abs(offset:peek() - target) < 1 then
								offset:set(if old then math.max(0, target - excess) else PAST_END)
							end
						end)
					end,
				})
			end)
		end
		ui.after(16, function()
			push()
			-- A steer's prompt: the host adds it (its copy, under the queued
			-- message's id) a moment after it's handed over, the view held
			-- clear of the end meanwhile; it rises once it's there. One that
			-- doesn't come (it didn't go, or waits for the turn's end) lets
			-- the view go back to the end; it holds its runway when it comes.
			if landing then
				local waited = 0
				local function look()
					if glides ~= turn then
						return
					end
					local at = messages:index_of("id", landing)
					if at then
						rise(messages:at(at))
					elseif waited >= LANDING_MS then
						let_go()
					else
						waited += 16
						ui.after(16, look)
					end
				end
				look()
				return
			end
			-- The prompt sent: the newest of the person's.
			local prompt: Message? = nil
			for i = count:peek(), 1, -1 do
				local m = messages:at(i)
				if m and m.role == "user" then
					prompt = m
					break
				end
			end
			rise(prompt)
		end)
	end
	local away = ui.state(`transcript_away{kept}`, false)
	-- The way back's width, as last laid out; said while it shows.
	local jump_w: number? = nil
	ui.effect(function()
		local shown = away:get()
		if props.on_jump_width then
			props.on_jump_width(if shown then jump_w else nil)
		end
	end)
	-- (Gone with it: a pill beside it no longer moves aside for it.)
	if props.on_jump_width then
		local say = props.on_jump_width
		ui.on_cleanup(function()
			say(nil)
		end)
	end
	-- Each mounted row, by id: what a place in the window is part of.
	local rows: { [string]: { outer: Ref, inner: Ref, index: State<number>, m: Message } } = {}
	-- A find's current match in a message that isn't laid out (the list
	-- virtualizes): that message scrolled to, its block revealed once it's
	-- built. (One that is laid out is left to the reveal: a reply taller
	-- than the view would otherwise jump to its start.)
	if find then
		local f = find :: TranscriptFind
		ui.effect(function()
			local c = f.current()
			if not c then
				return
			end
			local m = messages:at(c.row)
			local row = if m then rows[m.id] else nil
			if row and row.outer:peek() then
				return
			end
			ui.after(0, function()
				local el = list:peek()
				if el then
					el:scroll_to_row(c.row, { align = "center" })
				end
			end)
		end)
	end
	-- The row a view made again goes back to, and how far above the top it
	-- stood: set as the list mounts, put right as soon as that row is laid
	-- out (its own `on_resize`, the frame it lands), not on the clock.
	local returning: { id: string, dy: number }? = nil
	-- The view moved by however far message `at.id` stands from `at.dy` below
	-- its top, where it's laid out. False if it isn't laid out.
	local function hold_on(at: { id: string, dy: number }): boolean
		local r = rows[at.id]
		local row_el = r and r.outer:peek()
		local list_el = list:peek()
		if not row_el or not list_el or row_el.y == nil or list_el.y == nil then
			return false
		end
		local off = ((row_el.y :: number) - (list_el.y :: number)) - at.dy
		if math.abs(off) >= 0.5 then
			offset:set(math.max(0, offset:peek() + off))
		end
		return true
	end
	local function put_back(id: string): boolean
		local back = returning
		if not back or back.id ~= id then
			return false
		end
		return hold_on(back)
	end
	-- The view put back on message `at.id`, `at.dy` from its top as it was.
	-- Rows above it not measured yet stand at an estimate, so the row can
	-- land off its place: as it lays out (`put_back`), a frame on, and again
	-- as the rows it brought into view measure, where it really is says how
	-- far, and the view moves by that. Then the way back to the latest
	-- follows the list again. (Each armed here: a timer armed in another's
	-- callback can run late.) False if the message isn't there.
	local function go_back(at: Anchor): boolean
		local index = messages:index_of("id", at.id)
		local el = list:peek()
		if not index or not el then
			return false
		end
		-- Not laid out yet (a launch's first frames): it has no rows to put
		-- the message among. Held until it is (its `on_resize`), as a place
		-- still to come back to is.
		if not el.height or el.height <= 0 then
			waiting = { id = at.id, dy = at.dy }
			return true
		end
		here = { id = at.id, dy = at.dy, index = index }
		el:scroll_to_row(index, { align = "start" })
		local back = { id = at.id, dy = at.dy }
		returning = back
		local function correct()
			if returning ~= back then
				return
			end
			put_back(at.id)
		end
		ui.after(16, correct)
		ui.after(48, correct)
		ui.after(80, function()
			if returning == back then
				placing:set(false)
				returning = nil
			end
		end)
		return true
	end
	-- Rows coming or going above the reader's place (or the message a place
	-- still to come back to waits for, arriving): the view goes back to it.
	-- (At the end it follows the list; a send's glide owns the offset.)
	ui.effect(function()
		local target: any = waiting or here
		if not target then
			count:get()
			return
		end
		local index = messages:index_of("id", target.id)
		if not index then
			return
		end
		if waiting then
			local w = waiting :: Anchor
			waiting = nil
			go_back(w)
		elseif index ~= target.index and not at_end:peek() and not gliding:peek() then
			-- The list keeps the rows in view where they are as rows come or
			-- go above them (it anchors on its rows' keys), so the view stays,
			-- and is only moved by however far the message stands off its
			-- place once laid out (at the list's very top nothing anchors).
			-- One no longer laid out is gone back to. (Scrolled to it first,
			-- by rows above it not measured yet, the view showed older rows
			-- for the frames until the correction: a chat's history arriving
			-- after its newest rows, as one is opened.)
			local at = { id = target.id, dy = target.dy }
			here = { id = target.id, dy = target.dy, index = index }
			ui.after(0, function()
				if here == nil or here.id ~= at.id or at_end:peek() or gliding:peek() then
					return
				end
				if not hold_on(at) then
					go_back(at)
				end
			end)
		end
	end)
	-- The message under the pointer, its lane shown ("" for none), by id:
	-- each row's `on_hover` says so as the pointer crosses into it, and as
	-- content scrolls under a still pointer (zgx hovers again what the
	-- resting pointer is over once layout moves it): the lane goes with the
	-- pointer, never with a row scrolled away, and a scroll's frames run
	-- nothing here.
	local hover = ui.state(`transcript_hover{kept}`, "")
	local function set_hover(id: string)
		if hover:peek() ~= id then
			trace("hover", id)
			hover:set(id)
		end
	end
	-- Where it is, written once a scroll settles (at its end, nowhere: it
	-- opens there again), for a view made again to come back to.
	local keep = props.keep
	-- How wide the conversation runs: the caller's (followed), else the
	-- theme's measure.
	local function measure(): number
		local given: any = props.measure
		if type(given) == "function" then
			local follow: () -> number = given
			return follow()
		elseif type(given) == "number" then
			return given
		end
		return ui.theme.get("zeron.prose.measure") :: number
	end
	local function remember()
		-- (Not yet back where it was: nothing new to say.)
		if waiting then
			return
		end
		local el = list:peek()
		-- (Not laid out yet, a view just made: nothing to say.)
		if not el or not el.y or not el.height or el.height <= 0 then
			return
		end
		local best: any = nil
		if not at_end:peek() then
			for id, r in rows do
				local row = r.outer:peek()
				if row and row.y and row.height and row.y + row.height > el.y and (not best or row.y < best.y) then
					best = { id = id, y = row.y, index = r.index:peek() }
				end
			end
		end
		here = if best then { id = best.id, dy = best.y - el.y, index = best.index } else nil
		if keep then
			keep:set(if best then { id = best.id, dy = best.y - el.y } else nil)
		end
	end
	-- The reading place: the block (a paragraph, a list, a code block)
	-- crossing the view's top, and how far down its message it stands. As
	-- the width changes the list holds that message's top still (its rows
	-- above the view measured again without moving it), but a tall reply's
	-- lines above the block rewrap inside it: the view moves by however far
	-- the block moved in its message, so the block stays put, as Zeron's
	-- rows (each a block) do.
	local reading: { row: string, block: Ref, within: number, view: number? }? = nil
	local last_w: number? = nil
	local last_h: number? = nil
	-- At the end before the view's size changed: it stays at the end through
	-- the change (a window shrunk shorter or narrower, a panel easing open),
	-- whatever the list says of itself mid-change. Set as the list reaches
	-- its end; let go only when a scroll rests away from it with no change
	-- of size under way.
	local end_held = at_end:peek()
	ui.effect(function()
		-- (Not while it puts itself back where it was: the list rests at its
		-- end a moment first, before the place it goes back to is laid out.)
		if at_end:get() and not placing:get() then
			end_held = true
		end
	end)
	-- When a person last scrolled the list (a wheel or trackpad over its
	-- rows, heard before the list takes it): what tells their scroll from
	-- the list moving itself as rows measure.
	local wheeled_at = -math.huge
	local function wheeled(): boolean?
		wheeled_at = ui.now()
		-- (Taken somewhere of their own: a place still to come back to is
		-- let go.)
		waiting = nil
		return nil
	end
	-- When focus last came to something in the list (Tab, ⇧Tab to a row's
	-- control): taken there, the reader is somewhere of their own.
	local focused_at = -math.huge
	-- While the width is changing (and a moment after): the reading row
	-- puts the place back as its lines rewrap, the frame they do.
	local resizing_until = 0
	local function find_reading()
		reading = nil
		local el = list:peek()
		if not el or not el.y or not el.height or el.height <= 0 then
			return
		end
		local top = el.y :: number
		-- The rows in view, top first.
		local shown: { { id: string, el: any } } = {}
		for id, r in rows do
			local row = r.outer:peek()
			if row and row.y and row.height and row.y + row.height > top and row.y < top + el.height then
				table.insert(shown, { id = id, el = row })
			end
		end
		table.sort(shown, function(a, b)
			return a.el.y < b.el.y
		end)
		-- The first block starting in view (what is read, whole lines of it
		-- in sight), in whichever row: a row of no blocks (a prompt) is
		-- itself one. Else, nothing starting in view, the block the view's
		-- top crosses.
		local crossing: { row: string, block: Ref, at: number, row_y: number }? = nil
		for _, r in shown do
			local set = BLOCKS[r.id]
			if not set then
				if r.el.y >= top then
					reading = { row = r.id, block = rows[r.id].outer, within = 0, view = r.el.y - top }
					return
				end
			else
				local pick: Ref? = nil
				local at: number? = nil
				for b in set do
					local be = b:peek()
					if be and be.y and be.height and be.y + be.height > top then
						if be.y >= top and (not at or be.y < at) then
							pick, at = b, be.y
						elseif be.y < top and not crossing then
							crossing = { row = r.id, block = b, at = be.y, row_y = r.el.y }
						end
					end
				end
				if pick and at then
					reading = { row = r.id, block = pick, within = at - r.el.y, view = at - top }
					return
				end
			end
		end
		if crossing then
			reading = { row = crossing.row, block = crossing.block, within = crossing.at - crossing.row_y, view = crossing.at - top }
		end
	end
	local function keep_reading()
		local r = reading
		if not r or at_end:peek() then
			return
		end
		local row = rows[r.row]
		local row_el = row and row.outer:peek()
		local be = r.block:peek()
		if not row_el or not be or row_el.y == nil or be.y == nil then
			return
		end
		local within = (be.y :: number) - (row_el.y :: number)
		local d = within - r.within
		if math.abs(d) >= 0.5 then
			offset:set(math.max(0, offset:peek() + d))
		end
		r.within = within
	end
	-- Once the offset has rested 150 ms: where it is remembered, the reading
	-- place found again, the end let go if it was scrolled away from. Told
	-- by zgx's own timer, begun again at each change (`ui.debounced`), so a
	-- scroll's frames run nothing here; at every rest (`every`), an offset
	-- back where it last rested included (a view opened at its end and
	-- scrolled to its top), which would otherwise change nothing there and
	-- leave the place unwritten.
	local function rested()
		remember()
		find_reading()
		if not at_end:peek() and ui.now() >= resizing_until then
			end_held = false
		end
	end
	local rest = ui.debounced(offset, 150, { every = true })
	-- (A view just made: its place told once it has stood 150 ms.)
	ui.after(150, rested)
	local first = true
	ui.effect(function()
		rest:get()
		if first then
			first = false
			return
		end
		rested()
	end)
	-- The selection the host read, and where its action stands: `y` from
	-- the view's top, and its right edge, or its left when it stands above.
	local spot = ui.state(`transcript_spot{kept}`, nil :: any)
	local read = SelectionMenu.read
	local function row_at(y: number): any
		for _, r in rows do
			local el = r.outer:peek()
			if el and el.y and el.height and y >= el.y and y < el.y + el.height then
				return r
			end
		end
		return nil
	end
	-- Who said it: one name, two, or how many messages it runs across. With
	-- an end scrolled out of reach, there's no telling how many.
	local function who(s: any): string
		local first = if s.top then row_at(s.top.y + s.top.h / 2) else nil
		local final = if s.bottom then row_at(s.bottom.y + s.bottom.h / 2) else nil
		local function name(r: any): string
			return if r.m.role == "user" then locale.s("ds.transcript.you") else r.m.model or locale.s("ds.transcript.agent")
		end
		if not first or not final then
			return locale.s("ds.transcript.several_messages")
		end
		if first == final then
			return name(first)
		end
		local span = math.abs(final.index:peek() - first.index:peek()) + 1
		if span == 2 and name(first) ~= name(final) then
			return locale.s("ds.transcript.two_names", { first = name(first), final = name(final) })
		end
		return locale.s("ds.transcript.n_messages", { n = tostring(span) })
	end
	-- Where the action stands for a selection: under its last line, ending
	-- with it; else over its first, starting with it; else at the view's foot.
	local function place(s: any): any
		local el = list:peek()
		if not el or not el.x or not el.y or not el.width or not el.height then
			return nil
		end
		local b, t = s.bottom or s.top, s.top or s.bottom
		if not b or not t then
			return nil
		end
		local floor = el.height - foot() - ACTION - (if away:peek() then FOOT else MARGIN)
		local below = b.y + b.h + CLEAR - el.y
		if below <= floor then
			return { y = below, right = b.x + b.w - el.x }
		end
		local above = t.y - CLEAR - ACTION - el.y
		if above >= MARGIN then
			return { y = above, left = t.x - el.x }
		end
		return { y = floor, right = b.x + b.w - el.x }
	end
	-- What a selection quotes: from its first word, whoever said it.
	local function quote_of(s: any): Quote.Quote
		local first = if s.top then row_at(s.top.y + s.top.h / 2) else nil
		-- (The message as it is now: a reply written since its row was made
		-- has its words and its time.)
		local said: Message? = if first then first.state:peek() else nil
		local lead = if said then Quote.lead(s.text, said.text) else ""
		-- Where it was said: its message's time, when it lies in one.
		local final = if s.bottom then row_at(s.bottom.y + s.bottom.h / 2) else nil
		local from = if said and first == final then said.at else nil
		return { text = lead .. s.text, who = who(s), from = from }
	end
	local function settle()
		local s = read()
		local at = if s then place(s) else nil
		if at then
			local q = quote_of(s)
			at.text, at.who, at.from, at.offset = q.text, q.who, q.from, offset:peek()
		end
		spot:set(at)
	end
	local function take(act: ((q: Quote.Quote) -> ())?, q: Quote.Quote?)
		spot:set(nil)
		SelectionMenu.clear()
		if q and act then
			act(q)
		end
	end
	-- What the bar offers, in the selection's menu (which then stands for it).
	local function offers(s: any): { any }
		spot:set(nil)
		local q = quote_of(s)
		local out: { any } = {}
		if props.on_mention then
			table.insert(out, {
				label = locale.s("ds.transcript.mention"),
				icon = "chat-round-line",
				on_select = function()
					take(props.on_mention, q)
				end,
			})
		end
		if props.on_attach then
			table.insert(out, {
				label = locale.s("ds.transcript.attach"),
				icon = "paperclip",
				on_select = function()
					take(props.on_attach, q)
				end,
			})
		end
		return out
	end
	-- A scroll takes it away: it no longer stands by what it offers.
	-- (The offset is read only while there is one: no work a scroll frame
	-- otherwise.)
	ui.effect(function()
		local at = spot:get()
		if at and math.abs(offset:get() - at.offset) > 1 then
			spot:set(nil)
		end
	end)
	local Action = ui.component(function(): Node
		local pill = ui.ref()
		-- However the selection went (a press elsewhere, a key, its row
		-- rebuilt), the action goes with it.
		ui.every(WATCH, function()
			if read() == nil then
				spot:set(nil)
				return false
			end
			return true
		end)
		local function word(text: Label, label: Label, act: ((q: Quote.Quote) -> ())?): Node
			return ui.button {
				label = label,
				h = WORD,
				px = "$zeron.space.snug",
				-- A pill in a pill, its curve parallel to the bar's.
				rounded = "$radius.full",
				bg = "#00000000",
				color = "$text.primary",
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				cursor = "pointer",
				hover = { bg = "$hover" },
				active = { bg = "$pressed" },
				on_click = function()
					local at = spot:peek()
					take(act, if at then { text = at.text, who = at.who, from = at.from } else nil)
				end,
				text,
			}
		end
		-- Where it stands: the last place it was given, so it fades out there.
		local stood: any = nil
		local function standing(): any
			stood = spot:get() or stood
			return stood
		end
		local bar: ContainerProps = {
			ref = pill,
			absolute = true,
			top = function(): number
				local at = standing()
				return if at then at.y else 0
			end,
			left = function(): number
				local at = standing()
				local el = list:peek()
				if not at or not el then
					return 0
				end
				local w = pill.width or 0
				local x = if at.left then at.left else at.right - w
				return math.clamp(x, MARGIN, math.max(MARGIN, (list.width or 0) - w - MARGIN))
			end,
			gap = "$zeron.space.hair",
			items = "center",
			shadow = "$shadow.md",
			selectable = false,
			word(locale.t("ds.transcript.mention"), locale.t("ds.transcript.mention_hint"), props.on_mention),
			word(locale.t("ds.transcript.attach"), locale.t("ds.transcript.attach_hint"), props.on_attach),
		}
		control.contain(glass.style(bar, "menu"), { radius = "$radius.full", inset = "$zeron.space.hair", border = 1 })
		return ui.row(bar)
	end)
	-- Away from the end long enough to mean it: a moment's leaving isn't,
	-- nor a send's glide (it steps off the end to rise to it again).
	local pending: (() -> ())? = nil
	ui.effect(function()
		local here = (at_end:get() and not placing:get()) or gliding:get()
		if pending then
			pending()
			pending = nil
		end
		if here then
			away:set(false)
		else
			pending = ui.after(150, function()
				pending = nil
				away:set(true)
			end)
		end
	end)

	local function row(m: Message, index: State<number>, state: State<Message>): Node
		local mine = m.role == "user"
		-- The runway: the room under the newest prompt that lets it stand at
		-- the view's top while its reply is written under it. A prompt sent
		-- alone (its reply to come) holds it all, from the moment it's sent;
		-- once its reply is under it, the reply holds the view less the
		-- prompt's own height (pinned below), so the two take the same room
		-- and nothing moves as the reply arrives.
		local function need(): number
			local h = held:get()
			if h.id == m.id then
				return h.h
			end
			local i = index:get()
			if i ~= count:get() then
				return 0
			end
			if mine then
				return if sent_here(m.id) then list.height or 0 else 0
			end
			if i < 2 then
				return 0
			end
			local p = pin:get()
			local prompt = messages:at(i - 1)
			if not prompt or prompt.id ~= p.id or not sent_here(prompt.id) then
				return 0
			end
			return math.max(0, (list.height or 0) - p.h)
		end
		local function last(): boolean
			return index:get() == count:get()
		end
		local inner = ui.ref()
		local outer = ui.ref()
		local row_w: number? = nil
		-- (`state`: the message as it is now; `m`, as the row was made.)
		rows[m.id] = { outer = outer, inner = inner, index = index, m = m, state = state }
		ui.on_cleanup(function()
			rows[m.id] = nil
		end)
		-- Zeron's gaps: a message opens 16 under the lane of the one before;
		-- a prompt sits 6 under the switch seam over it.
		local function pt(): number
			if mine and index:get() > 1 then
				local prev = messages:at(index:get() - 1)
				if prev and prev.role == "switch" then
					return SEAM_GAP
				end
			end
			return TURN_GAP
		end
		local function hovered(): boolean
			return hover:get() == m.id
		end
		if mine then
			tops[m.id] = outer
			ui.on_cleanup(function()
				tops[m.id] = nil
			end)
		end
		runways[m.id] = {
			need = need,
			own = function(): number
				return (inner.height or 0) + pt()
			end,
		}
		ui.on_cleanup(function()
			runways[m.id] = nil
		end)
		return ui.row {
			ref = outer,
			on_wheel = wheeled,
			on_resize = function(size: { w: number, h: number })
				if TRACING then
					trace("row", m.id, size.h, need())
				end
				size_row(m.id, size.h)
				put_back(m.id)
				-- Its width changed (the view's did): its lines rewrapped this
				-- frame; if the reading place is in it, put that back now.
				local width_changed = row_w ~= nil and math.abs(size.w - (row_w :: number)) >= 0.5
				row_w = size.w
				if not end_held and reading and reading.row == m.id and (width_changed or ui.now() < resizing_until) then
					keep_reading()
				end
			end,
			w_full = true,
			justify = "center",
			items = "start",
			px = if props.side then ui.raw(props.side, "a little wider than the composer") else "$space.5",
			pt = function(): number
				return ui.raw(pt(), "Zeron's gaps over a message (transcript.rs top_gap_for)")
			end,
			-- The air at the conversation's end, on its last row: on the list
			-- itself it would clip every row short of the list's foot.
			pb = function(): number
				return if last() then ui.raw(TAIL + foot(), "the air at its end, and the room held for what floats over its foot") else 0
			end,
			relative = true,
			min_h = need,
			ui.column {
				ref = inner,
				-- A prompt's own height (its words, and the room above it, not
				-- the runway it may hold): what its reply's runway leaves it.
				-- Last, or its reply under it: a prompt sent alone (its reply
				-- to come) is pinned as it lands.
				on_resize = if mine
					then function(size: { w: number, h: number })
						if index:peek() >= count:peek() - 1 then
							pin:set { id = m.id, h = size.h + pt() }
						end
					end
					else nil,
				w_full = true,
				max_w = measure,
				min_w = 0,
				-- Anywhere on a message shows its lane (Zeron's entry hover).
				on_hover = if m.role == "switch"
					then nil
					else function(inside: boolean)
						if inside then
							set_hover(m.id)
						elseif hover:peek() == m.id then
							set_hover("")
						end
					end,
				if m.role == "switch" then seam(m)
					elseif mine then ui.column {
						hook = "chat.message.prompt",
						-- What it is, for a plugin's selector (and the
						-- message it's in, for what lands in it).
						class = "message message-user",
						provide = { message = m.id },
						w_full = true,
						if m.sender then sender_line(m, props) else nil,
						UserMessage {
							id = m.id,
							text = m.text,
							attachments = m.attachments,
							-- (Its pictures follow the message: sent from here,
							-- this Mac's files, then the chat's copies.)
							now = function(): any
								return state:get()
							end,
							open = props.open_link,
							link_menu = props.link_menu,
							pictures = props.sent_pictures,
							find = if find_for then find_for(`{m.id}:u`) else nil,
						},
						strip(m, state, hovered),
						waiting_line(state, props.on_retry, props.turn_state),
					}
					else reply(m, state, props.draft, hovered, props.compact, { turn_state = props.turn_state, view = list, offset = offset, at_end = at_end, row = outer, pictures = props.pictures, shown_pictures = props.sent_pictures, open = props.open_link, link_menu = props.link_menu, link_chip = props.link_chip, open_subagent = props.on_open_subagent, find = find_for }),
			},
		}
	end

	return ui.overlay {
		w_full = true,
		h_full = true,
		-- The lightbox a message's pictures open in (one a window draws).
		Lightbox.Host(),
		on_hover = function(inside: boolean)
			if not inside then
				hover:set("")
			end
		end,
		ui.div { absolute = true, left = 0, right = 0, top = 0, bottom = 0, SelectionMenu { w_full = true, h_full = true, more = offers, on_release = if props.on_mention or props.on_attach
			then function()
				-- (A turn on: the release ends the gesture first.)
				ui.after(0, settle)
			end
			else nil, ui.list {
			id = "transcript",
			ref = list,
			label = locale.t("ds.transcript.conversation"),
			data = messages,
			key = "id",
			row = row,
			row_text = function(i: number): string
				local m = messages:at(i)
				return if m then m.text else ""
			end,
			offset = offset,
			-- A conversation opens at its latest; with `keep`, where it was
			-- (a view made again by a reload): its top message back at the
			-- top, as far above it as it was.
			on_mount = function(_el)
				local at = if keep and not was_at_end then keep:peek() else nil
				if at and go_back(at) then
					return
				end
				-- Its message not come yet (a chat opened with its newest
				-- rows first): at the end meanwhile, and back there as it
				-- comes. Its place is kept until then.
				if at then
					waiting = { id = at.id, dy = at.dy }
				end
				placing:set(false)
				to_end(list, offset, count:peek())
			end,
			measured = true,
			-- Its width changed (a pane's edge dragged, a panel easing open,
			-- the window resized): the reading place put back, then taken
			-- again where it now stands.
			on_resize = function(size: { w: number, h: number })
				-- Laid out at last: a place held for it (`go_back` before its
				-- first layout) is gone back to now.
				local held = waiting
				if held and size.h > 0 and messages:index_of("id", held.id) then
					waiting = nil
					go_back(held)
				end
				local widened = last_w ~= nil and math.abs(size.w - last_w) >= 0.5
				local changed = widened or (last_h ~= nil and math.abs(size.h - last_h) >= 0.5)
				last_w, last_h = size.w, size.h
				-- (Its width changed in one step (the window maximized, a
				-- panel's page taking its width at once): every row above the
				-- reading place rewraps too, in the same layout, past what the
				-- reading row's own line puts back. A frame on, and twice more
				-- as the rows it brought into view measure, the reading place
				-- goes back where it stood in the view, by where it really is,
				-- as a place returned to is.)
				local r = reading
				if widened and r and not end_held and r.view ~= nil then
					local function settle()
						if end_held or reading ~= r then
							return
						end
						local be, list_el = r.block:peek(), list:peek()
						if not be or not list_el or be.y == nil or list_el.y == nil then
							return
						end
						local off = ((be.y :: number) - (list_el.y :: number)) - r.view
						if math.abs(off) >= 0.5 then
							offset:set(math.max(0, offset:peek() + off))
						end
					end
					ui.after(0, settle)
					ui.after(16, settle)
					ui.after(48, settle)
				end
				if changed then
					resizing_until = ui.now() + 300
					-- At the end: on it still, every frame of the change.
					if end_held and not gliding:peek() then
						-- (Asked of the list itself, which lands it as it lays
						-- out; an offset written mid-layout it would keep its
						-- place over. Then the offset, once this layout is done.)
						local el = list:peek()
						if el then
							el:scroll_to_row(count:peek(), { align = "end" })
						end
						-- (Unless the view was scrolled away from the end in
						-- between, a person's scroll landing between the change
						-- and this: it stays where they took it, and the end is
						-- let go. Nor while a send glides: the glide owns the
						-- offset, and lands at the end itself.)
						local armed = ui.now()
						ui.after(0, function()
							if not end_held or gliding:peek() then
								return
							end
							if wheeled_at >= armed and not at_end:peek() then
								trace("scrolled away mid-resize: the end let go", offset:peek())
								end_held = false
								return
							end
							-- (Nor once focus came into the list meanwhile: a
							-- row Tab brought into view stays in view. The
							-- wait is real time, so a launch's can land after
							-- a key.)
							if focused_at >= armed then
								trace("focus came in mid-resize: the end let go", offset:peek())
								end_held = false
								return
							end
							to_end(list, offset, count:peek())
						end)
						return
					end
				end
				if reading == nil then
					find_reading()
				end
			end,
			on_focus = function()
				focused_at = ui.now()
			end,
			follow = true,
			at_end = at_end,
			height = ESTIMATE,
			overscan = 3,
			-- (Its scrollbar is the rail below, over it: a bar of its own
			-- would fade at its edges with the rows.)
			w_full = true,
			h_full = true,
			-- (Zeron's 24 under the title bar; at the foot, the band the
			-- composer covers, or 24.)
			fade = function(): { number }
				return { 24, math.max(24, foot()) + (props.fade_lead or 0) }
			end,
			-- A row Tab (or ⇧Tab) focuses scrolls clear of both bands: the
			-- title bar's fade at the top, the composer at the foot (followed
			-- as it grows).
			scroll_padding = function(): { top: number, bottom: number }
				return { top = 24 + 8, bottom = math.max(24, foot()) + 8 }
			end,
			-- Always (a list's runs past its edges aren't measured, so a
			-- band "as far as content runs past" stayed shut). At rest
			-- nothing is under either band: the first row's room above it,
			-- and the room held for the foot below the last.
			-- On whole points, so a scroll moves its pixels rather than
			-- painting them again (a view between device pixels can't).
			snap = true,
		} } },
		-- The scrollbar, over the list and never faded, running its whole
		-- height: at the window's edge, beside the composer rather than
		-- under it, so scrolled to the end it reaches the page's end.
		Rail {
			id = "transcript-rail",
			offset = offset,
			extent = extent,
			view = function(): number
				return list.height or 0
			end,
			track = function(): number
				return list.height or 0
			end,
		},
		-- What a selection offers.
		ui.presence(function()
			return if spot:get() then "shown" else nil
		end, function()
			return Action {}
		end, {
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = 0,
			pointer_events = "none",
			enter = { opacity = { 0, 1 }, y = { 4, 0 }, duration = 140, ease = "outQuad" },
			exit = { opacity = 0, duration = 90 },
		}),
		-- Away from the end: the way back, over the list's foot.
		ui.row {
			absolute = true,
			left = 0,
			right = 0,
			bottom = foot,
			pb = ui.raw(6, "Zeron's: the pill floats six above the composer"),
			justify = "center",
			pointer_events = "none",
			-- (The composer's pill beside it: half its width and the gap between
			-- them to the left, the two centred together.)
			x = motion.spring(ui.computed(function(): number
				local other = if props.beside then props.beside() else nil
				return if other and other > 0 then -(other + 8) / 2 else 0
			end), { stiffness = 380, damping = 30 }),
			ui.presence(function()
				return if away:get() then "away" else nil
			end, function()
				-- The menus' glass (solid when opaque), a closer shadow: a pill
				-- over the reply, firming to the solid fill as it is touched.
				local pill = glass.style({
					label = locale.t("ds.transcript.jump_to_latest"),
					h = 30,
					px = "$space.3",
					gap = "$space.2",
					items = "center",
					rounded = "$zeron.prose.bubble.radius",
					cursor = "pointer",
					color = "$text.primary",
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
					hover = { bg = "$zeron.menu.solid" },
					active = { bg = "$zeron.menu.solid" },
				}, "menu")
				pill.shadow = "$shadow.md"
				local pill_ref = ui.ref()
				pill.ref = pill_ref
				pill.on_resize = function(size: { w: number, h: number })
					jump_w = size.w
					if props.on_jump_width and away:peek() then
						props.on_jump_width(size.w)
					end
				end
				pill.on_click = function()
					-- (It leaves as the view arrives: the keys go where it's
					-- told, not on to the next control.)
					if props.on_jump then
						local el = pill_ref:peek()
						if el then
							el:blur()
						end
						props.on_jump()
					end
					show_end()
				end
				pill[1] = ui.show(writing, status.StatusDot { kind = "working" })
				pill[2] = ui.text(function()
					return if writing:get() then locale.s("ds.transcript.new_reply") else locale.s("ds.transcript.jump_to_latest")
				end)
				pill[3] = Icon { name = "arrow-down", size = 14 }
				return ui.button(pill)
			end, {
				enter = { opacity = { 0, 1 }, y = { 8, 0 }, duration = 180, ease = "outQuad" },
				exit = { opacity = 0, y = 8, duration = 120 },
			}),
		},
	}
end

return {
	Transcript = Transcript,
	-- An offset past any conversation's end, which the list clamps to it.
	-- (`math.huge` is the end too since zgx beff48c5b; a finite one also
	-- reads as a plain number in state.)
	PAST_END = PAST_END,
	-- A prompt sent from here (a new chat's first, sent through the host):
	-- its turn holds the runway once it shows.
	sent_here = mark_sent,
	-- Goes to the end of the transcript shown (a person sending goes there,
	-- whatever they were reading).
	latest = function(name: string?)
		local go = if name then named_shown[name] else shown
		if go then
			go()
		end
	end,
	-- A person sending: `push` adds their prompt and its reply, and the
	-- transcript shown glides the prompt to the top (or jumps to the end
	-- from far up). `id`: the prompt's, which then holds the runway.
	-- (`name`: a named transcript's, a side chat's; none, the chat page's.)
	send = function(push: () -> (), id: string?, name: string?)
		if id then
			mark_sent(id)
		end
		local go = if name then named_sending[name] else sending
		if go then
			go(push)
		else
			push()
		end
	end,
	-- A queued message sent on (steered in, or sent now), as a send is: the
	-- transcript shown steps clear of the end, `deliver` hands it to the
	-- host, and its prompt glides to the top as the host's copy of it (its
	-- id, `id`, the queued message's) joins, then holds the runway.
	steer = function(deliver: () -> (), id: string, name: string?)
		mark_sent(id)
		local go = if name then named_sending[name] else sending
		if go then
			go(deliver, id)
		else
			deliver()
		end
	end,
}
