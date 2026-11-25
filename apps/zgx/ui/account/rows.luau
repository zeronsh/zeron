-- Rows by id, read one at a time: a reader of one chat (or crew member,
-- or board item) runs again when that row changes, not on every push of
-- the whole sessions or orchestration value.
--
--   local rows = require("@account/rows")
--   rows.chat(id)      the chat as data.chats has it (archived ones
--                      marked `archived`), nil if none
--   rows.member(id)    its live crew row (not released), by chat id
--   rows.item(id)      a board item by id
--   rows.orchestrator(id)  an orchestrator by id
--   rows.name(id)      a thread's or orchestrator's name (ds/orch_name)
--   rows.hue(id)       the hue a chat wears (data.hue_of's rule)
--   rows.title(id)     an orchestrator's name, else the chat's title
--                      (data.name_of's rule)
--   rows.ask(chat)     the chat's open ask (its latest; a quiet one, a
--                      chase, is none), as data.open_asks has it
--
-- One effect a source, owned by the script, walks each push once and
-- compares rows by identity (the bridge keeps an unchanged row's table:
-- its state reconciles, zeron/push-diffs): a row that changed bumps that
-- id's version, a small state its readers follow; the row itself sits in
-- a plain table. So a push touching one row wakes that row's readers and
-- this walk, nothing else.

local sessions = require("@account/sessions")
local orchestration = require("@account/orchestration")
local Name = require("@ds/orch_name")

local M = {}

type Index = { rows: { [string]: any }, versions: { [string]: State<number> }, name: string }

local function index(name: string): Index
	return { rows = {}, versions = {}, name = name }
end

local function version(ix: Index, id: string): State<number>
	local v = ix.versions[id]
	if not v then
		v = ui.state(`rows_{ix.name}:{id}`, 0)
		ix.versions[id] = v
	end
	return v
end

local function read(ix: Index, id: string?): any
	if id == nil then
		return nil
	end
	version(ix, id):get()
	return ix.rows[id]
end

-- `seen` (id → row) taken in: each row not the one held bumps its id.
-- Whether two rows say the same (their fields, and the tables in them a few
-- levels down): a push that hands back an unchanged row as a new table (a
-- whole value written again, its rows read anew) wakes nobody.
local function same(a: any, b: any, depth: number): boolean
	if rawequal(a, b) then
		return true
	end
	if type(a) ~= "table" or type(b) ~= "table" or depth <= 0 then
		return false
	end
	for k, v in a do
		if not same(v, b[k], depth - 1) then
			return false
		end
	end
	for k in b do
		if a[k] == nil then
			return false
		end
	end
	return true
end
M.same = same

local function take(ix: Index, seen: { [string]: any })
	local bump: { string } = {}
	for id, row in seen do
		local was = ix.rows[id]
		if was ~= row then
			-- (The same row as a new table: the one held stays, nobody woken.)
			if was ~= nil and same(was, row, 4) then
				continue
			end
			ix.rows[id] = row
			table.insert(bump, id)
		end
	end
	for id in ix.rows do
		if seen[id] == nil then
			ix.rows[id] = nil
			table.insert(bump, id)
		end
	end
	if #bump == 0 then
		return
	end
	ui.batch(function()
		for _, id in bump do
			local v = ix.versions[id]
			if v then
				v:set(v:peek() + 1)
			end
		end
	end)
end

local chats = index("chat")
local members = index("member")
local items = index("item")
local asks = index("ask")
local leads = index("orchestrator")
-- Each chat's orchestrator by its crew rows, released or not (the first
-- one listing it, as data.hue_of looks it up).
local crew_lead = index("crew_lead")

