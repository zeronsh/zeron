--!strict
-- The branch graph beside History's rows: which lane each commit is in,
-- and the lines through its row, worked out once for the commits in
-- order (newest first, a parent never before its child: `git log
-- --topo-order`) and drawn a row at a time.
--
-- Lanes are columns, each waiting for the commit it leads to. A commit
-- takes the lane that waits for it (the leftmost, when two do: the others
-- join it), hands that lane to its first parent, and opens a lane for each
-- other parent (or joins one that waits for it already). A lane never
-- moves sideways, so every line through a row is straight, but for those
-- that meet or leave the row's commit. Each lane keeps its colour from
-- where it starts.

export type Line = {
	-- From lane `a` to lane `b`; `from`/`to` are 0 (the row's top), 0.5
	-- (its commit's middle) or 1 (its foot).
	a: number,
	b: number,
	from: number,
	to: number,
	color: number,
}

export type Row = { col: number, color: number, lines: { Line }, lanes: number }

local M = {}

-- The rows for `commits` ({ sha, parents }), and the most lanes any row has.
-- `max_lanes` bounds line geometry only. Keep every lane, parent and color
-- in the topology: a hidden lane can join a visible one later, and compact
-- mode still needs every commit's own column/color and the full width.
function M.layout(commits: { { sha: string, parents: { string } } }, max_lanes: number?): ({ Row }, number)
	local visible = max_lanes or math.huge
	-- (A free lane is `false`, never a hole: `#lanes` stays the count.)
	local lanes: { string | false } = {}
	local colors: { number } = {}
	local next_color = 0
	local rows: { Row } = {}
	local widest = 1
	local function free_slot(avoid: number?): number
		for j = 1, #lanes + 1 do
			if not lanes[j] and j ~= avoid then
				return j
			end
		end
		return #lanes + 1
	end
	local function new_color(): number
		local c = next_color
		next_color += 1
		return c
	end
	for _, c in commits do
		local lines: { Line } = {}
		-- Its lane: the leftmost waiting for it, or a new one (a branch's tip).
		local col: number? = nil
		local n = #lanes
		for j = 1, n do
			if lanes[j] == c.sha then
				col = j
				break
			end
		end
		if col == nil then
			col = free_slot()
			colors[col :: number] = new_color()
		end
		local here = col :: number
		-- Width includes incoming lanes even when they end here and their
		-- lines are omitted. It must not depend on the retained geometry.
		local width = math.max(n, here)
		-- Into the row: each lane from the top, straight down, or into the
		-- commit if it waited for it.
		for j = 1, if here <= visible then math.min(n, visible) else 0 do
			local w = lanes[j]
			if w then
				if w == c.sha then
					table.insert(lines, { a = j, b = here, from = 0, to = 0.5, color = colors[j] })
				end
			end
		end
		-- The lanes that met here are free now.
		for j = 1, n do
			if lanes[j] == c.sha then
				lanes[j] = false
			end
		end
		local color = colors[here]
		-- Out of the row: its first parent down its own lane; each other
		-- parent down the lane that waits for it, or a new one.
		local parents = c.parents
		-- (The lanes this commit opens: they start at its middle.)
		local opened: { [number]: boolean } = {}
		if #parents == 0 then
			lanes[here] = false
		else
			lanes[here] = parents[1]
			if here <= visible then
				table.insert(lines, { a = here, b = here, from = 0.5, to = 1, color = colors[here] })
			end
			for k = 2, #parents do
				local p = parents[k]
				local into: number? = nil
				for j = 1, #lanes do
					if j ~= here and lanes[j] == p then
						into = j
						break
					end
				end
				if into == nil then
					into = free_slot(here)
					lanes[into :: number] = p
					colors[into :: number] = new_color()
					opened[into :: number] = true
				end
				width = math.max(width, into :: number)
				if here <= visible and into :: number <= visible then
					table.insert(lines, { a = here, b = into :: number, from = 0.5, to = 1, color = colors[into :: number] })
				end
			end
		end
		-- Lanes passing by, top to foot.
		for j = 1, math.min(#lanes, visible) do
			local w = lanes[j]
			if w and j ~= here and not opened[j] then
				table.insert(lines, 1, { a = j, b = j, from = 0, to = 1, color = colors[j] })
			end
		end
		-- (Free lanes at the end go.)
		while #lanes > 0 and not lanes[#lanes] do
			table.remove(lanes)
		end
		widest = math.max(widest, width)
		table.insert(rows, { col = here, color = color, lines = lines, lanes = width })
	end
	return rows, widest
end

return M
