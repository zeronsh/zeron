-- The ACP registry's agents on the device Providers shows, as the host
-- keeps them (src/acp_registry.rs): every agent that speaks the Agent
-- Client Protocol, what adding one takes there, and adding, updating and
-- removing one. Scripts read `registry.now` and ask for what changes it.
-- Each agent carries all it shows, so a page keys its rows by `id` and they
-- update in place.
--
--   registry.now:get()   { state = "looking" | "ready" | "unavailable",
--                          offline, agents = { Agent } }
--   registry.load(refresh?)   looks again (and the first time)
--   registry.add(id)     a promise: the agent added (or updated) there;
--                        rejects with the reason ("offline", else the
--                        engine's words)
--   registry.remove(id)  a promise
--   registry.env_get(id) a promise of its environment there (`KEY=VALUE` a
--                        line); env_set(id, text) keeps it there (owner-
--                        only, never synced), rejecting with why a line
--                        isn't one
--   registry.add_own(name, command, env)  one of your own there: a promise
--                        of its harness id
--
-- An agent: { id, harness, name, version, about, by, license?, source?,
-- source_url?, website?, kind? ("binary" | "npx" | "uvx"; nil: no build
-- for the device), readiness ("ready" | "needs-node" | "needs-uv" |
-- "no-build"), host?, checksum? (a binary checked against the registry's
-- sha256, or not; nil for a package), built_in? (the harness Zeron drives
-- it with), added? (the version there), adding, delisted, own (one of
-- your own, `own.<slug>`), icon? (SVG) }.

local M = {}

export type Agent = {
	id: string,
	harness: string,
	name: string,
	version: string,
	about: string,
	by: string,
	license: string?,
	source: string?,
	source_url: string?,
	website: string?,
	kind: string?,
	readiness: string,
	host: string?,
	checksum: boolean?,
	built_in: string?,
	added: string?,
	adding: boolean,
	delisted: boolean,
	own: boolean?,
	icon: string?,
}
export type Now = { state: string, offline: boolean, agents: { Agent } }

M.now = ui.shared("acp_registry", { state = "looking", offline = false, agents = {} } :: Now)

function M.load(refresh: boolean?)
	ui.invoke("acp_registry.load", refresh == true)
end

function M.add(id: string): any
	return ui.call("acp_registry.add", id)
end

function M.remove(id: string): any
	return ui.call("acp_registry.remove", id)
end

function M.env_get(id: string): any
	return ui.call("acp_registry.env_get", id)
end

function M.env_set(id: string, env: string): any
	return ui.call("acp_registry.env_set", { id = id, env = env })
end

function M.add_own(name: string, command: string, env: string): any
	return ui.call("acp_registry.add_own", { name = name, command = command, env = env })
end

-- The agent by id, as it stands (nil: not in the list).
function M.find(id: string): Agent?
	for _, a in M.now:get().agents do
		if a.id == id then
			return a
		end
	end
	return nil
end

-- The registry's agent behind a harness (`acp.<id>`), if it is one.
function M.of_harness(harness: string): Agent?
	local id = string.match(harness, "^acp%.(.+)$")
	return if id then M.find(id) else nil
end

return M
