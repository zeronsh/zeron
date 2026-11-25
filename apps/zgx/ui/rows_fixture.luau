-- Rows read one by one (account/rows), for tests/rows_keyed.test.luau:
-- forty chats, a reader of each that counts its runs (ds/orch_perf_trace,
-- `row.<id>`), and the pushes a session list gets: one chat's status, one
-- renamed, one added, one removed, each the whole list written again (so
-- every row comes back a new table, as a whole value written does).
local sessions = require("@account/sessions")
local rows = require("@account/rows")
local trace = require("@ds/orch_perf_trace")

local function list(): { any }
	local out = {}
	for i = 1, 40 do
		table.insert(out, { id = `c{i}`, title = `Chat {i}`, status = "done", device = "d", updated = 1000 - i, todo = { done = 1, total = 3 } })
	end
	return out
end
sessions.now:set({ state = "ready", here = "d", devices = {}, projects = {}, chats = list(), archived = {}, children = {},
	places = { pinned = {}, sections = {} }, places_ready = true } :: any)

local ids = ui.state("rows_fixture_ids", (function(): { string }
	local out = {}
	for i = 1, 40 do
		table.insert(out, `c{i}`)
	end
	return out
end)())

local function push(change: (list: { any }) -> { any })
	local n: any = table.clone(sessions.now:peek())
	local copy = {}
	for i, c in n.chats do
		copy[i] = table.clone(c)
	end
	n.chats = change(copy)
	sessions.now:set(n)
end

ui.command { id = "rows_status", title = "Rows: c7's status", run = function()
	push(function(l)
		for _, c in l do
			if c.id == "c7" then
				c.status = if c.status == "done" then "working" else "done"
			end
		end
		return l
	end)
end }
ui.command { id = "rows_rename", title = "Rows: c13 renamed", run = function()
	push(function(l)
		for _, c in l do
			if c.id == "c13" then
				c.title = "Renamed"
			end
		end
		return l
	end)
end }
ui.command { id = "rows_add", title = "Rows: c41 added", run = function()
	push(function(l)
		table.insert(l, 1, { id = "c41", title = "Chat 41", status = "working", device = "d", updated = 2000 })
		return l
	end)
	local now = table.clone(ids:peek())
	table.insert(now, "c41")
	ids:set(now)
end }
ui.command { id = "rows_remove", title = "Rows: c21 removed", run = function()
	push(function(l)
		local out = {}
		for _, c in l do
			if c.id ~= "c21" then
				table.insert(out, c)
			end
		end
		return out
	end)
end }
ui.command { id = "rows_same", title = "Rows: the same list again", run = function()
	push(function(l)
		return l
	end)
end }
local counts = ui.state("rows_counts", {} :: any)
ui.command { id = "rows_read", title = "Rows: read the counts", run = function()
	counts:set(table.clone(trace.counts))
end }
ui.command { id = "rows_reset", title = "Rows: reset the counts", run = function()
	trace.reset()
end }

return ui.column {
	id = "rows-fixture",
	ui.each(ids, function(id: string): Node
		return ui.text {
			class = "rows-fixture-row",
			function(): string
				trace.hit(`row.{id}`)
				local c = rows.chat(id)
				if not c then
					return `{id}: gone`
				end
				return `{id}: {c.title} {c.status}`
			end,
		}
	end),
}
