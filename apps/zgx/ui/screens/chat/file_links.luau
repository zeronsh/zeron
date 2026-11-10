-- A project's file or folder as Zeron mentions it in a prompt
-- (crates/proto file_mentions.rs `local_file_link`): `[name](zeron-file:path)`,
-- the name's brackets escaped, the path's other characters encoded, a
-- folder's ending in `/`. The engine rewrites it for the agent as a plain
-- relative link; the transcript shows its name.
local M = {}

function M.link(path: string, dir: boolean?): string
	local bare = string.gsub(path, "/+$", "")
	local name = string.match(bare, "([^/]+)$") or bare
	local label = string.gsub(name, "[\\%[%]`]", function(c: string): string
		return "\\" .. c
	end)
	local target = string.gsub(bare .. (if dir then "/" else ""), "[^%w%-%._~/]", function(c: string): string
		return string.format("%%%02X", string.byte(c))
	end)
	return `[{label}](zeron-file:{target})`
end

-- The project path a mention link stands for (a folder's with its `/`), or
-- nil for any other link (or one going outside the project).
function M.path_of(link: string): string?
	local target = string.match(link, "^zeron%-file:(.+)$")
	if not target then
		return nil
	end
	local ok = true
	local path = string.gsub(target, "%%(%x%x)", function(h: string): string
		return string.char(tonumber(h, 16) :: number)
	end)
	local bare = string.gsub(path, "/$", "")
	if bare == "" or string.find(bare, "//", 1, true) then
		ok = false
	end
	for part in string.gmatch(bare, "[^/]+") do
		if part == "." or part == ".." then
			ok = false
		end
	end
	if not ok or string.sub(path, 1, 1) == "/" or string.find(path, "\\", 1, true) then
		return nil
	end
	return path
end

-- A link's target as a file of the project at `root` (its absolute
-- folder), as Zeron opens a transcript's links (crates/ui
-- workspace_links.rs `resolve_workspace_file_link`): a `zeron-file:`
-- mention; a relative path (its file name with a `.`); an absolute one
-- (`file://` too), inside `root` or not (`outside`, its whole path); each
-- with a line from `#L12`, `#L12C5`, `#L12-L20`, `:12`, `:12:5` or
-- `:12-20`. nil for anything else: a web link, `mailto:`, a path that
-- climbs out (`..`), a line 0 or a range backwards.
-- (`end_line`: a range's last line, `path:10-12`, `#L10-L12`.)
export type Resolved = { path: string, line: number?, column: number?, end_line: number?, outside: boolean? }

local function decode(raw: string): string?
	local bad = false
	local out = string.gsub(raw, "%%(..)", function(h: string): string
		local n = if string.match(h, "^%x%x$") then tonumber(h, 16) else nil
		if not n then
			bad = true
			return ""
		end
		return string.char(n)
	end)
	if bad or not utf8.len(out) then
		return nil
	end
	return out
end

local function encode(path: string): string
	return (string.gsub(path, "[^%w%-%._~/]", function(c: string): string
		return string.format("%%%02X", string.byte(c))
	end))
end

-- No `\`, `?` or control characters; no empty, `.` or `..` part (but a
-- leading `/`).
local function clean(path: string): boolean
	if string.find(path, "[\\%?%c]") then
		return false
	end
	local index = 0
	for part in string.gmatch(path .. "/", "([^/]*)/") do
		index += 1
		if (part == "" and index ~= 1) or part == "." or part == ".." then
			return false
		end
	end
	return true
end

local function file_name(path: string): string
	return string.match(path, "([^/]*)$") or path
end

local function positive(v: string): number?
	local n = if string.match(v, "^%d+$") then tonumber(v) else nil
	return if n and n > 0 then n else nil
end

-- `12` or `12C5`.
local function line_column(v: string): (number?, number?)
	local line, column = string.match(v, "^(%d+)C(%d+)$")
	if line then
		return tonumber(line), tonumber(column)
	end
	if string.match(v, "^%d+$") then
		return tonumber(v), nil
	end
	return nil, nil
end

-- A `#…` fragment: the path before it and its line, or false (not a link).
local function fragment(target: string): (string | false, number?, number?, number?)
	local path_, frag_ = string.match(target, "^(.*)#([^#]*)$")
	if not path_ or not frag_ then
		return target, nil, nil, nil
	end
	local path, frag = path_ :: string, frag_ :: string
	if frag == "" then
		return path, nil, nil, nil
	end
	if string.find(frag, "/", 1, true) then
		return target, nil, nil, nil
	end
	local rest = string.match(frag, "^L(.*)$") :: string?
	if not rest then
		return path, nil, nil, nil
	end
	local start = string.match(rest, "^([^%-]*)%-") :: string?
	local finish = string.match(rest, "^[^%-]*%-(.*)$") :: string?
	local line, column = line_column(start or rest)
	if not line then
		return path, nil, nil, nil
	end
	if line == 0 or column == 0 then
		return false, nil, nil, nil
	end
	if finish then
		local e, ec = line_column((string.match(finish, "^L(.*)$") :: string?) or finish)
		if not e then
			return path, nil, nil, nil
		end
		if e < line or ec == 0 then
			return false, nil, nil, nil
		end
		return path, line, column, if e > line then e else nil
	end
	return path, line, column, nil
end

-- A `:12`, `:12:5` or `:12-20` suffix, as `fragment`.
local function suffix(target: string): (string | false, number?, number?, number?)
	local before_ = string.match(target, "^(.*):[^:]*$") :: string?
	if not before_ then
		return target, nil, nil, nil
	end
	local before = before_ :: string
	local last = string.sub(target, #before + 2)
	local a = string.match(last, "^([^%-]*)%-") :: string?
	local b = string.match(last, "^[^%-]*%-(.*)$") :: string?
	if a and b then
		if not string.match(a, "^%d+$") or not string.match(b, "^%d+$") then
			return target, nil, nil, nil
		end
		local start, finish = tonumber(a) :: number, tonumber(b) :: number
		if start == 0 or finish < start then
			return false, nil, nil, nil
		end
		return before, start, nil, if finish > start then finish else nil
	end
	if string.match(last, "^%d+$") and tonumber(last) == 0 then
		return false, nil, nil, nil
	end
	local n = positive(last)
	if not n then
		return target, nil, nil, nil
	end
	local before2 = string.match(before, "^(.*):[^:]*$") :: string?
	if before2 then
		local mid = string.sub(before, #before2 + 2)
		local line = positive(mid)
		if line then
			return before2, line, n, nil
		end
		if string.match(mid, "^%d+$") and tonumber(mid) == 0 then
			return false, nil, nil, nil
		end
	end
	return before, n, nil, nil
end

-- `path` inside `root`: its relative rest, or nil.
local function inside(path: string, root: string?): string?
	if not root or root == "" then
		return nil
	end
	local r = string.gsub(root, "/+$", "")
	if string.sub(path, 1, #r + 1) ~= r .. "/" then
		return nil
	end
	local rest = string.sub(path, #r + 2)
	return if rest ~= "" then rest else nil
end

-- Whether a link is a file of the chat's (in its folder `root`): a reply
-- draws it as the chip a prompt's mention is (ds/markdown `link_chip`).
function M.chip(target: string, root: string?): boolean
	local r = M.resolve(target, root)
	return r ~= nil and not r.outside
end

function M.resolve(target: string, root: string?): Resolved?
	target = string.match(target, "^%s*(.-)%s*$") or target
	if target == "" then
		return nil
	end
	local mention = string.match(target, "^zeron%-file:(.*)$")
	if mention then
		local path = decode(mention)
		if not path or encode(path) ~= mention or string.sub(path, -1) == "/" or string.find(path, ":", 1, true) or not clean(path) or string.sub(path, 1, 1) == "/" then
			return nil
		end
		return { path = path }
	end
	local file_url = string.sub(target, 1, 7) == "file://"
	local raw = target
	if file_url then
		local rest = string.sub(target, 8)
		if string.sub(rest, 1, 1) ~= "/" or string.find(rest, "?", 1, true) then
			return nil
		end
		raw = rest
	elseif string.find(target, "://", 1, true) or string.sub(target, 1, 7) == "mailto:" then
		return nil
	end
	local p1, line1, column1, end1 = fragment(raw)
	if not p1 then
		return nil
	end
	local p2, line2, column2, end2 = suffix(p1)
	if not p2 then
		return nil
	end
	local path = if string.find(p2, "%", 1, true) then (decode(p2) or p2) else p2
	if path == "" or not clean(path) then
		return nil
	end
	local line, column = line1 or line2, column1 or column2
	local end_line = if line1 then end1 else end2
	if string.sub(path, 1, 1) == "/" then
		if #path == 1 or string.sub(path, 1, 2) == "//" or string.sub(path, -1) == "/" or (not file_url and not string.find(file_name(path), ".", 1, true)) then
			return nil
		end
		local rel = inside(path, root)
		if rel then
			return { path = rel, line = line, column = column, end_line = end_line }
		end
		return { path = path, line = line, column = column, end_line = end_line, outside = true }
	end
	if file_url or string.sub(path, 1, 1) == "~" or string.match(path, "^%a[%w%+%-%.]*:") then
		return nil
	end
	if not string.find(file_name(path), ".", 1, true) then
		return nil
	end
	return { path = path, line = line, column = column, end_line = end_line }
end

return M
