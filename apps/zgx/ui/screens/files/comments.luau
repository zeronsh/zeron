-- Review comments on a file's lines (the shipping app's comments.rs): left
-- on a line of the editor, staged on the chat's composer, folded into the
-- next message as text. The composer keeps them (screens/chat/shared's
-- `attach_comment`, a cargo item of kind "comment" with `path`, `root`,
-- `line`, `body`); this reads and writes them there for the files pane,
-- and is what a fixture stands in for.
--
--   comments.available()            whether the chat page takes comments
--   comments.add({ path, line, body, root })    staged; its id
--   comments.of(root, path)         the file's staged comments, by line
--                                   (reactive): { { id, line, body } }
--   comments.edit(id, body)         its words again
--   comments.move(id, line)         carried to another line by edits
--   comments.remove(id)             unstaged
--
-- (Changes go through the composer's own calls: its chip's name and words
-- are made from the comment's fields, so they follow.)

export type Comment = { id: string, line: number, body: string }

local M = {}

-- The chat page's composer state, if it's there (a fixture mounts files
-- alone).
local function chat(): any?
	local ok, shared = pcall(function(): any
		return (require :: any)("@app/screens/chat/shared")
	end)
	return if ok then shared else nil
end

-- The composer's call `name`, if it has it.
local function call(name: string): ((...any) -> any)?
	local shared = chat()
	if shared and type(shared[name]) == "function" then
		return shared[name]
	end
	return nil
end

function M.available(): boolean
	return call("attach_comment") ~= nil
end

function M.add(c: { path: string, line: number, body: string, root: string? }): string?
	local attach = call("attach_comment")
	return if attach then attach({ path = c.path, line = c.line, body = c.body, root = c.root }) else nil
end

function M.of(root: string?, path: string): { Comment }
	local shared = chat()
	if not shared or not shared.cargo then
		return {}
	end
	local out: { Comment } = {}
	for _, item in shared.cargo:get() do
		if item.kind == "comment" and item.path == path and item.root == root then
			table.insert(out, { id = item.id, line = item.line, body = item.body or "" })
		end
	end
	return out
end

function M.edit(id: string, body: string)
	local update = call("update_comment")
	if update then
		update(id, body)
	end
end

function M.move(id: string, line: number)
	local move = call("move_comment")
	if move then
		move(id, line)
	end
end

function M.remove(id: string)
	local remove = call("remove_cargo")
	if remove then
		remove(id)
	end
end

return M
