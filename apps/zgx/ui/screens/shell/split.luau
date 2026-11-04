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
--   split.across:get()      how many panes wide the page must be for the
--                           narrowest to have its least (1 for none)
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

local motion = require("@ds/motion")

local M = {}

M.MAX = 6
-- The window's gap between the chats (`zeron.inset.gap`, ds/tokens).
M.GAP = (ui.theme.get("zeron.inset.gap") :: number?) or 4

-- (`shares`: how a row or a column divides among its parts, a share each, summing
-- to 1; none: equally. A divider dragged sets them.)
export type Node = { kind: string, id: string?, children: { Node }?, shares: { number }?, key: string? }
-- An edge: `p` of the page's length, plus `q` points.
export type Edge = { p: number, q: number }
export type Box = { left: Edge, right: Edge, top: Edge, bottom: Edge }

-- Every tab of chats, in the top bar's order, kept across launches: each a tree
-- of its panes (a chat alone, a leaf), and a chat is in one tab at a time. `key`,
-- a tab's own: without one, its first chat's. (Its key is the one the Inset template
-- kept splits under before this was Zeron's, so the splits of anyone who used it are
-- still theirs, with it or without.)
local trees = ui.persist("inset_split_trees", {} :: { Node })
-- (A tab closed used to be kept, to come back with its chats: those are gone.)
do
	local open_tabs: { Node } = {}
	for _, t in trees:peek() do
		if not (t :: any).closed then
			table.insert(open_tabs, t)
		end
	end
	if #open_tabs ~= #trees:peek() then
		trees:set(open_tabs)
	end