-- An archived row whose own field doesn't say so is marked in a copy, the
-- same copy while the row is the same (data.chats's rule, kept stable).
local archived_copy: { [any]: any } = setmetatable({}, { __mode = "k" }) :: any

local function chats_of(now: any): { [string]: any }
	local out: { [string]: any } = {}
	if not now then
		return out
	end
	for _, c in now.archived or {} do
		if c.archived then
			out[c.id] = c
		else
			local a = archived_copy[c]
			if not a then
				a = table.clone(c)
				a.archived = true
				archived_copy[c] = a
			end
			out[c.id] = a
		end
	end
	for _, kids in now.children or {} do
		for _, c in kids do
			out[c.id] = c
		end
	end
	for _, c in now.chats or {} do
		out[c.id] = c
	end
	return out
end

local function members_of(now: any): { [string]: any }
	local out: { [string]: any } = {}
	for _, m in (now and now.crew) or {} do
		if not m.released then
			out[m.chat_id] = m
		end
	end
	return out
end

local function asks_of(now: any): { [string]: any }
	local out: { [string]: any } = {}
	for _, a in (now and now.asks) or {} do
		if a.state == "open" and a.from_chat and not a.quiet then
			local had = out[a.from_chat]
			if not had or (a.created or 0) > (had.created or 0) then
				out[a.from_chat] = a
			end
		end
	end
	return out
end

local function leads_of(now: any): { [string]: any }
	local out: { [string]: any } = {}
	for _, o in (now and now.orchestrators) or {} do
		out[o.id] = o
	end
	return out
end

local function crew_leads_of(now: any): { [string]: any }
	local out: { [string]: any } = {}
	for _, m in (now and now.crew) or {} do
		if m.chat_id and out[m.chat_id] == nil then
			out[m.chat_id] = m.orchestrator_id
		end
	end
	return out
end

local function items_of(now: any): { [string]: any }
	local out: { [string]: any } = {}
	for _, it in (now and now.items) or {} do
		out[it.id] = it
	end
	return out
end

-- (Filled at once, so a reader built before the first walk finds its row.)
take(chats, chats_of(sessions.now:peek()))
do
	local now = orchestration.now:peek()
	take(members, members_of(now))
	take(items, items_of(now))
	take(asks, asks_of(now))
	take(leads, leads_of(now))
	take(crew_lead, crew_leads_of(now))
end

ui.effect(function()
	take(chats, chats_of(sessions.now:get()))
end)
ui.effect(function()
	local now = orchestration.now:get()
	take(members, members_of(now))
	take(items, items_of(now))
	take(asks, asks_of(now))
	take(leads, leads_of(now))
	take(crew_lead, crew_leads_of(now))
end)

function M.chat(id: string?): any
	return read(chats, id)
end
function M.member(id: string?): any
	return read(members, id)
end
function M.item(id: string?): any
	return read(items, id)
end
function M.orchestrator(id: string?): any
	return read(leads, id)
end
-- A thread's name as ds/orch_name says it (an orchestrator's own name; a
-- crew thread's title, else its role, else its card), from its rows alone.
function M.name(id: string): string
	local o = read(leads, id)
	local chat = read(chats, id)
	if o then
		return Name.from({ orchestrators = { [id] = o }, crew = {}, items = {} }, { [id] = chat }, id)
	end
	local m = read(members, id)
	local item = if m and m.item_id then read(items, m.item_id) else nil
	return Name.thread(chat, m, item, id)
end
function M.hue(id: string?): number?
	if not id then
		return nil
	end
	local o = read(leads, id)
	if o then
		return o.hue
	end
	local lead = read(crew_lead, id)
	if lead then
		local lo = read(leads, lead)
		if lo then
			return lo.hue
		end
	end
	-- (A chat under an orchestrator's chat, not (yet) in its crew.)
	local c = read(chats, id)
	if c and c.parent then
		local po = read(leads, c.parent)
		if po then
			return po.hue
		end
	end
	return nil
end
function M.title(id: string?): string?
	if not id then
		return nil
	end
	local o = read(leads, id)
	if o then
		return o.name
	end
	local c = read(chats, id)
	return if c then c.title else nil
end
function M.ask(chat: string?): any
	return read(asks, chat)
end

-- (For tests: how many versions each index holds.)
function M.tracked(): { chat: number, member: number, item: number, ask: number, orchestrator: number }
	local function n(ix: Index): number
		local k = 0
		for _ in ix.versions do
			k += 1
		end
		return k
	end
	return { chat = n(chats), member = n(members), item = n(items), ask = n(asks), orchestrator = n(leads) }
end

return M
