-- The languages with a shape of their own: shell, CSS, HTML (with its
-- scripts and styles coloured as JavaScript and CSS), YAML, markdown (with
-- its fenced code coloured as its language) and diffs. Each is a line
-- highlighter over the helpers in lex.luau, its states as exact as the
-- C-like ones.

local lex = require("@self/lex")

type Span = lex.Span
type Highlighter = lex.Highlighter

local C = lex.C
local push, set = lex.push, lex.set
local byte, find, sub, lower = string.byte, string.find, string.sub, string.lower

local M = {}

-- Shell ---------------------------------------------------------------------
-- A command's first word is a function; flags are attributes; `$NAME`,
-- `${...}` and `$(...)` are variables. Strings run on over lines, and a
-- heredoc's body is a string until its delimiter (state "H<delim>", or
-- "H-<delim>" for `<<-`, which allows tabs before it).

local SH_KEYWORDS = set(
	"if then else elif fi for while until do done case esac in function select return exit local export readonly declare typeset unset break continue shift trap source alias time coproc"
)
-- Where a command starts: after these, the next word is a command.
local SH_COMMAND_AFTER = set("then do else elif if while until time ! sudo exec xargs env")
-- After these, NAME=value is an assignment (export FOO=1).
local SH_DECLARE = set("export local readonly declare typeset env")

