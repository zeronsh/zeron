-- The ACP registry's agents (account/registry), answered here as the host
-- would: no engine in a test host. Required by ui/settings_stub.luau, so
-- Settings → Providers → Add an agent builds headless; the tests drive it
-- (tests/add_agent.test.luau). Adding one puts it in this Mac's agents
-- (account/agents) as the engine would: a harness `acp.<id>`.
--
-- What was asked is in `registry_stub_calls` ("registry.add:<id>", …).
-- `ui.command`s change what the next call does:
--   registry_stub.fail_next     the next add fails with the engine's words
--   registry_stub.newer         the registry has a newer Gemini CLI
--   registry_stub.offline       the registry can't be reached (the copy shows)

local registry = require("@account/registry")
local agents = require("@account/agents")

local calls = ui.state("registry_stub_calls", {} :: { string })
local fail_next = ui.state("registry_stub_fail_next", false)

local GEMINI = [[<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24"><path fill="currentColor" d="M11.04 19.32Q12 21.51 12 24q0-2.49.93-4.68.96-2.19 2.58-3.81t3.81-2.55Q21.51 12 24 12q-2.49 0-4.68-.93a12.3 12.3 0 0 1-3.81-2.58 12.3 12.3 0 0 1-2.58-3.81Q12 2.49 12 0q0 2.49-.96 4.68-.93 2.19-2.55 3.81a12.3 12.3 0 0 1-3.81 2.58Q2.49 12 0 12q2.49 0 4.68.96 2.19.93 3.81 2.55t2.55 3.81"/></svg>]]
local KIMI = [[<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24"><path fill="currentColor" d="M19.514 7.342C19.711 7.862 19.814 8.413 19.816 8.969C19.816 12.494 15.998 14.697 12.946 12.934C11.529 12.116 10.657 10.605 10.657 8.969C10.657 5.646 14.085 3.431 17.115 4.793C15.26 3.373 12.99 2.607 10.655 2.612C4.772 2.614 0.005 7.381 0 13.265C0.002 19.148 4.772 23.918 10.655 23.92C16.538 23.916 21.306 19.147 21.31 13.265C21.312 11.155 20.687 9.095 19.514 7.342ZM14.841 4.666C14.841 8.191 18.657 10.395 21.709 8.632C23.127 7.814 24 6.302 24 4.666C24 1.14 20.184-1.061 17.13 0.699C15.714 1.519 14.841 3.03 14.841 4.666"/></svg>]]

local function agent(fields: any): registry.Agent
	local a = {
		harness = `acp.{fields.id}`,
		by = "",
		readiness = "ready",
		adding = false,
		delisted = false,
	}
	for k, v in fields do
		a[k] = v
	end
	return a :: any
end

-- The registry as this Mac sees it (a few of the real one's).
local model: { registry.Agent } = {
	agent { id = "claude-acp", name = "Claude Agent", by = "Anthropic", version = "0.99.1", about = "Anthropic's Claude, through its ACP wrapper.", kind = "npx", host = "npmjs.com", built_in = "claude-code" },
	agent { id = "fast-agent", name = "fast-agent", by = "fast-agent.ai", version = "0.10.1", about = "Code and build agents with multi-provider support.", kind = "uvx", readiness = "needs-uv", host = "pypi.org", license = "Apache-2.0" },
	agent {
		id = "gemini",
		name = "Gemini CLI",
		by = "Google",
		version = "0.66.0",
		about = "Google's official CLI for Gemini.",
		kind = "npx",
		host = "npmjs.com",
		license = "Apache-2.0",
		source = "github.com/google-gemini/gemini-cli",
		source_url = "https://github.com/google-gemini/gemini-cli",
		icon = GEMINI,
	},
	agent {
		id = "kimi",
		name = "Kimi CLI",
		by = "Moonshot AI",
		version = "1.52.0",
		about = "Moonshot AI's coding assistant.",
		kind = "binary",
		host = "github.com",
		checksum = true,
		license = "MIT",
		source = "github.com/MoonshotAI/kimi-cli",
		source_url = "https://github.com/MoonshotAI/kimi-cli",
		icon = KIMI,
	},
	agent { id = "poolside", name = "Poolside", by = "Poolside", version = "1.0.16", about = "Poolside's coding agent.", kind = "binary", host = "downloads.poolside.ai", checksum = false, license = "proprietary" },
}
local offline = false

local function publish()
	local list = {}
	for _, a in model do
		table.insert(list, table.clone(a))
	end
	registry.now:set({ state = "ready", offline = offline, agents = list } :: any)
end

local function find(id: string): any
	for _, a in model do
		if a.id == id then
			return a
		end
	end
	return nil
end

-- A promise already settled, as the host's.
local function settled(word: string?): any
	local p = {}
	function p.after(_self: any, ok: ((any) -> ())?, bad: ((any) -> ())?)
		if word then
			if bad then
				bad({ message = word })
			end
		elseif ok then
			ok(nil)
		end
	end
	return p
end

-- The added agent in this Mac's agents, as the engine lists it.
local function list_agent(a: any, on: boolean)
	local now = table.clone(agents.now:peek())
	local list = {}
	for _, one in now.list or {} do
		if one.id ~= a.harness then
			table.insert(list, one)
		end
	end
	if on then
		table.insert(list, {
			id = a.harness,
			name = a.name,
			registry = true,
			-- (As the update coordinator reports it: the version added, and
			-- the registry's when it's newer.)
			version = if a.added ~= "" then a.added else nil,
			latest = if a.added and a.added ~= "" and a.added ~= a.version then a.version else nil,
			icon = a.icon,
			installed = true,
			can_install = false,
			enabled = true,
			installing = false,
			login = "idle",
			accounts = {},
		})
	end
	now.list = list
	agents.now:set(now)
end

local stub = registry :: any
stub.load = function(_refresh: boolean?)
	calls:push("registry.load")
	publish()
end
stub.add = function(id: string): any
	calls:push(`registry.add:{id}`)
	local a = find(id)
	if not a then
		return settled("not in the registry")
	end
	if fail_next:peek() then
		fail_next:set(false)
		return settled("Kimi CLI's download doesn't match the registry's checksum")
	end
	a.added = a.version
	publish()
	list_agent(a, true)
	return settled(nil)
end
-- Each agent's environment, as the device keeps it.
local envs: { [string]: string } = {}
stub.env_get = function(id: string): any
	calls:push(`registry.env_get:{id}`)
	local p = {}
	function p.after(_self: any, ok: ((any) -> ())?)
		if ok then
			ok(envs[id] or "")
		end
	end
	return p
end
stub.env_set = function(id: string, env: string): any
	calls:push(`registry.env_set:{id}:{env}`)
	if string.find(env, "^%s*[^=%s#][^=]*$") or string.find(env, "\n[^=#\n]+$") then
		return settled("line 1 isn't KEY=VALUE")
	end
	envs[id] = env
	local p = {}
	function p.after(_self: any, ok: ((any) -> ())?)
		if ok then
			ok(env)
		end
	end
	return p
end
-- One of your own: listed, and among this Mac's agents.
stub.add_own = function(name: string, command: string, env: string): any
	calls:push(`registry.add_own:{name}:{command}`)
	if string.find(command, "nowhere", 1, true) then
		return settled("/nowhere/agent isn't on this device")
	end
	local id = "own." .. string.gsub(string.lower(name), "[^%w]+", "-")
	local a = agent { id = id, name = name, version = "", about = "", own = true, added = "" }
	table.insert(model, a)
	envs[id] = env
	publish()
	list_agent(a, true)
	return settled(nil)
end
stub.remove = function(id: string): any
	calls:push(`registry.remove:{id}`)
	local a = find(id)
	if a then
		a.added = nil
		publish()
		list_agent(a, false)
	end
	return settled(nil)
end

-- A registry agent's sign-in: its own, under way, asking for a device code
-- (as the engine's poll says it); other agents' as the settings stub has
-- them.
local agents_stub = agents :: any
local sign_in_before = agents_stub.sign_in
agents_stub.sign_in = function(id: string)
	if string.sub(id, 1, 4) ~= "acp." then
		if sign_in_before then
			sign_in_before(id)
		end
		return
	end
	calls:push(`agents.sign_in:{id}`)
	local now = table.clone(agents.now:peek())
	local list = {}
	for _, one in now.list or {} do
		local a = table.clone(one)
		if a.id == id then
			a.login = "waiting"
			a.login_note = "Your code is WDJB-MJHT. Check it matches on the page that opened, then confirm."
			a.login_page = "https://auth.example.com/device?user_code=WDJB-MJHT"
			a.login_code = "WDJB-MJHT"
		end
		table.insert(list, a)
	end
	now.list = list
	agents.now:set(now)
end

-- A registry agent's sign-in that asks for typed input (as the engine's
-- poll says it): failed, with the command a terminal runs.
ui.command {
	id = "registry_stub.needs_terminal",
	title = "Gemini CLI's sign-in needs a terminal",
	run = function()
		local now = table.clone(agents.now:peek())
		local list = {}
		for _, one in now.list or {} do
			local a = table.clone(one)
			if a.id == "acp.gemini" then
				a.login = "failed"
				a.login_note = "This sign-in asks for something only a terminal can answer."
				-- (A capture runs a stand-in that asks for a key, as such a
				-- sign-in would.)
				a.login_command = if ui.state("registry_stub_real_terminal", false):peek()
					then "printf 'Gemini CLI sign-in\\n\\nPaste your Gemini API key: '; read -r key; printf '\\nSigned in.\\n'"
					else "/agents/gemini/node_modules/.bin/gemini --acp login"
			end
			table.insert(list, a)
		end
		now.list = list
		agents.now:set(now)
	end,
}

-- The sign-in terminal (screens/settings/sign_in_terminal) on stand-ins for
-- the host's terminal and zgx's: what it opened, typed and closed, in
-- `registry_stub_calls`. Every other terminal as it was.
local tapi = require("@app/screens/panel/terminal_api") :: any
-- (On, the real host's terminal stands: a capture with an engine running.)
local real_terminal = ui.state("registry_stub_real_terminal", false)
local function ours(target: any): boolean
	return not real_terminal:peek()
		and type(target) == "table"
		and type(target.chat) == "string"
		and string.sub(target.chat, 1, 12) == "acp-sign-in:"
end
local function done(value: any): any
	local p = {}
	function p.after(_self: any, ok: ((any) -> ())?)
		if ok then
			ok(value)
		end
	end
	return p
end
local start_before, claimed_before, write_before, close_before, term_before, view_before =
	tapi.start, tapi.claimed, tapi.write, tapi.close, tapi.term, tapi.view
tapi.start = function(target: any): any
	if not ours(target) then
		return start_before(target)
	end
	calls:push(`terminal.start:{target.chat}`)
	return done({ command = "bridge sign-in", claim = "sign-in-claim" })
end
tapi.claimed = function(claim: string): any
	if claim ~= "sign-in-claim" or real_terminal:peek() then
		return claimed_before(claim)
	end
	return done("sign-in-term")
end
tapi.write = function(id: string, device: string?, text: string): any
	if id ~= "sign-in-term" or real_terminal:peek() then
		return write_before(id, device, text)
	end
	calls:push(`terminal.write:{text}`)
	return done(nil)
end
tapi.close = function(id: string, device: string?): any
	if id ~= "sign-in-term" or real_terminal:peek() then
		return close_before(id, device)
	end
	calls:push("terminal.close")
	return done(nil)
end
local sessions: { [string]: any } = {}
tapi.term = function(key: string, options: any?): any
	if string.sub(key, 1, 12) ~= "acp-sign-in:" or real_terminal:peek() then
		return term_before(key, options)
	end
	if not sessions[key] and options then
		sessions[key] = { close = function() end, focus = function() end }
	end
	return sessions[key]
end
tapi.view = function(props: any): Node
	if real_terminal:peek() or not (props.term and props.term.focus and sessions[next(sessions) or ""] == props.term) then
		return view_before(props)
	end
	return ui.column { id = "sign-in-terminal", label = "Terminal", grow = 1, ui.text { "terminal", literal = true } }
end

-- A registry agent's sign-in as it says it is: Gemini CLI signed in
-- already (as whom, and offering a sign-out), and signed out by it.
local function set_sign_in(id: string, fields: { [string]: any })
	local now = table.clone(agents.now:peek())
	local list = {}
	for _, one in now.list or {} do
		local a = table.clone(one)
		if a.id == id then
			for k, v in fields do
				a[k] = v
			end
		end
		table.insert(list, a)
	end
	now.list = list
	agents.now:set(now)
end
ui.command {
	id = "registry_stub.signed_in",
	title = "Gemini CLI says it's signed in",
	run = function()
		set_sign_in("acp.gemini", { signed_in = true, signed_in_as = "ada@example.com", can_sign_out = true })
	end,
}
-- The ways Gemini CLI offers to sign in, as it says them: its own, another
-- it signs in itself, and a key it reads from its environment.
ui.command {
	id = "registry_stub.methods",
	title = "Gemini CLI offers its ways to sign in",
	run = function()
		set_sign_in("acp.gemini", {
			signed_in = false,
			login_methods = {
				{ id = "oauth-personal", name = "Log in with Google", kind = "agent" },
				{ id = "vertex-ai", name = "Vertex AI", description = "Use your Google Cloud project.", kind = "agent" },
				{
					id = "gemini-api-key",
					name = "Gemini API key",
					kind = "env_var",
					var = "GEMINI_API_KEY",
					link = "https://aistudio.google.com/apikey",
				},
			},
		})
	end,
}
agents_stub.sign_in_with = function(id: string, method: string)
	calls:push(`agents.sign_in_with:{id}:{method}`)
end
agents_stub.set_key = function(id: string, var: string, value: string)
	-- (The key itself is never recorded.)
	calls:push(`agents.set_key:{id}:{var}:{#value}`)
end
agents_stub.sign_out = function(id: string)
	calls:push(`agents.sign_out:{id}`)
	set_sign_in(id, { signed_in = false, signed_in_as = false, can_sign_out = true })
end

-- The sign-in's page, opened again.
agents_stub.open_page = function(id: string)
	calls:push(`agents.open_page:{id}`)
end

-- A failed sign-in let go, as the host does.
local dismiss_before = agents_stub.dismiss
agents_stub.dismiss = function(id: string)
	if string.sub(id, 1, 4) ~= "acp." then
		if dismiss_before then
			dismiss_before(id)
		end
		return
	end
	calls:push(`agents.dismiss:{id}`)
	local now = table.clone(agents.now:peek())
	local list = {}
	for _, one in now.list or {} do
		local a = table.clone(one)
		if a.id == id then
			a.login = "idle"
			a.login_note = nil
			a.login_command = nil
		end
		table.insert(list, a)
	end
	now.list = list
	agents.now:set(now)
end

-- The sign-in terminal on its own (a capture with an engine running, whose
-- agents aren't these).
ui.command {
	id = "registry_stub.sign_in_terminal",
	title = "Open Gemini CLI's sign-in terminal",
	run = function()
		require("@app/screens/settings/sign_in_terminal").show {
			id = "acp.gemini",
			name = "Gemini CLI",
			command = "printf 'Gemini CLI sign-in\\n\\nPaste your Gemini API key: '; read -r key; printf '\\nSigned in.\\n'",
			device = nil,
			device_name = "this Mac",
		}
	end,
}

ui.command {
	id = "registry_stub.fail_next",
	title = "The next add fails",
	run = function()
		fail_next:set(true)
	end,
}
ui.command {
	id = "registry_stub.newer",
	title = "The registry has a newer Gemini CLI",
	run = function()
		local g = find("gemini")
		g.version = "0.67.0"
		publish()
		if g.added then
			list_agent(g, true)
		end
	end,
}
ui.command {
	id = "registry_stub.offline",
	title = "The registry can't be reached",
	run = function()
		offline = true
		publish()
	end,
}

return {}
