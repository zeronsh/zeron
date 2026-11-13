-- A project's default tile, the same on every device: a copy of a repository
-- (its `repository` identity shared by every clone, the key its name and icon
-- are shared under) takes its hue from that identity, and its artwork from
-- the repository's copies in one order. A `local:` identity is one device's
-- own and a folder has none: those keep to their own id, as before.
--
--   default.shared(repository)   the identity copies share, nil if none
--   default.key(p)               what its default follows: `repo:<identity>`,
--                                else its id
--   default.hue_of(key)          the hue a key has, the same every time
--   default.hue(p)               its default hue
--   default.copies(p, all)       where its artwork is looked for, in order:
--                                the repository's copies, oldest first (id
--                                breaking ties), else itself
--
-- Pure: no state, no host.

local M = {}

-- (The default's order, kept as it was so no project's colour changes.)
local DEFAULT_HUES = { "violet", "blue", "emerald", "sky", "amber", "rose", "lime", "teal", "orange", "indigo", "pink", "cyan" }

export type Copy = { id: string, repository: string?, created: number? }

function M.shared(repository: string?): string?
	if repository == nil or repository == "" or string.sub(repository, 1, 6) == "local:" then
		return nil
	end
	return repository
end

function M.key(p: Copy): string
	local identity = M.shared(p.repository)
	return if identity then `repo:{identity}` else p.id
end

function M.hue_of(key: string): string
	local n = 0
	for i = 1, #key do
		n = (n * 31 + string.byte(key, i)) % 9973
	end
	return DEFAULT_HUES[n % #DEFAULT_HUES + 1]
end

function M.hue(p: Copy): string
	return M.hue_of(M.shared(p.repository) or p.id)
end

function M.copies<T>(p: T & Copy, all: { T & Copy }): { T & Copy }
	local identity = M.shared(p.repository)
	if not identity then
		return { p }
	end
	local out = {}
	for _, q in all do
		if q.repository == identity then
			table.insert(out, q)
		end
	end
	if #out == 0 then
		return { p }
	end
	table.sort(out, function(a, b): boolean
		local x, y = a.created or math.huge, b.created or math.huge
		if x ~= y then
			return x < y
		end
		return a.id < b.id
	end)
	return out
end

return M
