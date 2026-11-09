-- What a message goes with, from the chips on its draft (screens/chat/shared's
-- cargo), for whichever composer sends it: the chat page's, a side chat's.
--
--   local p = Carried.pack(chips, typed, text)   sorted: words, attachments, chats
--   Carried.text(words, p)                       the words with what goes in them
--   Carried.shown(p)                             its chips on the sent prompt

local Quote = require("@ds/quote")
local Comments = require("@app/screens/chat/comments")
local Annotation = require("@ds/annotation")
local ChatLinks = require("@app/screens/chat/chat_links")
local AttachmentMentions = require("@ds/attachment_mentions")

local M = {}

export type Pack = {
	-- The words so far: `typed`, with what chips put in them.
	typed: string,
	attachments: { any },
	-- Passages attached from the conversation: a block after the words
	-- (Quote.with_quotes), each with who said it and when.
	quotes: { Quote.Quote },
	-- Elements picked on a page in the panel's browser: each a token after the
	-- words (Annotation.with_annotations), which the engine expands for the
	-- agent.
	picks: { { [string]: any } },
	-- Review comments left on files: a block after the words
	-- (screens/chat/comments), each its file and line.
	comments: { Comments.Comment },
	-- Other chats attached (a session dropped from the sidebar): a block
	-- after the words (`with_chats`), each its link, which the engine reads
	-- out to the agent; the sent message shows each as a card, as the
	-- composer did (src/chat.rs `split_chats`).
	chats: { { name: string, chat: string, mark: string? } },
}

-- The block attached chats go in after the words (src/chat.rs
-- `CHATS_HEADER`): its header, then `- [title](zeron-chat:id)`.
M.CHATS_HEADER = "Chats attached:"

-- `words` with `chats`' block after them.
local function with_chats(words: string, chats: { { name: string, chat: string, mark: string? } }): string
	local lines: { string } = {}
	for _, c in chats do
		local link = ChatLinks.link(c.name, c.chat)
		if link then
			table.insert(lines, `- {link}`)
		end
	end
	if #lines == 0 then
		return words
	end
	local block = `{M.CHATS_HEADER}\n{table.concat(lines, "\n")}`
	return if string.find(words, "%S") then `{words}\n\n{block}` else block
end

-- `typed`: the words taken from the draft. `text`: what the composer sent, if
-- that's nothing.
function M.pack(chips: { any }, typed: string, text: string): Pack
	local p: Pack = { typed = typed, attachments = {}, quotes = {}, picks = {}, comments = {}, chats = {} }
	for _, item in chips do
		if item.kind == "comment" then
			table.insert(p.comments, { file = item.file, line = item.line, body = item.body, to = item.to, side = item.side, snippet = item.snippet })
		elseif item.path then
			-- (A project's file: on the session's computer already.)
			-- (An Appshot goes as its picture; what it shows of the app, below.)
			-- (A picture goes up as `Image N` (its number in the draft), as
			-- the shipping app stages one: a mention of it names it so.)
			local picture = item.kind == "image" or item.kind == "appshot"
			local name = item.name
			if picture and type(item.mention) == "number" then
				local ext = string.match(item.path, "(%.[%w]+)$") or ""
				name = AttachmentMentions.image_label(item.mention) .. ext
			end
			table.insert(p.attachments, { kind = if item.kind == "appshot" then "image" else item.kind, path = item.path, name = name, rel = item.rel, on_host = item.on_host })
		elseif item.kind == "annotation" and item.quote then
			table.insert(p.quotes, { text = item.quote, who = item.name, from = item.from })
		elseif item.kind == "annotation" and item.annotation then
			table.insert(p.picks, item)
		elseif item.kind == "text" and item.text then
			-- A pasted text carried as a chip: its words go in the text.
			p.typed = p.typed .. "\n\n" .. item.text
		elseif item.kind == "chat" and item.chat then
			-- Another chat attached (a session dropped from the sidebar):
			-- in a block after the words, as an attachment (a chat mentioned
			-- with @ in the words stays a link in them).
			table.insert(p.chats, { name = item.name, chat = item.chat, mark = item.mark })
			if p.typed == "" and text ~= "" then
				p.typed = text
			end
		end
	end
	return p
end

