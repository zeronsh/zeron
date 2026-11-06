-- What a chat's composer offers as `@`, `/` and `$` are typed, for whichever
-- composer shows that chat (the chat page's, a side chat's): the files of where
-- it works, the agent's commands and skills, then Zeron's own.
--
--   local c = Completions.bind { key, text, agent, target, files, in_chat, run }
--   ds.Composer { mentions = c.lists, on_mention = c.on_mention,
--     mention_enabled = c.enabled, mention_empty = c.empty }
--   c.command_of(draft)   the Zeron command a whole draft is, if any
--
-- `key` names what it keeps (so a reload reads nothing again), `text` is the
-- draft, `agent` the harness its model runs on, `target` where its commands and
-- skills are read (`{ chat, device }`, or a new chat's `{ space, device }`),
-- `files` where `@` searches, `in_chat` whether the chat is made yet, and `run`
-- runs one of Zeron's commands.

local files = require("@app/screens/files/api")
local mentions = require("@ds/composer_mentions")
local prefs = require("@app/screens/settings/prefs")
local Invocations = require("@app/screens/chat/invocations")
local ZeronCommands = require("@app/screens/chat/zeron_commands")

local M = {}

export type Bind = {
	key: string,
	text: State<string>,
	agent: () -> string?,
	target: () -> Invocations.Target?,
	files: () -> any?,
	in_chat: () -> boolean,
	run: (name: string) -> (),
}

function M.bind(b: Bind): { [string]: any }
	-- What `@` offers: the files of where it works, searched as the mention is
	-- typed. (`/` and `$` come from the catalog, below.)
	local sources = ui.state(`{b.key}_mentions`, { ["/"] = {}, ["$"] = {} } :: any)
	local function invocation_key(): string?
		return Invocations.key(b.agent(), b.target())
	end
	local lists = ui.computed(function(): any
		local given = table.clone(sources:get())
		-- `/` the agent's commands, `$` its skills (nil while they come).
		local catalog = Invocations.catalog(invocation_key())
		given["/"] = Invocations.items(catalog, "command")
		given["$"] = Invocations.items(catalog, "skill")
		-- No agent to ask: Zeron's own alone. Its list still coming: nil.
		if b.agent() == nil then
			given["/"] = {}
		elseif given["/"] == nil then
			return given
		end
		local slash = table.clone(given["/"] or {})
		local taken: { [string]: boolean } = {}
		for _, c in slash do
			taken[c.value] = true
		end
		if not prefs.completion(b.agent()).separate then
			for _, skill in given["$"] or {} do
				table.insert(slash, skill)
			end
		end
		-- Then Zeron's own (named clear of the agent's).
		for _, row in ZeronCommands.rows(taken, b.in_chat()) do
			table.insert(slash, row)
		end
		given["/"] = slash
		return given
	end)
	-- A `/` or `$` opening afresh reads the agent's list again (the last one
	-- showing meanwhile), as does another agent or place while it's open. Kept,
	-- so a reload with it open reads nothing.
	local invocations_open = ui.state(`{b.key}_invocations_open`, nil :: string?)
	ui.effect(function()
		local text = b.text:get()
		local t = mentions.token(text, #text)
		local wanted = t ~= nil and (t.trigger == "/" or (t.trigger == "$" and prefs.completion(b.agent()).dollar))
		local key = if wanted then invocation_key() else nil
		if key == invocations_open:peek() then
			return
		end
		invocations_open:set(key)
		local harness, target = b.agent(), b.target()
		if key and harness and target then
			Invocations.load(harness, target)
		end
	end)
	-- What was last searched for: kept, so a reload doesn't search again.
	local searched = ui.state(`{b.key}_mention_searched`, nil :: string?)
	ui.effect(function()
		local text = b.text:get()
		local t = mentions.token(text, #text)
		if not t or t.trigger ~= "@" then
			return
		end
		local target: any = b.files()
		if not target then
			return
		end
		local key = `{target.chat or target.space or ""}:{t.query}`
		if key == searched:peek() then
			return
		end
		-- (The run before's timer went as this one began: an effect's run owns
		-- what it makes. Written once it searches, so a run that stopped it
		-- before then doesn't count it as searched.)
		ui.after(120, function()
			searched:set(key)
			files.search(target, t.query):after(function(found: any)
				if searched:peek() ~= key then
					return
				end
				local items = {}
				for _, f in found or {} do
					local name = string.match(f.path, "([^/]+)/?$") or f.path
					local folder = string.match(f.path, "^(.*)/[^/]+/?$")
					table.insert(items, {
						value = f.path,
						label = name,
						detail = folder,
						file = not f.dir,
						dir = f.dir,
						-- Sent as Zeron's file mention (screens/chat/shared).
						link = require("@app/screens/chat/shared").file_link(f.path, f.dir),
					})
				end
				sources:set { ["@"] = items, ["/"] = {}, ["$"] = {} }
			end, function()
				sources:set { ["@"] = {}, ["/"] = {}, ["$"] = {} }
			end)
		end)
	end)
	return {
		lists = lists,
		-- A command, skill or file picked: what its token stands for; one of
		-- Zeron's own runs (its token gone).
		on_mention = function(item: any, trigger: string, token: string)
			if item.zeron then
				b.run(item.zeron)
			elseif trigger == "@" then
				Invocations.remember(item, trigger, token)
			else
				Invocations.remember(item, trigger)
			end
		end,
		-- The agent's completion settings: `$` opens the skills only while "Use
		-- $ for skills" is on (else it's ordinary text).
		enabled = function(trigger: string): boolean
			if trigger == "$" then
				return prefs.completion(b.agent()).dollar
			end
			return true
		end,
		-- With "Separate / commands" off, `/` lists the skills too.
		empty = setmetatable({}, {
			__index = function(_, trigger: string): string?
				if trigger == "/" and not prefs.completion(b.agent()).separate then
					return ui.t("chat.composer.commands_or_skills"):get()
				end
				return nil
			end,
		}) :: any,
		-- A draft that is only one of Zeron's own commands (its token goes;
		-- anything carried stays).
		command_of = function(draft: string): string?
			return ZeronCommands.of_text(draft, (lists:peek() or {})["/"] or {})
		end,
	}
end

return M
