-- What a person sent, as Zeron's transcript shows it (transcript.rs,
-- `render_user_body`): a bubble of the soft wash at the right, no wider than
-- four fifths of the column, its text the reply's size.
--
-- A long message folds to five lines. Its foot fades out over the last line
-- and a half rather than cutting a line in two (Zeron's #1089), and "Show
-- more" under it opens it: the bubble's height eases to the whole text,
-- which is laid out all along behind the clip, so nothing rewraps as it
-- opens. Whether it folds is measured, not guessed from its length: a
-- message folds when its text, at the bubble's width, runs past five lines.
-- Until it is measured it stands folded (a short one is shorter than the
-- fold, so that changes nothing for it), so a long message never shows its
-- whole height first and then closes: the fold's height is there from the
-- first frame, and only a person's own Show more and Show less ease.
-- Opened, it stays open by its `id` (kept apart from the view): a row
-- scrolled away and back, a reload, the chat opened again.
--
-- `UserMessage { id = "…", text = "…" }`.

local Icon = require("@self/icon")
local Lightbox = require("@self/lightbox")
local ReplyPicture = require("@self/reply_picture")
local Markdown = require("@self/markdown")
local FadeText = require("@self/fade_text")
local locale = require("@self/locale")

-- What came with it: an image (shown as a thumbnail) or a file (a chip with
-- its name and what it is).
-- (`appshot`: a picture that is an Appshot, the app it shows and its
-- window's title: the card says so under it.)
export type Attachment = { kind: "image" | "file", name: string, src: string?, meta: string?, icon: string?, appshot: { app: string, title: string?, bundle: string? }? }
-- (`open`: what opens a link in it that isn't the web's, as a file mention.)
-- (`pictures`: where its pictures' paths show from: a chat on another
-- device reads them from there, screens/chat/reply_images'
-- `for_attachments`; without it, each path as it is.)
-- (`find`: a find's marks in its text, ds/markdown's.)
-- (`now`: the message as it is now, when it changes after it's drawn. A
-- prompt sent from here is first this Mac's copy (`here`, src/chat.rs
-- `Pending`), its pictures the files it was sent from, here; then the
-- chat's, its pictures the uploads its device keeps, read from there as
-- `pictures` says, even while it still waits for its reply. Each picture
-- follows the `n`th of them.)
export type Now = { attachments: { Attachment }?, here: boolean? }
export type Props = {
	text: string,
	attachments: { Attachment }?,
	now: (() -> Now?)?,
	id: string?,
	open: ((target: string) -> ())?,
	-- Their context menus where they aren't the web's (ds/markdown's).
	link_menu: ((target: string) -> { Markdown.LinkEntry }?)?,
	pictures: ReplyPicture.Pictures?,
	find: any?,
}

-- The messages opened, by id: every bubble's, outside any of them.
local OPENED = ui.state("user_message_opened", {} :: { [string]: boolean })

-- An image, as a thumbnail cropped to Zeron's size, in a hairline frame
-- (a light image on a light window would otherwise lose its edge). The box
-- clips it to its corners, not the image: zgx's image drops its own rounded
-- clip when `cover` crops it top and bottom (a portrait image). A click (or
-- Enter) opens it in the lightbox, growing from here; ← → go through the
-- message's other pictures.
local function thumb(a: Attachment, source: () -> (string, boolean), group: string, images: () -> { Lightbox.Item }, index: number, pictures: ReplyPicture.Pictures?): Node
	-- Where it shows from (read from the chat's device first, there): the
	-- frame holds its place meanwhile; one that can't be read says so.
	-- This Mac's own file (a prompt on its way) shows as it is: the chat's
	-- device never had it.
	-- (Its file gone since it was sent, or not a picture: `ui.image`'s
	-- `on_error` says so, and that file shows as one that can't be.)
	local broken = ui.use_state("")
	local function picture(): ReplyPicture.Picture
		local src, own = source()
		local p = if pictures and not own then pictures.resolve(src) or { state = "failed" } else { state = "ready", src = src }
		if p.state == "ready" and broken:get() == p.src then
			return { state = "failed" }
		end
		return p
	end
	if pictures then
		ui.effect(function()
			local src, own = source()
			if not own then
				pictures.fetch(src)
			end
		end)
	end
	return Lightbox.Thumb {
		group = group,
		index = index,
		items = images,
		radius = ui.theme.get("zeron.prose.thumb.radius") :: number,
		label = locale.t("ds.user_message.open_named", { name = a.name }),
		tooltip = a.name,
		w = "$zeron.prose.thumb.w",
		h = "$zeron.prose.thumb.h",
		shrink = 0,
		rounded = "$zeron.prose.thumb.radius",
		ui.overlay {
			w_full = true,
			h_full = true,
			rounded = "$zeron.prose.thumb.radius",
			overflow = "hidden",
			border = 1,
			border_color = "$border",
			border_over = true,
			bg = "$hover",
			ui.image {
				src = function(): string
					local p = picture()
					return if p.state == "ready" then p.src or "" else ""
				end,
				fit = "cover",
				w_full = true,
				h_full = true,
				on_error = function()
					local p = picture()
					if p.state == "ready" and p.src then
						broken:set(p.src)
					end
				end,
			},
			ui.show(
				function(): boolean
					return picture().state == "failed"
				end,
				ui.column {
					w_full = true,
					h_full = true,
					items = "center",
					justify = "center",
					color = "$text.tertiary",
					Icon { name = "gallery", size = 18 },
				}
			),
		},
	}
end

-- Bubbles without an id: lightbox groups of their own.
local unnamed = 0

-- A file: its glyph on a small tile, its name over what it is.
local function chip(a: Attachment): Node
	return ui.row {
		h = 56,
		max_w = 260,
		pl = "$space.3",
		pr = "$space.4",
		gap = "$space.3",
		items = "center",
		rounded = "$zeron.prose.thumb.radius",
		bg = "$zeron.prose.bubble.bg",
		label = a.name,
		ui.row {
			w = 36,
			h = 36,
			shrink = 0,
			items = "center",
			justify = "center",
			rounded = "$radius.lg",
			bg = "$hover",
			color = "$text.secondary",
			Icon { name = (a.icon or "document") :: any, size = 18 },
		},
		ui.column {
			min_w = 0,
			grow = 1,
			gap = "$zeron.space.hair",
			FadeText { a.name, literal = true, text_size = "$font.size.md", color = "$text.primary", weight = "$font.weight.medium" },
			if a.meta
				then ui.text { a.meta, literal = true, text_size = "$font.size.xs", color = "$text.tertiary" }
				else nil,
		},
	}
end

-- Five lines of the prose's line, and the fade over the last one and a half.
local LINES = 5
local FADE_LINES = 1.5

return ui.component(function(props: Props): Node
	-- Open: kept by id when it has one (else this bubble's own).
	local own = ui.use_state(false)
	local id = props.id
	local function is_open(): boolean
		return if id then OPENED:get()[id] == true else own:get()
	end
	local function set_open(on: boolean)
		if id then
			local now = table.clone(OPENED:peek())
			now[id] = if on then true else nil
			OPENED:set(now)
		else
			own:set(on)
		end
	end
	local body = ui.ref()
	local line = ui.theme.get("zeron.prose.line") :: number
	local folded_h = LINES * line
	-- Folds when the whole text is taller than five lines (half a point
	-- over for rounding); `long` once measured so.
	local long = ui.computed(function(): boolean
		return (body.height or 0) > folded_h + 0.5
	end)
	-- Held at the fold until measured, then only while long and closed.
	local function folded(): boolean
		if is_open() then
			return false
		end
		-- (A ref reads 0, or nothing, before its first layout.)
		return (body.height or 0) <= 0 or long:get()
	end
	local attachments: { Node } = {}
	-- The pictures it carries, a set in the lightbox (named by its id).
	-- (Where each shows from as it opens: a picture read from another
	-- device is its file here.)
	local pictures = props.pictures
	-- Where its `n`th picture shows from now, and whether that's this
	-- Mac's own file (`now`'s): as drawn, without it.
	local function source(n: number, a: Attachment): (string, boolean)
		local m = if props.now then props.now() else nil
		if not m then
			return a.src or "", false
		end
		local own = m.here == true
		local k = 0
		for _, b in m.attachments or {} do
			if b.kind == "image" and b.src and b.src ~= "" then
				k += 1
				if k == n then
					return b.src, own
				end
			end
		end
		return a.src or "", own
	end
	local function images(): { Lightbox.Item }
		local out: { Lightbox.Item } = {}
		local n = 0
		for _, a in props.attachments or {} do
			if a.kind == "image" and a.src and a.src ~= "" then
				n += 1
				local at, own = source(n, a)
				local p = if pictures and not own then pictures.resolve(at) else nil
				local src = if p and p.state == "ready" and p.src then p.src else at
				table.insert(out, { src = src, name = a.name, path = if string.sub(src, 1, 1) == "/" then src else nil })
			end
		end
		return out
	end
	if not id then
		unnamed += 1
	end
	local group = `message:{id or `unnamed{unnamed}`}`
	local shown = 0
	for _, a in props.attachments or {} do
		if a.kind == "image" and a.src and a.src ~= "" then
			shown += 1
			local n = shown
			local function from(): (string, boolean)
				return source(n, a)
			end
			local shot = a.appshot
			if shot then
				-- An Appshot: its picture over "{App} · Appshot" and the
				-- window's title (the shipping app's card).
				table.insert(attachments, ui.column {
					w = "$zeron.prose.thumb.w",
					gap = "$zeron.space.hair",
					shrink = 0,
					label = locale.t("ds.user_message.appshot_label", { app = shot.app, title = shot.title or shot.app }),
					thumb(a, from, group, images, shown, pictures),
					ui.text { locale.t("ds.user_message.appshot", { app = shot.app }), literal = true, truncate = true, text_size = "$font.size.xs", color = "$text.tertiary" },
					ui.text { shot.title or shot.app, literal = true, truncate = true, text_size = "$font.size.sm", color = "$text.secondary" },
				})
			else
				table.insert(attachments, thumb(a, from, group, images, shown, pictures))
			end
		else
			table.insert(attachments, chip(a))
		end
	end
	return ui.column {
		w_full = true,
		items = "end",
		gap = "$space.2",
		role = "article",
		label = locale.t("ds.user_message.you"),
		-- What came with it rides above the words, at the right, as Zeron's do
		-- (not part of a selection of the words: it isn't what was said).
		if #attachments > 0
			then ui.row { selectable = false, max_w = "80%", flex_wrap = true, justify = "end", gap = "$space.2", attachments }
			else nil,
		if props.text == "" then nil else ui.column {
			min_w = 0,
			max_w = "80%",
			px = "$space.4",
			py = "$zeron.space.snug",
			rounded = "$zeron.prose.bubble.radius",
			bg = "$zeron.prose.bubble.bg",
			-- The clip over the whole text: five lines while folded, fading at
			-- its foot; open, as tall as the text, the space easing between.
			ui.column {
				w_full = true,
				overflow = "hidden",
				max_h = function(): number
					return if folded() then folded_h else 1e6
				end,
				layout = "height",
				layout_clip = true,
				layout_duration = 260,
				layout_ease = "outQuart",
				fade = function()
					return { 0, if folded() and long:get() then line * FADE_LINES else 0 }
				end,
				ui.column {
					ref = body,
					w_full = true,
					shrink = 0,
					Markdown { text = props.text, breaks = true, open = props.open, link_menu = props.link_menu, find = props.find },
				},
			},
			-- The way to open it: a plain link under the text, at its left
			-- edge, no pill or wash.
			ui.show(
				long,
				ui.button {
					label = function()
						return if is_open() then locale.s("ds.user_message.collapse") else locale.s("ds.user_message.expand")
					end,
					expanded = is_open,
					self = "start",
					p = 0,
					bg = "#00000000",
					active = { bg = "#00000000" },
					mt = "$space.2",
					gap = "$space.1",
					items = "center",
					cursor = "pointer",
					selectable = false,
					text_size = "$font.size.md",
					line_height = "$font.line.md",
					color = "$text.secondary",
					hover = { color = "$text.primary", bg = "#00000000" },
					focus = { border = 0 },
					on_click = function()
						set_open(not is_open())
					end,
					ui.text(function()
						return if is_open() then locale.s("ds.user_message.show_less") else locale.s("ds.user_message.show_more")
					end),
					Icon {
						name = function()
							return if is_open() then "alt-arrow-up" else "alt-arrow-down"
						end,
						size = 12,
					},
				}
			),
		},
	}
end)
