-- What the composer's `/` and `$` offer: the agent's slash commands and
-- skills for where it works (a chat's folder, or a new chat's project, on
-- its device), as Zeron's composer finds them (crates/ui/src/composer.rs):
-- read from the engine by the host (`chat.invocations`, src/completion.rs),
-- commands first in the agent's order, then its enabled skills, each with
-- what it does.
--
-- Each list is kept by agent and place (kept state: a reload reads none
-- again) and read again each time a `/` or `$` opens afresh, the last list
-- showing meanwhile, as Zeron's does.
--
-- A command or skill picked lands in the draft as its token (`/review`,
-- `$code-review`); what it stands for is remembered, and the send writes
-- each such token as its canonical reference (`[/review](zeron-invoke:…)`),
-- which the engine gives each agent in its own form. A token typed by hand,
-- not picked, stays the words it is (Zeron's: only references made by
-- completion are read).

local M = {}

export type Row = { kind: "command" | "skill", name: string, label: string, detail: string, link: string }
export type Catalog = { state: "loading" | "ready" | "failed", rows: { Row }, warning: string?, error: string? }
export type Target = { chat: string?, space: string?, device: string?, path: string? }

-- Every list, by `harness|device|place`.
local CATALOGS = ui.state("chat_invocations", {} :: { [string]: Catalog })
-- The tokens picked, and the reference each stands for.
local PICKS = ui.state("chat_invocation_picks", {} :: { [string]: string })

-- How a list is read: the host's action (a test puts its own here).
M.read = function(harness: string, target: Target): any
	return ui.call("chat.invocations", { harness = harness, target = target })
end

function M.key(harness: string?, target: Target?): string?
	if not harness or not target then
		return nil
	end
	local place = target.chat or target.space or target.path
	if not place then
		return nil
	end
	return `{harness}|{target.device or "here"}|{place}`
end

local function put(key: string, catalog: Catalog)
	local now = table.clone(CATALOGS:peek())
	now[key] = catalog
	CATALOGS:set(now)
end

-- Reads the list for `harness` at `target` (again, if it has been), the
-- last one showing until it lands.
function M.load(harness: string, target: Target)
	local key = M.key(harness, target)
	if not key then
		return
	end
	local was = CATALOGS:peek()[key]
	if was and was.state == "loading" then
		return
	end
	put(key, { state = "loading", rows = if was then was.rows else {}, warning = if was then was.warning else nil })
	local ok, promise = pcall(M.read, harness, target)
	if not ok or not promise then
		put(key, { state = "failed", rows = if was then was.rows else {}, error = ui.t("chat.invocations.couldnt_list"):get() })
		return
	end
	promise:after(function(got: any)
		local rows = if type(got) == "table" and type(got.rows) == "table" then got.rows else {}
		put(key, { state = "ready", rows = rows, warning = if type(got) == "table" then got.warning else nil })
	end, function(err: any)
		put(key, { state = "failed", rows = if was then was.rows else {}, error = tostring(err) })
	end)
end

-- The list kept for `key` (followed), or nil.
function M.catalog(key: string?): Catalog?
	if not key then
		return nil
	end
	return CATALOGS:get()[key]
end

-- The composer's rows for a trigger's list (`/` the commands, `$` the
-- skills): nil while the first read is on its way (the list shows its
-- loading rows).
function M.items(catalog: Catalog?, kind: "command" | "skill"): { any }?
	if not catalog or (catalog.state == "loading" and #catalog.rows == 0) then
		return nil
	end
	local out = {}
	for _, r in catalog.rows do
		if r.kind == kind then
			table.insert(out, {
				value = r.name,
				label = r.label,
				detail = if r.detail ~= "" then r.detail else nil,
				icon = if kind == "skill" then "magic-stick" else "command",
				link = r.link,
			})
		end
	end
	return out
end

-- A row picked (the composer's `on_mention`: the item, then the trigger it
-- was typed at): its token in the draft stands for its reference from now
-- on.
-- (`token`: what it is in the draft, when that isn't the trigger and its
-- value: a file's shortest name.)
function M.remember(item: any, trigger: string, token: string?)
	if type(item) ~= "table" or type(item.link) ~= "string" or type(item.value) ~= "string" then
		return
	end
	local now = table.clone(PICKS:peek())
	now[token or (trigger .. item.value)] = item.link
	PICKS:set(now)
end

-- Where a token may stand: at the start, after a space or an opening
-- bracket (as a trigger starts one), and up to a character that can't be
-- part of a name.
local BEFORE = "[%s%(%[{>]"
local NAME = "[%w%./_:%-]"

-- `text` with each picked token written as its reference.
function M.encode(text: string): string
	local picks = PICKS:peek()
	-- Longest first, so `/review` doesn't take the start of `/review-all`
	-- (the boundary check would refuse it anyway).
	local tokens: { string } = {}
	for token in picks do
		table.insert(tokens, token)
	end
	table.sort(tokens, function(a: string, b: string): boolean
		return #a > #b
	end)
	for _, token in tokens do
		local link = picks[token]
		local out = {}
		local at = 1
		while true do
			local s, found = string.find(text, token, at, true)
			if not s or not found then
				break
			end
			local e: number = found
			local prev = if s > 1 then string.sub(text, s - 1, s - 1) else ""
			local next_char = string.sub(text, e + 1, e + 1)
			-- (Not a reference's own label: `[/review](…)`.)
			local labelled = prev == "[" and string.sub(text, e + 1, e + 2) == "]("
			if not labelled and (prev == "" or string.match(prev, BEFORE)) and (next_char == "" or not string.match(next_char, NAME)) then
				table.insert(out, string.sub(text, at, s - 1))
				table.insert(out, link)
			else
				table.insert(out, string.sub(text, at, e))
			end
			at = e + 1
		end
		table.insert(out, string.sub(text, at))
		text = table.concat(out)
	end
	return text
end

-- Forgets every list and pick (a test's fresh start).
function M.forget()
	CATALOGS:set({})
	PICKS:set({})
end

return M
