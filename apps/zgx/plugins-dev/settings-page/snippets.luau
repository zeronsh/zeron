--!strict
-- Snippets' model: the snippets you keep (`zeron.store`, a list kept for
-- this plugin alone), how each is checked, and what one puts in the message
-- box. The page (settings_page.luau) and the button (init.luau) both read
-- it; neither keeps a copy of its own.

local zeron = require("./zeron")("settings-page")

local M = {}

-- A snippet: its words, and whether they go in as written ("text") or with
-- its fields filled in ("template").
export type Snippet = { id: string, name: string, text: string, kind: string }

M.MAX_NAME = 40
M.MAX_TEXT = 2000

-- What a template fills in, as it goes in.
M.FIELDS = {
	date = function(): string
		return (os.date("%A %d %B") :: string)
	end,
	time = function(): string
		return (os.date("%H:%M") :: string)
	end,
}

-- What a new install starts with.
local STARTERS: { Snippet } = {
	{ id = "review", name = "Ask for a review", text = "Could you review this when you have a moment? I'd most like a second look at the tests.", kind = "text" },
	{ id = "standup", name = "Standup", text = "Standup for {date}\n- Done:\n- Next:\n- In the way:", kind = "template" },
}

local kept = zeron.store("snippets", STARTERS)

-- Every snippet, in the order kept (followed where it's read).
function M.list(): { Snippet }
	local list = kept:get()
	return if type(list) == "table" then list else {}
end

-- One by its id, now, or nil.
function M.find(id: string): Snippet?
	for _, s in M.list() do
		if s.id == id then
			return s
		end
	end
	return nil
end

local function trimmed(text: string): string
	return string.match(text, "^%s*(.-)%s*$") or ""
end

-- Why `name` won't do for snippet `id`, or nil.
function M.name_problem(name: string, id: string): string?
	local n = trimmed(name)
	if n == "" then
		return "Give it a name."
	elseif utf8.len(n) and (utf8.len(n) :: number) > M.MAX_NAME then
		return `Keep it to {M.MAX_NAME} characters.`
	end
	for _, s in M.list() do
		if s.id ~= id and string.lower(s.name) == string.lower(n) then
			return "Another snippet has this name."
		end
	end
	return nil
end

-- Why `text` won't do for a snippet of `kind`, or nil.
function M.text_problem(text: string, kind: string): string?
	if trimmed(text) == "" then
		return "Write what it puts in the message box."
	elseif #text > M.MAX_TEXT then
		return `Keep it to {M.MAX_TEXT} characters.`
	end
	if kind == "template" then
		for field in string.gmatch(text, "{([%w_]*)}") do
			if not (M.FIELDS :: any)[field] then
				return "{" .. field .. "} isn't a field. Use {date} or {time}."
			end
		end
	end
	return nil
end

-- What `s` puts in the message box now: its words, a template's fields
-- filled in.
function M.fill(s: Snippet): string
	if s.kind ~= "template" then
		return s.text
	end
	return (string.gsub(s.text, "{([%w_]*)}", function(field: string): string?
		local fill = (M.FIELDS :: any)[field]
		return if fill then fill() else nil
	end))
end

local function save(list: { Snippet })
	kept:set(list)
end

-- `id`'s fields changed (those given).
function M.update(id: string, fields: { name: string?, text: string?, kind: string? })
	local list = table.clone(M.list())
	for i, s in list do
		if s.id == id then
			local next = table.clone(s)
			if fields.name then
				next.name = trimmed(fields.name)
			end
			if fields.text then
				next.text = fields.text
			end
			if fields.kind then
				next.kind = fields.kind
			end
			list[i] = next
			save(list)
			return
		end
	end
end

-- A new, empty snippet at the end, named so no other is: its id.
function M.add(): string
	local list = table.clone(M.list())
	local name, n = "New snippet", 1
	while M.name_problem(name, "") ~= nil do
		n += 1
		name = `New snippet {n}`
	end
	local id = `s{math.floor(ui.now())}-{#list + 1}`
	table.insert(list, { id = id, name = name, text = "", kind = "text" })
	save(list)
	return id
end

-- `id` let go of; the one now in its place (the next, else the last), or nil.
function M.remove(id: string): string?
	local list = table.clone(M.list())
	for i, s in list do
		if s.id == id then
			table.remove(list, i)
			save(list)
			local beside = list[i] or list[#list]
			return if beside then beside.id else nil
		end
	end
	return nil
end

return M