end
-- (The tabs' order before a tab was a thing of its own: `trees` held only splits.)
local old_order = ui.persist("shell_session_tabs", {} :: { string })

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
	local shares = node.shares
	return { kind = node.kind, children = kids, shares = if shares and #shares == #kids then shares else nil }
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
	return normalize({ kind = node.kind, children = kids, shares = node.shares })
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
		return { kind = n.kind, children = kids, shares = n.shares }
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

-- A divider: the gap between two parts of a row or a column, which dragged moves
-- their shares. `length`: what its row or column shares out. `path`: its row or
-- column, by the child at each level; `at`: the part before it.
export type Divider = Box & { id: string, axis: string, length: Edge, path: { number }, at: number }

-- Each chat's box in the page, `gap` points apart, and the dividers: a node's
-- parts share its span less the gaps between them. Independent of the page's size
-- (see `Edge`).
local function arrange(tree: Node?, gap: number): ({ [string]: Box }, { Divider })
	local out: { [string]: Box } = {}
	local dividers: { Divider } = {}
	local function place(n: Node, box: Box, path: { number })
		if n.kind == "leaf" then
			out[n.id :: string] = box
			return
		end
		local kids = n.children or {}
		local count = #kids
		local row = n.kind == "row"
		local first = if row then box.left else box.top
		local last = if row then box.right else box.bottom
		-- What the parts share: the span less the gaps.
		local p = last.p - first.p
		local q = last.q - first.q - (count - 1) * gap
		local function along(from: Edge, to: Edge): Box
			return if row
				then { left = from, right = to, top = box.top, bottom = box.bottom }
				else { left = box.left, right = box.right, top = from, bottom = to }
		end
		local shares = n.shares
		local so_far = 0
		for i, c in kids do
			local share = if shares and #shares == count then shares[i] else 1 / count
			local from = edge(first.p + p * so_far, first.q + q * so_far + gap * (i - 1))
			so_far += share
			local to = edge(first.p + p * so_far, first.q + q * so_far + gap * (i - 1))
			local deeper = table.clone(path)
			table.insert(deeper, i)
			place(c, along(from, to), deeper)
			if i < count then
				local d: any = along(to, edge(to.p, to.q + gap))
				-- (Its way round in its name: a split turned from a row to a column has
				-- another divider, not this one going the wrong way.)
				d.id = `{n.kind}:{table.concat(path, ".")}/{i}`
				d.axis = if row then "row" else "col"
				d.length = edge(p, q)
				d.path = path
				d.at = i
				table.insert(dividers, d)
			end
		end
	end
	if tree then
		place(tree, M.WHOLE, {})
	end
	return out, dividers
end
local function frames_of(tree: Node?, gap: number): { [string]: Box }
	local out = arrange(tree, gap)
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

-- How many panes wide the page must be for the narrowest of them to have
-- one pane's width: a row's parts share its width equally, each needing the
-- widest any of them needs; a column's are each as wide as it.
local function across_of(node: Node?): number
	if not node or node.kind == "leaf" then
		return 1
	end
	local most = 1
	for _, c in node.children or {} do
		most = math.max(most, across_of(c))
	end
	return if node.kind == "row" then most * #(node.children or {}) else most
end

-- The tree's own workings, for any split of things (the panel's tabs too:
-- screens/panel), and its boxes.
M.tree = { leaf = leaf, leaves = leaves, normalize = normalize, without = without, insert = insert, frames = frames_of, steady = steady }

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

local function key_of(t: Node): string
	return t.key or leaves(t)[1] or ""
end

local function holds(t: Node, id: string): boolean
	return table.find(leaves(t), id) ~= nil
end

local function tab_with_key(list: { Node }, key: string?): Node?
	for _, t in list do
		if key_of(t) == key then
			return t
		end
	end
	return nil
end

-- A key no other tab has: its first chat's, numbered past any that has it.
local function fresh_key(first: string, list: { Node }): string
	local key, n = first, 1
	while tab_with_key(list, key) do
		n += 1
		key = `{first}~{n}`
	end
	return key
end

-- A tree as a tab, keeping that tab's key.
local function keyed(tree: Node, key: string): Node
	local out = table.clone(tree)
	out.key = key
	return out
end

M.focused = ui.computed(function(): string?
	local c = route.chat:get()
	return if c.name == "session" then c.session else nil
end)

-- The tab showing: the one with the chat focused. (Nil until a chat with no tab has
-- one: below.)
M.current = steady(function(): Node?
	local id = M.focused:get()
	if not id then
		return nil
	end
	for _, t in trees:get() do
		if holds(t, id) then
			return t
		end
	end
	return nil
end)

-- The tabs with `id` taken out of all but the tab `except`: one left with no
-- chats goes.
local function taken_out(list: { Node }, id: string, except: string?): { Node }
	local out: { Node } = {}
	for _, t in list do
		local key = key_of(t)
		if key == except or not holds(t, id) then
			table.insert(out, t)
		else
			local rest = without(t, id)
			if rest then
				table.insert(out, keyed(rest, key))
			end
		end
	end
	return out
end

-- (Tabs kept from when only splits were kept: their order, once.)
ui.effect(function()
	local old = old_order:get()
	if #old == 0 then
		return
	end
	local kept = trees:peek()
	local list: { Node } = {}
	local placed: { [string]: boolean } = {}
	for _, id in old do
		local inside: Node? = nil
		for _, t in kept do
			if holds(t, id) then
				inside = t
				break
			end
		end
		if not inside then
			table.insert(list, leaf(id))
		elseif not placed[key_of(inside)] then
			placed[key_of(inside)] = true
			table.insert(list, inside)
		end
	end
	for _, t in kept do
		if not placed[key_of(t)] then
			table.insert(list, t)
		end
	end
	trees:set(list)
	old_order:set({})
end)
-- A chat in more than one tab stays in the first that's open. (A chat may have
-- been kept in several.)
ui.effect(function()
	local list = trees:get()
	local owner: { [string]: string } = {}
	for _, t in list do
		for _, id in leaves(t) do
			owner[id] = owner[id] or key_of(t)
		end
	end
	local out = list
	local changed = false
	for id, key in owner do
		local count = 0
		for _, t in out do
			if holds(t, id) then
				count += 1
			end
		end
		if count > 1 then
			out = taken_out(out, id, key)
			changed = true
		end
	end
	if changed then
		trees:set(out)
	end
end)
-- A chat focused with no tab takes the one tab for a chat alone, in its place, or a
-- new one at the end: navigating replaces it, until chats are composed in it. (As the
-- focus moves, not as the tabs change: a tab just closed, the focus on its way to
-- another, doesn't come back.)
ui.effect(function()
	local id = M.focused:get()
	if not id then
		return
	end
	local list = trees:peek()
	local t = M.current:peek()
	if not t then
		local tab = leaf(id)
		tab.key = fresh_key(id, list)
		local more: { Node } = {}
		local placed = false
		for _, other in list do
			if #leaves(other) >= 2 then
				table.insert(more, other)
			elseif not placed then
				table.insert(more, tab)
				placed = true
			end
		end
		if not placed then
			table.insert(more, tab)
		end
		trees:set(more)
	end
end)
-- One tab for a chat alone, however it came to be more (a split left with one, tabs
-- kept from before): the focused chat's, else the last.
ui.effect(function()
	local list = trees:get()
	local focused = M.focused:peek()
	local keep: Node? = nil
	local alone = 0
	for _, t in list do
		if #leaves(t) == 1 then
			alone += 1
			if not (keep and focused and holds(keep, focused)) then
				keep = t
			end
		end
	end
	if alone <= 1 then
		return
	end
	local out: { Node } = {}
	for _, t in list do
		if #leaves(t) >= 2 or t == keep then
			table.insert(out, t)
		end
	end
	trees:set(out)
end)

-- The tabs open, in their order, by key.
M.tabs = steady(function(): { string }
	local out: { string } = {}
	for _, t in trees:get() do
		table.insert(out, key_of(t))
	end
	return out
end)

-- A tab's chats (none: no such tab).
function M.panes(key: string): { string }
	local t = tab_with_key(trees:get(), key)
	return if t then leaves(t) else {}
end

-- The key of the tab showing.
function M.current_key(): string?
	local t = M.current:get()
	return if t then key_of(t) else nil
end

-- Those tabs closed, and forgotten: a chat of theirs opened again gets a tab of its
-- own.
function M.close_tabs(keys: { string })
	local list: { Node } = {}
	for _, t in trees:peek() do
		if not table.find(keys, key_of(t)) then
			table.insert(list, t)
		end
	end
	trees:set(list)
end

-- A tab moved to where another stands.
function M.move_tab(key: string, onto: string)
	local list = table.clone(trees:peek())
	local from, to
	for i, t in list do
		local k = key_of(t)
		if k == key then
			from = i
		elseif k == onto then
			to = i
		end
	end
	if not from or not to then
		return
	end
	local moved = table.remove(list, from)
	table.insert(list, to, moved :: Node)
	trees:set(list)
end

M.shown = steady(function(): Node?
	local id = M.focused:get()
	local current = M.current:get()
	if not id or not current then
		return nil
	end
	-- (Its chats deleted since are left out.)
	local t: Node? = current
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
-- The dividers of the split, as it is.
M.dividers = ui.computed(function(): { Divider }
	local _, list = arrange(M.shown:get(), M.GAP)
	return list
end)

-- The tree with its node at `path` changed by `change`.
local function at_path(node: Node, path: { number }, depth: number, change: (n: Node) -> Node): Node
	if depth > #path then
		return change(node)
	end
	local kids = table.clone(node.children or {})
	kids[path[depth]] = at_path(kids[path[depth]], path, depth + 1, change)
	return { kind = node.kind, children = kids, shares = node.shares }
end

-- A divider dragged: the part before it to `share` of what the row or column
-- shares out, the part after it taking up the difference, each at least `least`.
function M.resize(path: { number }, at: number, start: { number }, moved: number, least: number)
	local current = M.current:peek()
	if not current then
		return
	end
	local key = key_of(current)
	local tree = at_path(current, path, 1, function(n: Node): Node
		local shares = table.clone(start)
		local both = shares[at] + shares[at + 1]
		local first = math.clamp(shares[at] + moved, math.min(least, both / 2), math.max(both - least, both / 2))
		shares[at], shares[at + 1] = first, both - first
		return { kind = n.kind, children = n.children, shares = shares }
	end)
	local list: { Node } = {}
	for _, t in trees:peek() do
		table.insert(list, if key_of(t) == key then keyed(tree, key) else t)
	end
	trees:set(list)
end
-- A divider's row or column's shares now.
function M.shares_at(path: { number }): { number }
	local current = M.current:peek()
	local n = current
	for _, i in path do
		n = if n and n.children then n.children[i] else nil
	end
	local count = if n and n.children then #n.children else 0
	if n and n.shares and #n.shares == count then
		return table.clone(n.shares)
	end
	local out: { number } = {}
	for _ = 1, count do
		table.insert(out, 1 / count)
	end
	return out
end

-- A change of structure (a pane come, gone or moved) eases each pane to its new
-- place; a divider dragged moves them at once, and the same
-- tab's panes only: another tab lands at once.
local function shape_of(t: Node?): string
	if not t then
		return ""
	end
	if t.kind == "leaf" then
		return t.id or ""
	end
	local parts: { string } = {}
	for _, c in t.children or {} do
		table.insert(parts, shape_of(c))
	end
	return `{t.kind}({table.concat(parts, ",")})`
end
M.shape = ui.computed(function(): string
	local current = M.current:get()
	return `{if current then key_of(current) else ""}#{shape_of(M.shown:get())}`
end)
local area_element: any = nil
function M.set_area(element: any)
	area_element = element
end
-- The page's room now, in points (read when needed, not followed).
function M.area(): { w: number, h: number }
	local el = area_element
	return { w = if el then el.width or 0 else 0, h = if el then el.height or 0 else 0 }
end
-- Where each chat's box shows: `frames`, but as the shape of the tab showing changes,
-- easing there from where each stood. In shares of the page, as `frames` is, so they
-- keep to each other and to the page as it moves too (the sidebar stepping aside for
-- a pane just added). A pane just added opens out of the gap it comes in at, as its
-- neighbours make room, at its full size all along (`opening_size`).
local settle = ui.state("split.settle", 1)
local settle_from: { [string]: Box } = {}
local settle_new: { [string]: boolean } = {}
local settling: any = nil
local function mix(a: Box, b: Box, t: number): Box
	local function at(x: Edge, y: Edge): Edge
		return edge(x.p + (y.p - x.p) * t, x.q + (y.q - x.q) * t)
	end
	return { left = at(a.left, b.left), right = at(a.right, b.right), top = at(a.top, b.top), bottom = at(a.bottom, b.bottom) }
end
-- `node` with the panes in `old` keeping their shares and the others taking none.
local function with_new_shut(node: Node, old: { [string]: Box }): (Node, boolean)
	if node.kind == "leaf" then
		return node, old[node.id :: string] == nil
	end
	local kids: { Node } = {}
	local shut: { boolean } = {}
	local all_new = true
	local kept = 0
	local count = #(node.children or {})
	for i, c in node.children or {} do
		local kid, new = with_new_shut(c, old)
		kids[i], shut[i] = kid, new
		all_new = all_new and new
		if not new then
			kept += if node.shares and #node.shares == count then node.shares[i] else 1 / count
		end
	end
	local shares: { number } = {}
	for i = 1, count do
		local share = if node.shares and #node.shares == count then node.shares[i] else 1 / count
		shares[i] = if shut[i] or kept <= 0 then 0 else share / kept
	end
	return { kind = node.kind, children = kids, shares = shares }, all_new
end
M.shown_frames = ui.computed(function(): { [string]: Box }
	local to = M.frames:get()
	local t = settle:get()
	if t >= 1 then
		return to
	end
	local out: { [string]: Box } = {}
	for id, b in to do
		local from = settle_from[id]
		out[id] = if from then mix(from, b, t) else b
	end
	return out
end)
local seen_frames = M.frames:peek()
local seen_shape = M.shape:peek()
-- The chat the tab showing holds, when it holds one: it has no frame, the page is its.
local function lone_of(tree: Node?): string?
	local ids = leaves(tree)
	return if #ids == 1 then ids[1] else nil
end
local seen_lone = lone_of(M.current:peek())
ui.effect(function()
	local to, shape = M.frames:get(), M.shape:get()
	local was, was_shape, was_lone = seen_frames, seen_shape, seen_lone
	seen_frames, seen_shape, seen_lone = to, shape, lone_of(M.current:peek())
	if was_shape == shape then
		return
	end
	-- (A chat that stood alone stood on the whole page.)
	if next(was) == nil and was_lone then
		was = frames_of(leaf(was_lone), M.GAP)
	end
	if settling then
		settling:cancel()
		settling = nil
	end
	-- (None of them there before, so nothing to ease from: as for another tab.)
	local any_stays = false
	for id in to do
		any_stays = any_stays or was[id] ~= nil
	end
	-- (Another tab, or no motion: there at once.)
	if string.match(was_shape, "^[^#]*") ~= string.match(shape, "^[^#]*") or motion.reduced_now() or (next(to) ~= nil and not any_stays) then
		settle:set(1)
		return
	end
	-- From where each shows now, a step under way included.
	local t = settle:peek()
	local from: { [string]: Box } = {}
	for id, b in was do
		local start = settle_from[id]
		from[id] = if t < 1 and start then mix(start, b, t) else b
	end
	-- A pane just added: from the gap it comes in at.
	local tree = M.shown:peek()
	local shut = if tree then frames_of((with_new_shut(tree, was)), M.GAP) else {}
	settle_new = {}
	for id in to do
		if not from[id] then
			from[id] = shut[id]
			settle_new[id] = true
		end
	end
	settle_from = from
	settle:set(0)
	settling = anime.animate(settle, { value = 1, duration = 280, ease = "outQuint" })
end)

-- The size a pane just added is opening to, while it opens (what it holds stays at
-- that size, its edge revealing it); else nil.
function M.opening_size(id: string): { w: number, h: number }?
	if settle:get() >= 1 or not settle_new[id] then
		return nil
	end
	local b = M.frames:peek()[id]
	local area = M.area()
	if not b then
		return nil
	end
	return {
		w = (b.right.p - b.left.p) * area.w + b.right.q - b.left.q,
		h = (b.bottom.p - b.top.p) * area.h + b.bottom.q - b.top.q,
	}
end

M.across = ui.computed(function(): number
	return across_of(M.shown:get())
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
	-- The tab as it now is, in place of the one it was; the chat taken out of the
	-- tab it was in, which goes if that was its last.
	local current = M.current:peek()
	local key = if current then key_of(current) else focused
	local list: { Node } = {}
	local replaced = false
	for _, t in taken_out(trees:peek(), id, key) do
		if key_of(t) == key then
			table.insert(list, keyed(tree, key))
			replaced = true
		else
			table.insert(list, t)
		end
	end
	if not replaced then
		table.insert(list, keyed(tree, key))
	end
	trees:set(list)
	if focus then
		route.open_session(id)
	end
end

-- Whether a chat dragged over a tab can join it: not one already in it, and with
-- room there.
function M.can_join(id: string, key: string): boolean
	local t = tab_with_key(trees:peek(), key)
	return t ~= nil and not holds(t, id) and #leaves(t) < M.MAX
end

-- A chat dragged onto a tab: at the end of it, and out of the tab it was in, which
-- goes if that was its last. That tab shows, the chat focused.
function M.join_tab(id: string, key: string)
	if not M.can_join(id, key) then
		return
	end
	local list = taken_out(trees:peek(), id, key)
	local out: { Node } = {}
	for _, t in list do
		if key_of(t) == key then
			local panes = leaves(t)
			local tree = insert(t, panes[#panes], id, "right")
			table.insert(out, keyed(tree or t, key))
		else
			table.insert(out, t)
		end
	end
	trees:set(out)
	route.open_session(id)
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
	-- This tab less the pane. (Its chat goes on, in the sidebar.)
	local current = M.current:peek()
	local key = if current then key_of(current) else ""
	local rest = without(shown, id)
	local list: { Node } = {}
	for _, t in trees:peek() do
		if key_of(t) ~= key then
			table.insert(list, t)
		elseif rest then
			table.insert(list, keyed(rest, key))
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