local function sh_dollar(line: string, i: number, out: { Span }): number
	local _, e = find(line, "^%$[%a_][%w_]*", i)
	if not e then
		_, e = find(line, "^%$[%d#%?%$!@%*%-]", i)
	end
	if not e and byte(line, i + 1) == 123 then
		e = find(line, "}", i, true)
	end
	if not e and byte(line, i + 1) == 40 then
		-- $( ... ) and $(( ... )): to the matching paren on the line.
		local level, j = 0, i + 1
		while j <= #line do
			local c = byte(line, j)
			if c == 40 then
				level += 1
			elseif c == 41 then
				level -= 1
				if level == 0 then
					break
				end
			end
			j += 1
		end
		e = math.min(j, #line)
	end
	e = e or i
	push(out, i, e, C.variable)
	return e + 1
end

-- A double-quoted string's body from `j`: where it ends, or nil.
local function sh_double(line: string, j: number, out: { Span }): number?
	local from = j
	local n = #line
	while j <= n do
		local b = byte(line, j)
		if b == 92 then
			push(out, from, j - 1, C.string)
			push(out, j, math.min(j + 1, n), C.escape)
			j += 2
			from = j
		elseif b == 36 then
			push(out, from, j - 1, C.string)
			j = sh_dollar(line, j, out)
			from = j
		elseif b == 34 then
			push(out, from, j, C.string)
			return j + 1
		else
			j += 1
		end
	end
	push(out, from, n, C.string)
	return nil
end

function M.shell(line: string, state: any): ({ Span }, any)
	local out: { Span } = {}
	local n = #line
	local i = 1
	if type(state) == "string" then
		local tag = sub(state, 1, 1)
		if tag == "H" then
			local dash = sub(state, 2, 2) == "-"
			local delim = sub(state, if dash then 3 else 2)
			local body = if dash then string.match(line, "^\t*(.-)%s*$") else string.match(line, "^(.-)%s*$")
			if body == delim then
				push(out, 1, n, C.label)
				return out, 0
			end
			push(out, 1, n, C.string)
			return out, state
		elseif tag == "d" then
			local p = sh_double(line, 1, out)
			if not p then
				return out, state
			end
			i = p
		elseif tag == "q" then
			local q = find(line, "'", 1, true)
			if not q then
				push(out, 1, n, C.string)
				return out, state
			end
			push(out, 1, q, C.string)
			i = q + 1
		end
	end
	local command = true
	local declaring = false
	local heredoc: string? = nil
	while i <= n do
		local b = byte(line, i)
		if b == 32 or b == 9 then
			i += 1
			continue
		end
		if b == 35 and (i == 1 or find(sub(line, i - 1, i - 1), "[%s;|&(]")) then
			push(out, i, n, C.comment)
			break
		elseif b == 39 then
			local q = find(line, "'", i + 1, true)
			if not q then
				push(out, i, n, C.string)
				return out, "q"
			end
			push(out, i, q, C.string)
			i = q + 1
			command = false
		elseif b == 34 then
			push(out, i, i, C.string)
			local p = sh_double(line, i + 1, out)
			if not p then
				return out, "d"
			end
			i = p
			command = false
		elseif b == 36 then
			if byte(line, i + 1) == 39 then
				-- $'...': escapes like C's.
				local j = i + 2
				local from = i
				while j <= n and byte(line, j) ~= 39 do
					if byte(line, j) == 92 then
						push(out, from, j - 1, C.string)
						push(out, j, j + 1, C.escape)
						j += 2
						from = j
					else
						j += 1
					end
				end
				push(out, from, math.min(j, n), C.string)
				i = j + 1
			else
				i = sh_dollar(line, i, out)
			end
			command = false
		elseif b == 60 and byte(line, i + 1) == 60 and byte(line, i + 2) ~= 60 then
			local _, e, dash, delim = find(line, "^<<(%-?)%s*['\"]?([%w_]+)['\"]?", i)
			if e and dash and delim then
				push(out, i, i + 1 + #dash, C.operator)
				push(out, i + 2 + #dash, e, C.label)
				heredoc = "H" .. dash .. delim
				i = e + 1
			else
				push(out, i, i + 1, C.operator)
				i += 2
			end
		elseif find(line, "^[|&;()<>]", i) then
			local _, e = find(line, "^[|&;()<>]+", i)
			e = e :: number
			push(out, i, e, if find(sub(line, i, e), "[()]") then C.punctuation else C.operator)
			command = not find(sub(line, i, e), "^[<>]")
			i = e + 1
		else
			local s, e = find(line, "^[^%s|&;()<>'\"$#`]+", i)
			if not s or not e then
				i += 1
				continue
			end
			local w = sub(line, s, e)
			local eq = find(w, "=", 1, true)
			if (command or declaring) and eq and find(w, "^[%a_][%w_]*=") then
				-- An assignment: NAME=value, and the command still to come.
				push(out, s, s + eq - 2, C.variable)
				push(out, s + eq - 1, s + eq - 1, C.operator)
			elseif SH_KEYWORDS[w] then
				push(out, s, e, C.keyword)
				command = SH_COMMAND_AFTER[w] or false
				declaring = SH_DECLARE[w] or false
				i = e + 1
				continue
			elseif command then
				push(out, s, e, C["function"])
				command = SH_COMMAND_AFTER[w] or false
				i = e + 1
				continue
			elseif byte(w) == 45 then
				local x = find(w, "=", 1, true)
				push(out, s, if x then s + x - 2 else e, C.attribute)
			elseif find(w, "^%d+$") then
				push(out, s, e, C.number)
			end
			command = false
			i = e + 1
		end
	end
	return out, heredoc or 0
end

-- CSS -----------------------------------------------------------------------
-- Selectors outside a block (tags, .classes as types, #ids as constants,
-- :pseudo-classes as keywords); inside, `property: value`, the value's
-- numbers, colours, functions and custom properties coloured. States: "d<n>"
-- inside n braces, "c<n>" in a comment there (top level is 0 and "c0").

local CSS_IMPORTANT = "!important"

local function css_scan(line: string, i: number, out: { Span }, depth: number): (number?, number)
	local n = #line
	local value = false
	-- Inside a block, what runs into a `{` before a `;` or `}` is a nested
	-- rule's selector (SCSS, @media); anything else is a declaration.
	local function selector(at: number): boolean
		local p = find(line, "[{};]", at)
		return p ~= nil and byte(line, p) == 123
	end
	while i <= n do
		local b = byte(line, i)
		if b == 32 or b == 9 then
			i += 1
		elseif b == 47 and byte(line, i + 1) == 42 then
			local _, e = find(line, "*/", i + 2, true)
			if not e then
				push(out, i, n, C.comment)
				return nil, depth
			end
			push(out, i, e, C.comment)
			i = e + 1
		elseif b == 47 and byte(line, i + 1) == 47 and depth > 0 then
			push(out, i, n, C.comment)
			return n + 1, depth
		elseif b == 34 or b == 39 then
			local q = find(line, string.char(b), i + 1, true) or n
			push(out, i, q, C.string)
			i = q + 1
		elseif b == 123 then
			push(out, i, i, C.punctuation)
			depth += 1
			value = false
			i += 1
		elseif b == 125 then
			push(out, i, i, C.punctuation)
			depth = math.max(depth - 1, 0)
			value = false
			i += 1
		elseif b == 59 then
			push(out, i, i, C.punctuation)
			value = false
			i += 1
		elseif b == 64 then
			local _, e = find(line, "^@[%w%-]+", i)
			e = e or i
			push(out, i, e, C.keyword)
			i = e + 1
		elseif b == 33 and sub(line, i, i + #CSS_IMPORTANT - 1) == CSS_IMPORTANT then
			push(out, i, i + #CSS_IMPORTANT - 1, C.keyword)
			i += #CSS_IMPORTANT
		elseif b == 35 and (value or (depth > 0 and not selector(i))) then
			local _, e = find(line, "^#%x+", i)
			e = e or i
			push(out, i, e, C.number)
			i = e + 1
		elseif depth > 0 and (value or not selector(i)) then
			-- A declaration.
			if lex.is_digit(b) or ((b == 46 or b == 45 or b == 43) and lex.is_digit(byte(line, i + 1))) then
				local _, e = find(line, "^[%+%-]?%d*%.?%d+[eE]?%d*", i)
				e = e or i
				local _, u = find(line, "^[%a%%]+", e + 1)
				push(out, i, u or e, C.number)
				i = (u or e) + 1
			else
				local s, e = find(line, "^%-?%-?[%a_][%w_%-]*", i)
				if s and e then
					local w = sub(line, s, e)
					if not value and find(line, "^%s*:", e + 1) then
						push(out, s, e, if sub(w, 1, 2) == "--" then C.variable else C.property)
					elseif byte(line, e + 1) == 40 then
						push(out, s, e, C["function"])
					elseif sub(w, 1, 2) == "--" then
						push(out, s, e, C.variable)
					end
					-- A value's keywords (solid, auto, ease-out) stay in the
					-- text's ink, so its numbers and colours stand out.
					i = e + 1
				elseif b == 58 then
					push(out, i, i, C.punctuation)
					value = true
					i += 1
				else
					if find(sub(line, i, i), "[%(%),]") then
						push(out, i, i, C.punctuation)
					elseif lex.OPERATOR[b] then
						push(out, i, i, C.operator)
					end
					i += 1
				end
			end
		else
			-- A selector.
			local s, e = find(line, "^[%.#]?%-?[%a_][%w_%-]*", i)
			if s and e then
				local first = byte(line, s)
				push(out, s, e, if first == 46 then C.type elseif first == 35 then C.constant else C.tag)
				i = e + 1
			elseif b == 58 then
				local _, e2 = find(line, "^::?[%w%-]+", i)
				e2 = e2 or i
				push(out, i, e2, C.keyword)
				i = e2 + 1
			elseif b == 91 then
				local q = find(line, "]", i, true) or n
				push(out, i, q, C.attribute)
				i = q + 1
			elseif b == 38 or b == 42 then
				push(out, i, i, C.keyword)
				i += 1
			else
				if find(sub(line, i, i), "[>+~,%(%)]") then
					push(out, i, i, C.operator)
				end
				i += 1
			end
		end
	end
	return n + 1, depth
end

function M.css(line: string, state: any): ({ Span }, any)
	local out: { Span } = {}
	local depth = 0
	local i = 1
	if type(state) == "string" then
		depth = tonumber(sub(state, 2)) or 0
		if sub(state, 1, 1) == "c" then
			local _, e = find(line, "*/", 1, true)
			if not e then
				push(out, 1, #line, C.comment)
				return out, state
			end
			push(out, 1, e, C.comment)
			i = e + 1
		end
	end
	local p, d = css_scan(line, i, out, depth)
	if not p then
		return out, "c" .. d
	end
	return out, if d == 0 then 0 else "d" .. d
end

-- HTML ----------------------------------------------------------------------
-- Tags, attributes and their values, comments, entities; a script's body is
-- JavaScript and a style's CSS, their states carried inside HTML's
-- ("S<js state>", "Y<css state>"). Inside a tag across lines: "T<name>".

local function html_attrs(line: string, j: number, out: { Span }, name: string): (number?, any)
	local n = #line
	while j <= n do
		local b = byte(line, j)
		if b == 62 then
			push(out, j, j, C.punctuation)
			local l = lower(name)
			if l == "script" then
				return j + 1, "S0"
			elseif l == "style" then
				return j + 1, "Y0"
			end
			return j + 1, 0
		elseif b == 47 and byte(line, j + 1) == 62 then
			push(out, j, j + 1, C.punctuation)
			return j + 2, 0
		elseif b == 34 or b == 39 then
			local q = find(line, string.char(b), j + 1, true) or n
			push(out, j, q, C.string)
			j = q + 1
		elseif b == 61 then
			push(out, j, j, C.operator)
			local _, e = find(line, "^[^%s\"'>=`]+", j + 1)
			if e then
				push(out, j + 1, e, C.string)
				j = e + 1
			else
				j += 1
			end
		else
			local s, e = find(line, "^[^%s\"'>/=]+", j)
			if s and e then
				push(out, s, e, C.attribute)
				j = e + 1
			else
				j += 1
			end
		end
	end
	return nil, "T" .. name
end

-- An embedded body (a script's, a style's) from `j` to its closing tag.
local function html_embed(
	line: string,
	j: number,
	out: { Span },
	inner: Highlighter,
	state: any,
	closer: string,
	tag: string
): (number?, any)
	local close = find(lower(line), closer, j, true)
	local body = sub(line, j, if close then close - 1 else #line)
	local spans, st = inner(body, state)
	lex.embed(out, spans, j)
	if not close then
		return nil, tag .. lex.encode(st)
	end
	return close, 0
end

local function html(js: Highlighter): Highlighter
	return function(line: string, state: any): ({ Span }, any)
		local out: { Span } = {}
		local n = #line
		local i = 1
		if type(state) == "string" then
			local tag = sub(state, 1, 1)
			local rest = sub(state, 2)
			local p: number?, st: any
			if tag == "C" then
				local _, e = find(line, "-->", 1, true)
				if e then
					push(out, 1, e, C.comment)
					p = e + 1
				else
					push(out, 1, n, C.comment)
					return out, state
				end
			elseif tag == "T" then
				p, st = html_attrs(line, 1, out, rest)
			elseif tag == "S" then
				p, st = html_embed(line, 1, out, js, lex.decode(rest), "</script", "S")
			elseif tag == "Y" then
				p, st = html_embed(line, 1, out, M.css, lex.decode(rest), "</style", "Y")
			end
			if not p then
				return out, st
			end
			local at: number = p
			if st ~= nil and st ~= 0 then
				-- Into a script or a style mid-line.
				local q, s2 = html_embed(
					line,
					p,
					out,
					if sub(st, 1, 1) == "S" then js else M.css,
					0,
					if sub(st, 1, 1) == "S" then "</script" else "</style",
					sub(st, 1, 1)
				)
				if not q then
					return out, s2
				end
				at = q
			end
			i = at
		end
		while i <= n do
			local lt = find(line, "[<&]", i)
			if not lt then
				break
			end
			if byte(line, lt) == 38 then
				local _, e = find(line, "^&#?[%w]+;", lt)
				if e then
					push(out, lt, e, C.escape)
					i = e + 1
				else
					i = lt + 1
				end
			elseif sub(line, lt, lt + 3) == "<!--" then
				local _, e = find(line, "-->", lt + 4, true)
				if not e then
					push(out, lt, n, C.comment)
					return out, "C"
				end
				push(out, lt, e, C.comment)
				i = e + 1
			elseif byte(line, lt + 1) == 33 or byte(line, lt + 1) == 63 then
				-- <!DOCTYPE ...>, <?xml ...?>
				local e = find(line, ">", lt, true) or n
				push(out, lt, e, C.meta)
				i = e + 1
			else
				local closing = byte(line, lt + 1) == 47
				local s, e = find(line, "^[%w_:%-%.]+", lt + (if closing then 2 else 1))
				if not s or not e then
					i = lt + 1
					continue
				end
				push(out, lt, s - 1, C.punctuation)
				push(out, s, e, C.tag)
				local p, st = html_attrs(line, e + 1, out, if closing then "" else sub(line, s, e))
				if not p then
					return out, st
				end
				i = p
				if st ~= 0 then
					local s1 = sub(st, 1, 1)
					local q, s2 = html_embed(
						line,
						i,
						out,
						if s1 == "S" then js else M.css,
						0,
						if s1 == "S" then "</script" else "</style",
						s1
					)
					if not q then
						return out, s2
					end
					i = q
				end
			end
		end
		return out, 0
	end
end
M.html = html

-- YAML ----------------------------------------------------------------------
-- `key:` as properties, scalars as strings, numbers and constants as such,
-- anchors and aliases as labels, tags as types. A block scalar (`key: |`)
-- runs over every line indented past its key: state "B<indent>".

local YAML_CONSTANTS = set("true false null yes no on off True False Null TRUE FALSE NULL ~")

local function yaml_value(line: string, i: number, out: { Span })
	local n = #line
	while i <= n do
		local b = byte(line, i)
		if b == 32 or b == 9 then
			i += 1
		elseif b == 35 and find(sub(line, i - 1, i - 1), "%s") then
			push(out, i, n, C.comment)
			return
		elseif b == 34 or b == 39 then
			local q = i + 1
			while q <= n do
				local c = byte(line, q)
				if c == 92 and b == 34 then
					q += 2
				elseif c == b then
					break
				else
					q += 1
				end
			end
			push(out, i, math.min(q, n), C.string)
			i = q + 1
		elseif b == 38 or b == 42 then
			local _, e = find(line, "^[&*][^%s,%[%]{}]+", i)
			e = e or i
			push(out, i, e, C.label)
			i = e + 1
		elseif b == 33 then
			local _, e = find(line, "^!+[^%s,%[%]{}]*", i)
			e = e or i
			push(out, i, e, C.type)
			i = e + 1
		elseif find(sub(line, i, i), "[%[%]{},]") then
			push(out, i, i, C.punctuation)
			i += 1
		elseif b == 124 or b == 62 then
			local _, e = find(line, "^[|>][%-%+%d]*", i)
			e = e or i
			push(out, i, e, C.operator)
			i = e + 1
		else
			-- A key in a flow mapping, or a plain scalar to the next comment
			-- or flow punctuation.
			local _, e = find(line, "^[^,%[%]{}#]*", i)
			e = e or i
			local text = sub(line, i, e)
			local key = string.match(text, "^([^:]-):%s") or string.match(text, "^([^:]-):$")
			if key and #key > 0 then
				push(out, i, i + #key - 1, C.property)
				push(out, i + #key, i + #key, C.punctuation)
				i += #key + 1
				continue
			end
			local trimmed = string.match(text, "^(.-)%s*$") or text
			local last = i + #trimmed - 1
			if YAML_CONSTANTS[trimmed] then
				push(out, i, last, C.constant)
			elseif find(trimmed, "^[%-%+]?%.?%d[%d_%.eExXoO%x%-:]*$") then
				push(out, i, last, C.number)
			else
				push(out, i, last, C.string)
			end
			-- A # inside a scalar starts a comment only after a space.
			i = e + 1
			if byte(line, i) == 35 and not find(sub(line, i - 1, i - 1), "%s") then
				local _, more = find(line, "^#[^%s]*", i)
				push(out, i, more or i, C.string)
				i = (more or i) + 1
			end
		end
	end
end

function M.yaml(line: string, state: any): ({ Span }, any)
	local out: { Span } = {}
	local n = #line
	local _, lead = find(line, "^ *")
	local indent = lead :: number
	if type(state) == "string" then
		local block = tonumber(sub(state, 2)) or 0
		if indent >= n or indent > block then
			push(out, indent + 1, n, C.string)
			return out, state
		end
	end
	local i = indent + 1
	if find(line, "^%-%-%-") or find(line, "^%.%.%.") then
		push(out, 1, 3, C.meta)
		yaml_value(line, 4, out)
		return out, 0
	end
	if byte(line, i) == 35 then
		push(out, i, n, C.comment)
		return out, 0
	end
	-- List markers.
	while find(line, "^%-%s", i) or (byte(line, i) == 45 and i == n) do
		push(out, i, i, C.punctuation)
		local _, e = find(line, "^%-%s*", i)
		i = (e :: number) + 1
	end
	local key_from = i
	local s, e, key = find(line, "^([^%s#'\"%[{][^#]-)%s*:%s", i)
	if not s then
		s, e, key = find(line, "^([^%s#'\"%[{][^#]-)%s*:$", i)
	end
	if not s then
		s, e, key = find(line, "^(\"[^\"]*\")%s*:", i)
	end
	if not s then
		s, e, key = find(line, "^('[^']*')%s*:", i)
	end
	if s and e and key then
		push(out, key_from, key_from + #key - 1, C.property)
		push(out, e - (if byte(line, e) == 58 then 0 else 1), e, C.punctuation)
		i = e + 1
	end
	local rest = sub(line, i)
	yaml_value(line, i, out)
	if find(rest, "^%s*[|>][%-%+%d]*%s*$") or find(rest, "^%s*[|>][%-%+%d]*%s+#") then
		return out, "B" .. indent
	end
	return out, 0
end

-- Markdown ------------------------------------------------------------------
-- Headings, emphasis, code, links, lists, quotes, tables; a fenced block's
-- code coloured as its language, its state carried inside markdown's:
-- "F<fence>|<language>|<state>". An HTML comment over lines: "C".

local function md_inline(line: string, i: number, out: { Span }, pipes: boolean?)
	local n = #line
	while i <= n do
		local p = find(line, if pipes then "[`*_~%[!<\\h|]" else "[`*_~%[!<\\h]", i)
		if not p then
			break
		end
		local b = byte(line, p)
		local handled = false
		if b == 124 then
			push(out, p, p, C.punctuation)
			i = p + 1
			handled = true
		elseif b == 96 then
			local _, ticks_e = find(line, "^`+", p)
			local ticks = sub(line, p, ticks_e :: number)
			local close = find(line, ticks, (ticks_e :: number) + 1, true)
			if close then
				push(out, p, (ticks_e :: number), C.punctuation)
				push(out, (ticks_e :: number) + 1, close - 1, C.string)
				push(out, close, close + #ticks - 1, C.punctuation)
				i = close + #ticks
				handled = true
			end
		elseif b == 92 and find(line, "^\\[%p]", p) then
			push(out, p, p + 1, C.escape)
			i = p + 2
			handled = true
		elseif b == 42 or b == 95 then
			local _, e = find(line, "^[*_]+", p)
			local run = sub(line, p, e :: number)
			-- Intraword underscores (snake_case) aren't emphasis.
			local before = byte(line, p - 1)
			if not (b == 95 and before and lex.is_alpha(before) or before and lex.is_digit(before)) and #run <= 3 then
				local close = find(line, run, (e :: number) + 1, true)
				if close and close > (e :: number) + 1 and not find(sub(line, (e :: number) + 1, (e :: number) + 1), "%s") then
					local color = if #run >= 2 then C.strong else C.emphasis
					push(out, p, close + #run - 1, color)
					i = close + #run
					handled = true
				end
			end
			if not handled then
				i = (e :: number) + 1
				handled = true
			end
		elseif b == 126 and byte(line, p + 1) == 126 then
			local close = find(line, "~~", p + 2, true)
			if close then
				push(out, p, close + 1, C.deleted)
				i = close + 2
				handled = true
			end
		elseif b == 91 or (b == 33 and byte(line, p + 1) == 91) then
			local o = if b == 33 then p + 1 else p
			local _, e = find(line, "^%b[]", o)
			if e then
				local _, u = find(line, "^%b()", e + 1)
				if not u then
					_, u = find(line, "^%b[]", e + 1)
				end
				if u then
					push(out, p, o, C.punctuation)
					push(out, o + 1, e - 1, C.link)
					push(out, e, e, C.punctuation)
					push(out, e + 1, u, C.url)
					i = u + 1
					handled = true
				elseif byte(line, e + 1) == 58 then
					-- A reference definition: [id]: url
					push(out, p, e, C.link)
					push(out, e + 1, n, C.url)
					return
				end
			end
		elseif b == 60 then
			local _, e = find(line, "^<https?://[^>%s]+>", p)
			if e then
				push(out, p, e, C.url)
				i = e + 1
				handled = true
			else
				_, e = find(line, "^</?[%a][%w%-]*[^>]*>", p)
				if e then
					push(out, p, e, C.tag)
					i = e + 1
					handled = true
				end
			end
		elseif b == 104 then
			local _, e = find(line, "^https?://[^%s<>%)]+", p)
			if e and (p == 1 or not lex.is_alpha(byte(line, p - 1))) then
				push(out, p, e, C.url)
				i = e + 1
				handled = true
			end
		end
		if not handled then
			i = p + 1
		end
	end
end

local function markdown(resolve: (string) -> Highlighter?): Highlighter
	return function(line: string, state: any): ({ Span }, any)
		local out: { Span } = {}
		local n = #line
		if type(state) == "string" then
			if state == "C" then
				local _, e = find(line, "-->", 1, true)
				if not e then
					push(out, 1, n, C.comment)
					return out, state
				end
				push(out, 1, e, C.comment)
				md_inline(line, e + 1, out)
				return out, 0
			end
			local _, _, fence, lang, inner = find(state, "^F([`~]+)|([^|]*)|(.*)$")
			if fence and lang and inner then
				local _, e = find(line, "^%s*" .. (if sub(fence, 1, 1) == "`" then "`" else "~") .. "+%s*$")
				local _, run = find(line, "^%s*[`~]+")
				local marks = string.match(line, "[`~]+")
				if e and run and marks and #marks >= #fence then
					push(out, 1, n, C.punctuation)
					return out, 0
				end
				local h = resolve(lang)
				if not h then
					return out, state
				end
				local spans, st = h(line, lex.decode(inner))
				return spans, "F" .. fence .. "|" .. lang .. "|" .. lex.encode(st)
			end
		end
		local _, lead = find(line, "^%s*")
		local i = (lead :: number) + 1
		-- A fence opening.
		local _, fe, fence, info = find(line, "^%s*(```+)%s*([^`]*)$")
		if not fe then
			_, fe, fence, info = find(line, "^%s*(~~~+)%s*(.*)$")
		end
		if fe and fence and info then
			local lang = lower(string.match(info, "^[{%.]*([%w_+#%-]*)") or "")
			push(out, i, i + #fence - 1, C.punctuation)
			push(out, i + #fence, n, C.label)
			return out, "F" .. fence .. "|" .. lang .. "|0"
		end
		local b = byte(line, i)
		if b == 35 then
			local _, e = find(line, "^#+%s", i)
			if e or find(line, "^#+$", i) then
				push(out, i, n, C.heading)
				return out, 0
			end
		end
		-- A rule, or a setext heading's underline.
		if find(line, "^%s*[-*_=][-*_=%s]*$") and #(string.gsub(line, "%s", "")) >= 3 then
			push(out, 1, n, C.punctuation)
			return out, 0
		end
		if sub(line, i, i + 3) == "<!--" then
			local _, e = find(line, "-->", i + 4, true)
			if not e then
				push(out, i, n, C.comment)
				return out, "C"
			end
			push(out, i, e, C.comment)
			md_inline(line, e + 1, out)
			return out, 0
		end
		-- Quotes, then list markers (and task boxes).
		while byte(line, i) == 62 do
			push(out, i, i, C.quote)
			local _, e = find(line, "^>%s*", i)
			i = (e :: number) + 1
		end
		local _, me = find(line, "^[-*+]%s+", i)
		if not me then
			_, me = find(line, "^%d+[.)]%s+", i)
		end
		if me then
			push(out, i, me, C.keyword)
			i = me + 1
			local _, te = find(line, "^%[[ xX]%]%s", i)
			if te then
				push(out, i, te, C.keyword)
				i = te + 1
			end
		end
		-- A table row: its pipes, and a delimiter row whole.
		local row = find(line, "^%s*|") ~= nil
		if row and find(line, "^[%s|:%-]+$") then
			push(out, 1, n, C.punctuation)
			return out, 0
		end
		md_inline(line, i, out, row)
		return out, 0
	end
end
M.markdown = markdown

-- Diffs ---------------------------------------------------------------------
-- Additions, deletions, hunks and file headers, by each line's first bytes.

function M.diff(line: string, _state: any): ({ Span }, any)
	local n = #line
	if n == 0 then
		return {}, 0
	end
	local b = byte(line)
	if
		find(line, "^diff ")
		or find(line, "^index ")
		or find(line, "^%-%-%- ")
		or find(line, "^%+%+%+ ")
		or find(line, "^new file")
		or find(line, "^deleted file")
		or find(line, "^similarity")
		or find(line, "^rename ")
		or find(line, "^old mode")
		or find(line, "^new mode")
		or find(line, "^Binary files")
	then
		return { { 1, n, C.meta } }, 0
	elseif b == 64 and byte(line, 2) == 64 then
		local _, e = find(line, "^@@[^@]*@@")
		e = e or n
		local out = { { 1, e, C.hunk } }
		if e < n then
			out[2] = { e + 1, n, C.comment }
		end
		return out, 0
	elseif b == 43 then
		return { { 1, n, C.inserted } }, 0
	elseif b == 45 then
		return { { 1, n, C.deleted } }, 0
	elseif b == 92 then
		return { { 1, n, C.comment } }, 0
	end
	return {}, 0
end

return M
