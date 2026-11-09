-- What a message goes with, from the chips on its draft (screens/chat/shared's
-- cargo), for whichever composer sends it: the chat page's, a side chat's.
--
--   local p = Carried.pack(chips, typed, text)   sorted, chat links in `p.typed`
--   Carried.text(words, p)                       the words with what goes in them
--   Carried.shown(p)                             its chips on the sent prompt

local Quote = require("@ds/quote")
local Comments = require("@app/screens/chat/comments")
local Annotation = require("@ds/annotation")
local ChatLinks = require("@app/screens/chat/chat_links")

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
}

-- `typed`: the words taken from the draft. `text`: what the composer sent, if
-- that's nothing.
function M.pack(chips: { any }, typed: string, text: string): Pack
	local p: Pack = { typed = typed, attachments = {}, quotes = {}, picks = {}, comments = {} }
	for _, item in chips do
		if item.kind == "comment" then
			table.insert(p.comments, { file = item.file, line = item.line, body = item.body, to = item.to, side = item.side, snippet = item.snippet })
		elseif item.path then
			-- (A project's file: on the session's computer already.)
			-- (An Appshot goes as its picture; what it shows of the app, below.)
			table.insert(p.attachments, { kind = if item.kind == "appshot" then "image" else item.kind, path = item.path, name = item.name, rel = item.rel, on_host = item.on_host })
		elseif item.kind == "annotation" and item.quote then
			table.insert(p.quotes, { text = item.quote, who = item.name, from = item.from })
		elseif item.kind == "annotation" and item.annotation then
			table.insert(p.picks, item)
		elseif item.kind == "text" and item.text then
			-- A pasted text carried as a chip: its words go in the text.
			p.typed = p.typed .. "\n\n" .. item.text
		elseif item.kind == "chat" and item.chat then
			-- Another chat mentioned (a session dropped from the sidebar):
			-- its link in the words, the engine's to read out to the agent.
			local link = ChatLinks.link(item.name, item.chat) or ""
			p.typed = if p.typed ~= "" then `{p.typed} {link}` elseif text ~= "" then `{text} {link}` else link
		end
	end
	return p
end

-- The prompt: `words`, with the blocks and tokens of what it carries.
function M.text(words: string, p: Pack): string
	return Comments.with_comments(Annotation.with_annotations(Quote.with_quotes(words, p.quotes), p.picks), p.comments)
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
