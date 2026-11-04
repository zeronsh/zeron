-- Split chats: chats laid out together in the page, side by side and above
-- one another, one of them focused. The focused one is the route's session:
-- the whole chat page (its composer, its panel at the right); the others
-- show their transcripts and composers beside it, dimmed, each watched by
-- the host in a lane slot of its own (screens/chat/lanes). A click on one
-- focuses it: the route moves to it, and the panel with it.
--
-- A split is a tree: a chat (`leaf`), or a `row` (side by side) or `col`
-- (one above another) of splits, each of a node's parts as large as the
-- others. Splits are kept across launches; a chat is in one at most. A
-- split shows while one of its chats is open: opening a chat that's in no
-- split shows it alone, and opening one of a split's again brings it back.
--
--   split.shown:get()       the split showing now (its tree), or nil
--   split.members:get()     its chats, in reading order ({} for none)
--   split.focused:get()     the focused chat (the route's session), or nil
--   split.frames:get()      each chat's box in the page ({ [id]: Box }),
--                           as edges that don't depend on the page's size
--   split.place(props, box) a node placed at `box()` by its insets
--   split.can_take(id)      whether a drop of `id` can join what shows
--   split.drop(id, target, side, focus)  `id` put at `target`'s side
--                           ("left", "right", "top" or "bottom"), moved
--                           there if it's in the split already; focused
--                           when `focus`
--   split.close(id)         taken out of its split (its neighbour focused
--                           if it was); a split of one is no split
--   split.dragging          a chat being dragged (the sidebar's rows, a
--                           pane's head): the panes' drop zones show then
--   split.MAX               the most chats in a split
--
-- Resizing the window or sliding the sidebar runs none of this: a box's
-- edges are each a percentage of the page plus points (zgx's terminal
-- example's way), which zgx resolves at every size; they're worked out
-- again only when the split's shape changes.

local route = require("@app/screens/shell/route")
local sessions = require("@account/sessions")

local M = {}

M.MAX = 6
-- The window's gap between the chats (`zeron.inset.gap`, ds/tokens).
M.GAP = (ui.theme.get("zeron.inset.gap") :: number?) or 4

export type Node = { kind: string, id: string?, children: { Node }? }
-- An edge: `p` of the page's length, plus `q` points.
export type Edge = { p: number, q: number }
export type Box = { left: Edge, right: Edge, top: Edge, bottom: Edge }

-- Every split, kept across launches. (Its key is the one the Inset
-- template kept them under before this was Zeron's, so the splits of
-- anyone who used it are still theirs, with it or without.)
local trees = ui.persist("inset_split_trees", {} :: { Node })

-- A chat being dragged (shared with the sidebar's rows, which set it).
M.dragging = ui.shared("split.dragging", false)

-- The ids of the chats in the split showing, for the sidebar (its rows
-- strong for each) and the panel (one for them all): set below as the
-- split showing changes.
M.grouped = ui.shared("split.grouped", {} :: { string })

local function leaf(id: string): Node
	return { kind = "leaf", id = id }
end

local function leaves(node: Node?, out: { string }?): { string }
	local list = out or {}
	if not node then
		return list
	end
	if node.kind == "leaf" then
		table.insert(list, node.id :: string)
	else
		for _, c in node.children or {} do
			leaves(c, list)
		end
	end
	return list
end

-- A tree tidied: a part with none left goes, a part of one is that one,
-- and a row in a row (a column in a column) is its parts in its place.
local function normalize(node: Node?): Node?
	if not node then
		return nil
	end
	if node.kind == "leaf" then
		return node
	end
	local kids: { Node } = {}
	for _, c in node.children or {} do
		local n = normalize(c)
		if n then
			if n.kind == node.kind then
				for _, g in n.children or {} do
					table.insert(kids, g)
				end
			else
				table.insert(kids, n)
			end
		end
	end
	if #kids == 0 then
		return nil
	elseif #kids == 1 then
		return kids[1]
	end
	return { kind = node.kind, children = kids }
end

-- The tree less the chat `id`, tidied.
local function without(node: Node?, id: string): Node?
	if not node then
		return nil
	end
	if node.kind == "leaf" then
		return if node.id == id then nil else node
	end
	local kids: { Node } = {}
	for _, c in node.children or {} do
		local n = without(c, id)
		if n then
			table.insert(kids, n)
		end
	end
	return normalize({ kind = node.kind, children = kids })
end

-- The tree with `id` put at `target`'s side: beside it in its row (above
-- or below it in its column) when it's in one that way, else `target`
-- split in two that way.
local function insert(node: Node, target: string, id: string, side: string): Node?
	local want = if side == "left" or side == "right" then "row" else "col"
	local before = side == "left" or side == "top"
	local function walk(n: Node): Node
		if n.kind == "leaf" then
			if n.id == target then
				return { kind = want, children = if before then { leaf(id), n } else { n, leaf(id) } }
			end
			return n
		end
		if n.kind == want then
			for i, c in n.children or {} do
				if c.kind == "leaf" and c.id == target then
					local kids = table.clone(n.children :: { Node })
					table.insert(kids, if before then i else i + 1, leaf(id))
					return { kind = n.kind, children = kids }
				end
			end
		end
		local kids: { Node } = {}
		for _, c in n.children or {} do
			table.insert(kids, walk(c))
		end
		return { kind = n.kind, children = kids }
	end
	return normalize(walk(node))
end

-- Two values the same, all the way down.
local function same(a: any, b: any): boolean
	if type(a) ~= "table" or type(b) ~= "table" then
		return a == b
	end
	for k, v in a do
		if not same(v, b[k]) then
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

-- A computed that tells only a real change: `f`'s answer the same as the
-- last, what reads it isn't woken. (The sessions list changes as any chat
-- streams; what's worked out from it here rarely does.)
local function steady<T>(f: () -> T): any
	local last: any = nil
	return ui.computed(function(): T
		local next = f()
		if last ~= nil and same(last, next) then
			return last
		end
		last = next
		return next
	end)
end

local function edge(p: number, q: number): Edge
	return { p = p, q = q }
end
M.WHOLE = { left = edge(0, 0), right = edge(1, 0), top = edge(0, 0), bottom = edge(1, 0) } :: Box

-- Each chat's box in the page, `gap` points apart: a node's parts share
-- its span less the gaps between them equally. Independent of the page's
-- size (see `Edge`).
local function frames_of(tree: Node?, gap: number): { [string]: Box }
	local out: { [string]: Box } = {}
	local function place(n: Node, box: Box)
		if n.kind == "leaf" then
			out[n.id :: string] = box
			return
		end
		local kids = n.children or {}
		local count = #kids
		local row = n.kind == "row"
		local first = if row then box.left else box.top
		local last = if row then box.right else box.bottom
		-- Each part's length: the span less the gaps, shared.
		local p = (last.p - first.p) / count
		local q = (last.q - first.q - (count - 1) * gap) / count
		for i, c in kids do
			local k = i - 1
			local from = edge(first.p + p * k, first.q + (q + gap) * k)
			local to = edge(from.p + p, from.q + q)
			place(
				c,
				if row
					then { left = from, right = to, top = box.top, bottom = box.bottom }
					else { left = box.left, right = box.right, top = from, bottom = to }
			)
		end
	end
	if tree then
		place(tree, M.WHOLE)
	end
	return out
end

-- `props` placed at `box()`: absolute, its insets a percentage of what
-- it's in from each side plus points (margins), for zgx to place at every
-- size, its edges on whole points (`snap`) so a 1-point edge stays crisp.
-- `box` is read only as the split's shape changes.
local WHY = "a split's edge: a share of the page, plus points"
function M.place(props: { [any]: any }, box: () -> Box)
	local function percent(p: number): string
		return `{p * 100}%`
	end
	props.absolute = true
	props.snap = true
	props.left = function(): string
		return percent(box().left.p)
	end
	props.ml = function(): number
		return ui.raw(box().left.q, WHY)
	end
	props.right = function(): string
		return percent(1 - box().right.p)
	end
	props.mr = function(): number
		return ui.raw(-box().right.q, WHY)
	end
	props.top = function(): string
		return percent(box().top.p)
	end
	props.mt = function(): number
		return ui.raw(box().top.q, WHY)
	end
	props.bottom = function(): string
		return percent(1 - box().bottom.p)
	end
	props.mb = function(): number
		return ui.raw(-box().bottom.q, WHY)
	end
end

-- Whether a box meets the page's top at its left end, and at its right.
function M.top_left(box: Box): boolean
	return box.top.p == 0 and box.top.q == 0 and box.left.p == 0 and box.left.q == 0
end
function M.top_right(box: Box): boolean
	return box.top.p == 0 and box.top.q == 0 and box.right.p == 1 and box.right.q == 0
end

-- The tree's own workings, for any split of things (the panel's tabs too:
-- screens/panel), and its boxes.
M.tree = { leaf = leaf, leaves = leaves, normalize = normalize, without = without, insert = insert, frames = frames_of, steady = steady }

local function tree_index(id: string, list: { Node }): number?
	for i, t in list do
		if table.find(leaves(t), id) then
			return i
		end
	end
	return nil
end

-- The chats there are (not deleted): archived ones stay. (Read only for a
-- split's chats, and only as the sessions list changes.)
local known = steady(function(): { [string]: boolean }?
	local now = sessions.now:get()
	if now.state ~= "ready" then
		return nil
	end
	local out: { [string]: boolean } = {}
	for _, c in now.chats do
		out[c.id] = true
	end
	for _, c in now.archived or {} do
		out[c.id] = true
	end
	return out
end)

M.focused = ui.computed(function(): string?
	local c = route.chat:get()
	return if c.name == "session" then c.session else nil
end)

M.shown = steady(function(): Node?
	local id = M.focused:get()
	if not id then
		return nil
	end
	local list = trees:get()
	local i = tree_index(id, list)
	if not i then
		return nil
	end
	-- (Its chats deleted since are left out.)
	local t: Node? = list[i]
	local there = known:get()
	if there then
		for _, member in leaves(t) do
			if member ~= id and not there[member] then
				t = without(t, member)
			end
		end
	end
	return if t and #leaves(t) >= 2 then t else nil
end)

M.members = steady(function(): { string }
	return leaves(M.shown:get())
end)

M.frames = ui.computed(function(): { [string]: Box }
	return frames_of(M.shown:get(), M.GAP)
end)

ui.effect(function()
	local shown = M.members:get()
	if not same(shown, M.grouped:peek()) then
		M.grouped:set(table.clone(shown))
	end
end)

function M.can_take(id: string): boolean
	local focused = M.focused:peek()
	if not focused then
		return false
	end
	local members = M.members:peek()
	if #members == 0 then
		members = { focused }
	end
	-- (One already in it moves; a new one needs room.)
	return table.find(members, id) ~= nil or #members < M.MAX
end

function M.drop(id: string, target: string, side: string, focus: boolean)
	if id == target then
		return
	end
	local focused = M.focused:peek()
	if not focused or not M.can_take(id) then
		return
	end
	local base = without(M.shown:peek() or leaf(focused), id)
	if not base or not table.find(leaves(base), target) then
		return
	end
	local tree = insert(base, target, id, side)
	if not tree then
		return
	end
	-- The split as it now is, in place of the one it was; the chat taken
	-- out of any other it was in.
	local list: { Node } = {}
	for _, t in trees:peek() do
		if not table.find(leaves(t), focused) then
			local rest = without(t, id)
			if rest and #leaves(rest) >= 2 then
				table.insert(list, rest)
			end
		end
	end
	table.insert(list, tree)
	trees:set(list)
	if focus then
		route.open_session(id)
	end
end

function M.close(id: string)
	local shown = M.shown:peek()
	if not shown then
		return
	end
	local order = leaves(shown)
	local at = table.find(order, id)
	if not at then
		return
	end
	local focused = M.focused:peek()
	local list: { Node } = {}
	for _, t in trees:peek() do
		local rest = if table.find(leaves(t), id) then without(t, id) else t
		if rest and #leaves(rest) >= 2 then
			table.insert(list, rest)
		end
	end
	trees:set(list)
	if focused == id then
		-- Its neighbour (the one after it, or before at the end) focused.
		local next_id = order[at + 1] or order[at - 1]
		if next_id then
			route.open_session(next_id)
		end
	end
end

function M.focus(id: string)
	route.open_session(id)
end

-- A chat's title and agent, as the sidebar lists it: a computed that wakes
-- what reads it only when they change.
function M.chat(id: string): any
	return steady(function(): { title: string, agent: string? }
		local now = sessions.now:get()
		for _, list in { now.chats, now.archived or {} } do
			for _, c in list do
				if c.id == id then
					return { title = c.title, agent = c.agent }
				end
			end
		end
		return { title = "", agent = nil }
	end)
end

-- What a drag carries that can join the split: a sidebar row's session,
-- or a pane's own head.
function M.dragged(payload: any): string?
	if type(payload) ~= "table" then
		return nil
	end
	if type(payload.split_pane) == "string" then
		return payload.split_pane
	end
	if type(payload.session) == "string" then
		return payload.session
	end
	return nil
end

return M