-- The prompt: `words`, with the blocks and tokens of what it carries.
function M.text(words: string, p: Pack): string
	-- (A pick mentioned in the words is there already: not again after.)
	local picks: { any } = {}
	for _, item in p.picks do
		if not string.find(words, Annotation.token(item.annotation or item), 1, true) then
			table.insert(picks, item)
		end
	end
	-- (The chats' block last: the host takes it off first, src/chat.rs.)
	return with_chats(Comments.with_comments(Annotation.with_annotations(Quote.with_quotes(words, p.quotes), picks), p.comments), p.chats)
end

-- An attached item's name, as what the message carries says it (its
-- block's line, its chip on the sent message): what an `@` reference to it
-- says, "(attachment: <name>)", and the sent message draws as its chip.
-- Nil for what has none (pasted words go in the words themselves).
function M.label(item: any): string?
	if item.kind == "comment" then
		return Comments.place(item)
	elseif item.kind == "annotation" and item.quote then
		return Quote.where({ text = item.quote, who = item.name, from = item.from })
	elseif item.kind == "annotation" and item.annotation then
		return Annotation.label(item.annotation.index or item.index or 0)
	elseif item.kind == "text" then
		return nil
	end
	return if type(item.name) == "string" and item.name ~= "" then item.name else nil
end

-- What each kind of attached item is, in a word ("Image", "Quote"), and
-- its icon: an `@` row's.
function M.kind(item: any): (string, string)
	if item.kind == "comment" then
		return "comment", "chat-round-line"
	elseif item.kind == "annotation" and item.quote then
		return "quote", "cursor"
	elseif item.kind == "annotation" then
		return "annotation", "cursor"
	elseif item.kind == "chat" then
		return "chat", "side-chat"
	elseif item.kind == "appshot" then
		return "appshot", "gallery"
	elseif item.kind == "image" then
		return "image", "gallery"
	end
	return "file", "paperclip"
end

-- The canonical link an `@` reference to `item` puts in the words: a
-- picture's or a file's by its number in the draft (ds/attachment_mentions),
-- an attached chat's Zeron chat mention, a browser pick's token. Each the
-- engine gives an agent as its plain label. Nil for what has no such link
-- (a quote, a review comment, pasted words).
function M.mention(item: any): string?
	if item.kind == "chat" and item.chat then
		return ChatLinks.link(item.name, item.chat)
	elseif item.kind == "annotation" and item.annotation then
		return Annotation.token(item.annotation)
	elseif item.path and type(item.mention) == "number" then
		local picture = item.kind == "image" or item.kind == "appshot"
		return AttachmentMentions.link(item.mention, if picture then nil else item.name)
	end
	return nil
end

-- The words as they go: a mention of an attachment no longer attached
-- leaves as its plain label (proto `demote_unattached_mentions`).
function M.settle(words: string, chips: { any }): string
	local numbers: { number } = {}
	for _, item in chips do
		if item.path and type(item.mention) == "number" then
			table.insert(numbers, item.mention)
		end
	end
	return AttachmentMentions.demote_unattached(words, numbers)
end

-- What a sent message shows of what it carries.
function M.shown(p: Pack): { any }
	local out = {}
	for _, a in p.attachments do
		table.insert(out, {
			kind = if a.kind == "image" then "image" else "file",
			name = a.name,
			src = if a.kind == "image" then a.path else nil,
		})
	end
	-- And each passage it carries, as a chip (as the host's copy shows
	-- them: chat.rs `split_quotes`).
	for _, q in p.quotes do
		table.insert(out, { kind = "file", icon = "cursor", name = Quote.where(q), meta = Quote.excerpt(q.text) })
	end
	-- And each element picked on a page (chat.rs `split_annotations`).
	for _, chip in Annotation.shown(p.picks) do
		table.insert(out, chip)
	end
	-- And each chat attached, a card with its agent's mark (chat.rs
	-- `chat_chip`).
	for _, c in p.chats do
		table.insert(out, { kind = "chat", name = c.name, chat = c.chat, mark = c.mark })
	end
	-- And its review comments, one chip (chat.rs `comments_chip`).
	if #p.comments > 0 then
		local places: { string } = {}
		for _, c in p.comments do
			table.insert(places, Comments.place(c))
		end
		table.insert(out, {
			kind = "file",
			icon = "chat-round-line",
			name = ui.t("chat.composer.comments", { n = #p.comments }):get(),
			meta = table.concat(places, ", "),
		})
	end
	return out
end

return M
