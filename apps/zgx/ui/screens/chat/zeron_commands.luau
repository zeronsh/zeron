-- Zeron's own `/` commands, beside the agent's (crates/ui/src/composer.rs
-- `WorkspaceCommand`, run by shell.rs): each does something in the window
-- rather than going to the agent. Taken from the list, a command runs at
-- once and its token goes, the rest of the draft staying (nothing is sent);
-- a draft that is only the command (`/model`) runs it as it's sent. Where
-- the agent has a command of the same name, the agent's keeps it and
-- Zeron's is `/zeron:name`. Those for a chat (files, terminal, rename,
-- stop) are offered only in one.
--
-- (`/diff` is left out until the panel has a changes tab.)

local locale = require("@ds/locale")

local M = {}

export type Command = { name: string, detail: string, chat: boolean }

M.CATALOG = {
	{ name = "model", detail = "Zeron: choose agent, model, and reasoning", chat = false },
	{ name = "new", detail = "Zeron: start a new conversation", chat = false },
	{ name = "resume", detail = "Zeron: search and open conversations", chat = false },
	{ name = "settings", detail = "Zeron: open settings", chat = false },
	{ name = "files", detail = "Zeron: open project files", chat = true },
	{ name = "terminal", detail = "Zeron: open a terminal", chat = true },
	{ name = "rename", detail = "Zeron: rename this conversation", chat = true },
	{ name = "stop", detail = "Zeron: stop the active run", chat = true },
} :: { Command }

-- The rows the `/` list ends with: Zeron's commands, named clear of the
-- agent's (`taken`: the names it uses), in a chat or not. (Each `detail`
-- in the locale shown, chat.zeron_commands.<name>; the catalog keeps the
-- English.)
function M.rows(taken: { [string]: boolean }, in_chat: boolean): { any }
	local out = {}
	for _, c in M.CATALOG do
		if c.chat and not in_chat then
			continue
		end
		local name = c.name
		while taken[name] do
			name = `zeron:{name}`
		end
		table.insert(out, {
			value = name,
			label = `/{name}`,
			detail = locale.s(`chat.zeron_commands.{c.name}`),
			icon = "command",
			-- (Taken, its token goes: ds/mention_tokens.)
			consume = true,
			zeron = c.name,
		})
	end
	return out
end

-- The command a whole draft is, if any: its own token, nothing before it
-- (a draft indented is words, as Zeron's reads it), only space after.
function M.of_text(text: string, rows: { any }): string?
	local token = string.match(text, "^/([%w%./_:%-]+)%s*$")
	if not token then
		return nil
	end
	for _, r in rows do
		if r.zeron and r.value == token then
			return r.zeron
		end
	end
	return nil
end

-- Runs Zeron's command `name` for the chat `session` (if one is open).
-- (Its modules looked up as it runs: the shell's and the panel's require
-- the chat page's.)
function M.run(name: string, session: string?)
	local req: any = require
	if name == "model" then
		req("@app/screens/chat/shared").open_model_picker()
	elseif name == "new" then
		req("@app/screens/shell/route").new_chat()
	elseif name == "resume" then
		req("@app/screens/shell/view").palette:set(true)
	elseif name == "settings" then
		req("@app/screens/shell/route").settings()
	elseif name == "files" then
		req("@app/screens/panel").show_files()
	elseif name == "terminal" then
		req("@app/screens/panel").show_terminal()
	elseif name == "rename" and session then
		req("@app/screens/shell/sidebar").rename(session)
	elseif name == "stop" and session then
		req("@app/screens/chat/store").interrupt(session)
	end
end

return M
