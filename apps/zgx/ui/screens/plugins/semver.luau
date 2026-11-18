--!strict
-- Versions as the store has them: strict SemVer 2.0.0, and the ranges a
-- plugin's `engines.zeron` names the Zeron versions it works with, in
-- npm's range language (`>=0.4.0`, `^0.4.0`, `~0.4.1`, `0.4.x`, `1.2.3 -
-- 1.4.0`, `>=0.4.0 <0.6.0`, `^0.4.0 || ^1.0.0`; nothing or `*`, any). A
-- Zeron that's a pre-release (a beta) is placed by its order, as npm's
-- `includePrerelease` does: a beta of 0.5.0 comes after 0.4.9 and before
-- 0.5.0, so `>=0.4.0` takes it.
--
--   semver.parse("1.2.3-beta.1")       { major, minor, patch, pre = { "beta", 1 } } or nil
--   semver.compare(a, b)               -1, 0, 1 (parsed or text)
--   semver.satisfies("0.4.2", ">=0.4") true; nil when either can't be read
--   semver.least(">=0.4.0 <0.6.0")     "0.4.0": the earliest it takes, to say

local M = {}

export type Version = { major: number, minor: number, patch: number, pre: { string | number } }

local function num(s: string): number?
	if s == "0" or string.match(s, "^[1-9]%d*$") then
		return tonumber(s)
	end
	return nil
end

function M.parse(text: string): Version?
	if type(text) ~= "string" then
		return nil
	end
	local core, rest = string.match(text, "^v?(%d+%.%d+%.%d+)(.*)$")
	if not core then
		return nil
	end
	local a, b, c = string.match(core :: string, "^(%d+)%.(%d+)%.(%d+)$")
	local major, minor, patch = num(a :: string), num(b :: string), num(c :: string)
	if not (major and minor and patch) then
		return nil
	end
	local pre: { string | number } = {}
	local tail = rest :: string
	-- (Build metadata never orders: dropped.)
	local build_at = string.find(tail, "+", 1, true)
	if build_at then
		local build = string.sub(tail, build_at + 1)
		if not string.match(build, "^[0-9A-Za-z%-]+[%.0-9A-Za-z%-]*$") or string.find(build, "..", 1, true) or string.sub(build, -1) == "." then
			return nil
		end
		tail = string.sub(tail, 1, build_at - 1)
	end
	if tail ~= "" then
		if string.sub(tail, 1, 1) ~= "-" or #tail == 1 then
			return nil
		end
		for part in string.gmatch(string.sub(tail, 2) .. ".", "([^%.]*)%.") do
			if part == "" or not string.match(part, "^[0-9A-Za-z%-]+$") then
				return nil
			end
			if string.match(part, "^%d+$") then
				local n = num(part)
				if n == nil then
					return nil -- a leading zero
				end
				table.insert(pre, n)
			else
				table.insert(pre, part)
			end
		end
	end
	return { major = major, minor = minor, patch = patch, pre = pre }
end

local function cmp_pre(a: { string | number }, b: { string | number }): number
	if #a == 0 and #b == 0 then
		return 0
	elseif #a == 0 then
		return 1
	elseif #b == 0 then
		return -1
	end
	for i = 1, math.max(#a, #b) do
		local x, y = a[i], b[i]
		if x == nil then
			return -1
		elseif y == nil then
			return 1
		elseif type(x) == "number" and type(y) == "number" then
			if x ~= y then
				return if x < y then -1 else 1
			end
		elseif type(x) == "number" then
			return -1
		elseif type(y) == "number" then
			return 1
		elseif x ~= y then
			return if (x :: string) < (y :: string) then -1 else 1
		end
	end
	return 0
end

local function cmp(a: Version, b: Version): number
	for _, k in { "major", "minor", "patch" } do
		local x, y = (a :: any)[k], (b :: any)[k]
		if x ~= y then
			return if x < y then -1 else 1
		end
	end
	return cmp_pre(a.pre, b.pre)
end

function M.compare(a: Version | string, b: Version | string): number?
	local x = if type(a) == "string" then M.parse(a) else a
	local y = if type(b) == "string" then M.parse(b) else b
	if not x or not y then
		return nil
	end
	return cmp(x, y)
end

-- A comparator: `op` against `v`.
type Comparator = { op: string, v: Version }

local function v(major: number, minor: number, patch: number, pre: { string | number }?): Version
	return { major = major, minor = minor, patch = patch, pre = pre or {} }
end

-- A partial version (`1`, `1.2`, `1.x`, `*`): its numbers, nil where it's a
-- wildcard, and its pre-release when whole.
local function partial(text: string): (boolean, number?, number?, number?, { string | number }?)
	if text == "" or text == "*" or text == "x" or text == "X" then
		return true, nil, nil, nil, nil
	end
	local whole = M.parse(text)
	if whole then
		return true, whole.major, whole.minor, whole.patch, whole.pre
	end
	local parts = string.split(string.gsub(text, "^v", "") :: string, ".")
	if #parts > 3 then
		return false
	end
	local out: { number? } = {}
	local wild = false
	for i, p in parts do
		if p == "x" or p == "X" or p == "*" then
			wild = true
		elseif wild then
			return false -- `1.x.3`
		else
			local n = num(p)
			if n == nil then
				return false
			end
			out[i] = n
		end
	end
	return true, out[1], out[2], out[3], nil
end

-- One comparator set's terms (space separated), each expanded to plain
-- comparators; false if any can't be read.
local function expand(term: string, into: { Comparator }): boolean
	local op, rest = string.match(term, "^([<>]=?)(.*)$")
	if not op then
		op, rest = string.match(term, "^(=)(.*)$")
	end
	if not op then
		op, rest = string.match(term, "^([%^~])(.*)$")
	end
	op = op or ""
	rest = rest or term
	local ok, major, minor, patch, pre = partial(rest :: string)
	if not ok then
		return false
	end
	if major == nil then
		-- Anything (`*`, `>=*`); `<*` and `>*` take nothing.
		if op == "<" or op == ">" then
			table.insert(into, { op = "<", v = v(0, 0, 0, { 0 }) })
		end
		return true
	end
	local M_, m_, p_ = major :: number, minor, patch
	if op == "^" then
		local lo = v(M_, m_ or 0, p_ or 0, pre)
		local hi: Version
		if M_ > 0 or m_ == nil then
			hi = v(M_ + 1, 0, 0, { 0 })
		elseif (m_ :: number) > 0 or p_ == nil then
			hi = v(0, (m_ :: number) + 1, 0, { 0 })
		else
			hi = v(0, 0, (p_ :: number) + 1, { 0 })
		end
		table.insert(into, { op = ">=", v = lo })
		table.insert(into, { op = "<", v = hi })
	elseif op == "~" then
		local lo = v(M_, m_ or 0, p_ or 0, pre)
		local hi = if m_ == nil then v(M_ + 1, 0, 0, { 0 }) else v(M_, (m_ :: number) + 1, 0, { 0 })
		table.insert(into, { op = ">=", v = lo })
		table.insert(into, { op = "<", v = hi })
	elseif p_ ~= nil then
		table.insert(into, { op = if op == "" then "=" else op, v = v(M_, m_ :: number, p_ :: number, pre) })
	else
		-- A partial: the run of versions it names.
		local lo = v(M_, m_ or 0, 0)
		local hi = if m_ == nil then v(M_ + 1, 0, 0, { 0 }) else v(M_, (m_ :: number) + 1, 0, { 0 })
		if op == "" or op == "=" then
			table.insert(into, { op = ">=", v = lo })
			table.insert(into, { op = "<", v = hi })
		elseif op == ">" then
			table.insert(into, { op = ">=", v = hi })
		elseif op == ">=" then
			table.insert(into, { op = ">=", v = lo })
		elseif op == "<" then
			table.insert(into, { op = "<", v = lo })
		elseif op == "<=" then
			table.insert(into, { op = "<", v = hi })
		end
	end
	return true
end

-- A range read into its alternatives (`||`), each a list of comparators
-- all of which must hold; nil if it can't be read.
local function sets(range: string): { { Comparator } }?
	local out: { { Comparator } } = {}
	for _, alt in string.split(range, "||") do
		local text = string.gsub(alt, "^%s*(.-)%s*$", "%1") :: string
		-- (`>= 1.2` is `>=1.2`.)
		text = string.gsub(text, "([<>=~%^]+)%s+", "%1") :: string
		local set: { Comparator } = {}
		local lo, hi = string.match(text, "^(%S+)%s+%-%s+(%S+)$")
		if lo and hi then
			local ok1 = expand(">=" .. lo, set)
			-- (`- 1.4` takes all of 1.4; `- 1.4.0` up to it.)
			local ok2, major, minor, patch = partial(hi :: string)
			if not ok1 or not ok2 then
				return nil
			end
			if major ~= nil then
				if patch ~= nil then
					local whole = M.parse(hi :: string) :: Version
					table.insert(set, { op = "<=", v = whole })
				elseif minor ~= nil then
					table.insert(set, { op = "<", v = v(major :: number, (minor :: number) + 1, 0, { 0 }) })
				else
					table.insert(set, { op = "<", v = v((major :: number) + 1, 0, 0, { 0 }) })
				end
			end
		else
			for term in string.gmatch(text, "%S+") do
				if not expand(term, set) then
					return nil
				end
			end
		end
		table.insert(out, set)
	end
	return out
end

local function holds(c: Comparator, x: Version): boolean
	local d = cmp(x, c.v)
	if c.op == "=" then
		return d == 0
	elseif c.op == ">" then
		return d > 0
	elseif c.op == ">=" then
		return d >= 0
	elseif c.op == "<" then
		return d < 0
	elseif c.op == "<=" then
		return d <= 0
	end
	return false
end

function M.satisfies(version: string, range: string?): boolean?
	local x = M.parse(version)
	if not x then
		return nil
	end
	if range == nil or string.match(range, "^%s*$") then
		return true
	end
	local alts = sets(range :: string)
	if not alts then
		return nil
	end
	for _, set in alts do
		local all = true
		for _, c in set do
			if not holds(c, x) then
				all = false
				break
			end
		end
		if all then
			return true
		end
	end
	return false
end

-- The earliest version a range takes, said plainly ("0.4.0"), or nil
-- when it has no floor (or can't be read).
function M.least(range: string?): string?
	if range == nil then
		return nil
	end
	local alts = sets(range)
	if not alts then
		return nil
	end
	local best: Version? = nil
	for _, set in alts do
		local floor: Version? = nil
		for _, c in set do
			if c.op == ">=" or c.op == ">" or c.op == "=" then
				if floor == nil or cmp(c.v, floor) > 0 then
					floor = c.v
				end
			end
		end
		if floor == nil then
			return nil
		end
		if best == nil or cmp(floor, best) < 0 then
			best = floor
		end
	end
	if best == nil then
		return nil
	end
	local b = best :: Version
	return `{b.major}.{b.minor}.{b.patch}`
end

return M
