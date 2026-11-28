-- Another Zeron chat mentioned in a message (crates/proto chat_mentions.rs):
-- `[title](zeron-chat:<id>)`, its title on one line with its brackets,
-- backslashes and backticks escaped, and only a plain chat id. The engine
-- reads it out to the agent as that chat and how to read it.
--
--   chat_links.link(title, id)   the link, or nil for an id that isn't plain
--   chat_links.id_of(target)     the chat a link opens (a mention or a
--                                chat's `zeron://open/chat/…` link), or nil
--   chat_links.title(title)      the title as the chip and the link say it

local M = {}

function M.valid(id: string): boolean
	return #id > 0 and #id <= 128 and string.match(id, "^[%w_%-]+$") ~= nil
end

function M.title(title: string?): string
	local t = string.match((string.gsub(title or "", "%s+", " ")), "^%s*(.-)%s*$") or ""
	return if t ~= "" then t else ui.t("chat.chat_links.untitled"):get()
end

-- The chat a link opens, by its id: a mention (`zeron-chat:<id>`) or the
-- link a chat's Copy link puts on the clipboard
-- (`zeron://open/chat/<id>?workspace=…`, crates/notify links.rs), pasted
-- into a message or written by an agent. nil for anything else.
function M.id_of(target: string): string?
	local id = string.match(target, "^zeron%-chat:([%w_%-]+)$")
		or string.match(target, "^zeron://open/chat/([%w_%-]+)$")
		or string.match(target, "^zeron://open/chat/([%w_%-]+)[?#]")
	if id and M.valid(id) then
		return id
	end
	return nil
end

function M.link(title: string?, id: string): string?
	if not M.valid(id) then
		return nil
	end
	local label = string.gsub(M.title(title), "[\\%[%]`]", function(c: string): string
		return "\\" .. c
	end)
	return `[{label}](zeron-chat:{id})`
end

return M
