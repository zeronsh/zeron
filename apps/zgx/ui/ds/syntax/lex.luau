-- The shared machinery of every language's highlighter: the colours, span
-- building, and the lexer for C-like code driven by a language's table of
-- rules (its comments, strings, words and quirks). Languages with a shape
-- of their own (markup, shell, data) build on the helpers here.
--
-- A highlighter takes a line and what the line before left open, and
-- returns the line's spans (`{ from, to, color }`, bytes from 1) and what it
-- leaves the next line. What a line leaves is 0 (nothing open) or a short
-- string naming the open thing exactly (`"c2"`: a block comment two deep;
-- `"R]==]"`: a raw string waiting for `]==]`), so two lines leaving the
-- same thing compare equal and the editor stops re-colouring as soon as an
-- edit's effect settles.

local M = {}

export type Span = { any }
export type Highlighter = (line: string, state: any) -> ({ Span }, any)

-- Every colour a highlighter gives: theme tokens, so each theme colours them
-- its own way (tokens.luau declares them; the themes pick palette hues).
local C = {
	text = "$zeron.syntax.text",
	keyword = "$zeron.syntax.keyword",
	string = "$zeron.syntax.string",
	escape = "$zeron.syntax.escape",
	regex = "$zeron.syntax.regex",
	number = "$zeron.syntax.number",
	constant = "$zeron.syntax.constant",
	comment = "$zeron.syntax.comment",
	["function"] = "$zeron.syntax.function",
	macro = "$zeron.syntax.macro",
	type = "$zeron.syntax.type",
	variable = "$zeron.syntax.variable",
	property = "$zeron.syntax.property",
	attribute = "$zeron.syntax.attribute",
	label = "$zeron.syntax.label",
	operator = "$zeron.syntax.operator",
	punctuation = "$zeron.syntax.punctuation",
	tag = "$zeron.syntax.tag",
	heading = "$zeron.syntax.heading",
	link = "$zeron.syntax.link",
	url = "$zeron.syntax.url",
	emphasis = "$zeron.syntax.emphasis",
	strong = "$zeron.syntax.strong",
	quote = "$zeron.syntax.quote",
	inserted = "$zeron.syntax.inserted",
	deleted = "$zeron.syntax.deleted",
	hunk = "$zeron.syntax.hunk",
	meta = "$zeron.syntax.meta",
}
M.C = C

local byte, find, sub, lower = string.byte, string.find, string.sub, string.lower

-- A set from words separated by spaces.
function M.set(words: string): { [string]: boolean }
	local out = {}
	for word in string.gmatch(words, "%S+") do
		out[word] = true
	end
	return out
end
local EMPTY: { [string]: boolean } = {}

-- Adds a span, folding it into the last when they touch in the same colour,
-- so a run of operators or a string with no escapes is one span.
local function push(out: { Span }, a: number, b: number, color: string)
	if b < a then
		return
	end
	local last = out[#out]
	if last and last[3] == color and last[2] + 1 >= a then
		if b > last[2] then
			last[2] = b
		end
		return
	end
	out[#out + 1] = { a, b, color }
end
M.push = push

-- Adds another highlighter's spans for the part of a line from `at`
-- (embedded code: a script in HTML, a fenced block in markdown).
function M.embed(out: { Span }, spans: { Span }, at: number)
	local shift = at - 1
	for _, s in spans do
		push(out, s[1] + shift, s[2] + shift, s[3])
	end
end

-- Embedded states inside a state string: 0 is written "0".
function M.encode(state: any): string
	return if state == 0 or state == nil then "0" else tostring(state)
end
function M.decode(s: string): any
	return if s == "0" or s == "" then 0 else s
end

local BACKSLASH, SLASH, STAR, QUOTE, APOS = 92, 47, 42, 34, 39
local LT, GT, EQ, LBRACE, RBRACE, LPAREN, RPAREN = 60, 62, 61, 123, 125, 40, 41
local LBRACK, RBRACK, DOT, HASH, AT, BANG, DOLLAR, COLON = 91, 93, 46, 35, 64, 33, 36, 58

local function is_digit(b: number?): boolean
	return b ~= nil and b >= 48 and b <= 57
end
local function is_alpha(b: number?): boolean
	return b ~= nil and ((b >= 97 and b <= 122) or (b >= 65 and b <= 90) or b == 95)
end
M.is_digit = is_digit
M.is_alpha = is_alpha

-- Operators and punctuation by byte.
local OPERATOR: { [number]: boolean } = {}
for _, b in { string.byte("+-*/%=<>!&|^~?:", 1, -1) } do
	OPERATOR[b] = true
end
local PUNCT: { [number]: boolean } = {}
for _, b in { string.byte("(){}[];,.", 1, -1) } do
	PUNCT[b] = true
end
M.OPERATOR = OPERATOR
M.PUNCT = PUNCT

-- An escape in a string from `i` (its backslash): where it ends.
local function escape_end(line: string, i: number): number
	local _, e = find(line, "^\\u%{%x+%}", i)
	if e then
		return e
	end
	_, e = find(line, "^\\u%x%x%x%x", i)
	if e then
		return e
	end
	_, e = find(line, "^\\U%x%x%x%x%x%x%x%x", i)
	if e then
		return e
	end
	_, e = find(line, "^\\x%x%x", i)
	if e then
		return e
	end
	_, e = find(line, "^\\[0-7][0-7]?[0-7]?", i)
	if e then
		return e
	end
	return math.min(i + 1, #line)
end
M.escape_end = escape_end

-- A number from `i`: where it ends (0x1F, 0b1010, 1_000, 3.14e-2, 10u32,
-- 2.5f), or nil. A `..` after digits (a range) is not a fraction.
local function number_end(line: string, i: number): number?
	local _, e = find(line, "^0[xX][%x_]+", i)
	if not e then
		_, e = find(line, "^0[bBoO][%x_]+", i)
	end
	if not e then
		_, e = find(line, "^%d[%d_]*", i)
		if e then
			if byte(line, e + 1) == DOT and is_digit(byte(line, e + 2)) then
				_, e = find(line, "^[%d_]*", e + 2)
			elseif byte(line, e + 1) == DOT and not is_alpha(byte(line, e + 2)) and byte(line, e + 2) ~= DOT then
				e += 1
			end
		else
			_, e = find(line, "^%.%d[%d_]*", i)
		end
		if e then
			local _, x = find(line, "^[eE][%+%-]?%d+", (e :: number) + 1)
			e = x or e
		end
	end
	if not e then
		return nil
	end
	local _, s = find(line, "^[%a_][%w_]*", e + 1)
	return s or e
end
M.number_end = number_end

-- The lexer for C-like languages. `spec` names the language's rules:
--   line        line comment openers ({ "//" })
--   block       a block comment's { open, close }; `nest` if they nest
--   strings     string kinds, longest opener first: { open, close?, escape?
--               (true unless false), multi? (runs past a line's end),
--               interp? ("${", "{", "\\(", "$", "#{": code inside), raw? }
--   prefixes    words that make the quote after them a string variant
--               ("r": no escapes, "f": `{}` interpolation, as Python's)
--   keywords, types, constants, variables (self, this)    word sets
--   defs        keywords naming a function next (fn, def, func)
--   decls       keywords naming a type next (struct, class, enum)
--   fold        words match whatever their case (SQL)
--   ident       the identifier pattern
--   caps_type   a Capitalised word is a type; ALL_CAPS is a constant
--   chars       `'x'` is a character ('a, a Rust lifetime, if `lifetimes`)
--   macros      `name!` is a macro (Rust)
--   attributes  `#[...]` is an attribute (Rust)
--   decorators  `@name` is an attribute (Python, Swift, Kotlin, TS, WGSL)
--   preprocessor  `#word` starting a line is a directive (C, C#)
--   long        Lua's long brackets: `[[...]]` strings, `--[[...]]` comments
--   rust_raw    Rust's `r#"..."#` (and `b`, `br`, `c` before a quote)
--   cpp_raw     C++'s `R"x(...)x"`
--   format      `{}` in strings is a placeholder (Rust's format strings)
--   regex       `/.../flags` where a value can't end (JavaScript)
--   jsx         `<Tag attr={...}>children</Tag>` (JSX, TSX)
--   key         a string or word before this (":" in JSON, "=" in TOML) is a key
--   headers     `[table]` starting a line is a heading (TOML, INI)
--   number      a pattern for numbers instead of the usual (TOML's dates)
function M.code(spec: { [string]: any }): Highlighter
	local keywords = spec.keywords or EMPTY
	local types = spec.types or EMPTY
	local constants = spec.constants or EMPTY
	local variables = spec.variables or EMPTY
	local defs = spec.defs or EMPTY
	local decls = spec.decls or EMPTY
	local prefixes = spec.prefixes or EMPTY
	local fold = spec.fold
	local ident = spec.ident or "^[%a_][%w_]*"
	local line_comments: { string } = spec.line or {}
	local block = spec.block
	local nest = spec.nest
	local kinds: { { [string]: any } } = spec.strings or {}
	local caps = spec.caps_type
	local key_suffix: string? = spec.key
	local number_pattern: string? = spec.number

	-- What can start something other than a plain word or symbol, so most
	-- bytes skip every check below.
	local starts: { [number]: boolean } = {}
	for _, open in line_comments do
		starts[byte(open)] = true
	end
	if block then
		starts[byte(block[1])] = true
	end
	for _, k in kinds do
		starts[byte(k.open)] = true
		k.close = k.close or k.open
		if k.escape == nil then
			k.escape = true
		end
	end

	-- A kind made a variant by a prefix (`r"..."`, `f'...'`), made once.
	local variants: { [string]: { [string]: any } } = {}
	local function kind_of(base: number, flags: string): { [string]: any }
		if flags == "" then
			return kinds[base]
		end
		local key = base .. flags
		local k = variants[key]
		if not k then
			local b = kinds[base]
			k = { open = b.open, close = b.close, escape = b.escape, multi = b.multi, interp = b.interp }
			if find(flags, "r", 1, true) then
				k.escape = false
			end
			if find(flags, "f", 1, true) then
				k.interp = "{"
			end
			variants[key] = k
		end
		return k
	end

	local scan: (line: string, i: number, out: { Span }, stop: number?, depth: number?) -> (number?, any, number)
	local jsx: (line: string, j: number, out: { Span }, mode: string, depth: number, braces: number) -> (number?, any)

	-- A string's body from `j` (past its opener): where it ends, or nil and
	-- what the line leaves.
	local function str(line: string, j: number, out: { Span }, base: number, flags: string): (number?, any)
		local k = kind_of(base, flags)
		local close: string = k.close
		local c1 = byte(close)
		local interp: string? = k.interp
		local i1 = interp and byte(interp)
		local esc = k.escape
		local format = spec.format and close == '"'
		local n = #line
		local from = j
		while j <= n do
			local b = byte(line, j)
			if b == BACKSLASH and esc and not (interp and b == i1 and sub(line, j, j + #interp - 1) == interp) then
				push(out, from, j - 1, C.string)
				if j == n then
					-- A backslash ending the line runs the string on.
					push(out, j, j, C.escape)
					return nil, "s" .. base .. flags
				end
				local e = escape_end(line, j)
				push(out, j, e, C.escape)
				j = e + 1
				from = j
			elseif b == c1 and sub(line, j, j + #close - 1) == close then
				push(out, from, j + #close - 1, C.string)
				return j + #close, 0
			elseif interp and b == i1 and sub(line, j, j + #interp - 1) == interp then
				local after = j + #interp
				if interp == "{" and byte(line, after) == LBRACE then
					-- `{{` in an f-string is a brace.
					push(out, from, j - 1, C.string)
					push(out, j, after, C.escape)
					j = after + 1
					from = j
				elseif interp == "$" then
					local _, e = find(line, "^[%a_][%w_]*", after)
					if e then
						push(out, from, j - 1, C.string)
						push(out, j, e, C.variable)
						j = e + 1
						from = j
					elseif byte(line, after) == LBRACE then
						push(out, from, j - 1, C.string)
						push(out, j, after, C.punctuation)
						local stop, st = scan(line, after + 1, out, RBRACE)
						if not stop then
							return nil, if st == 0 then "i" .. base .. flags else st
						end
						push(out, stop, stop, C.punctuation)
						j = stop + 1
						from = j
					else
						j += 1
					end
				else
					push(out, from, j - 1, C.string)
					push(out, j, after - 1, C.punctuation)
					local closer = if interp == "\\(" then RPAREN else RBRACE
					local stop, st = scan(line, after, out, closer)
					if not stop then
						return nil, if st == 0 then "i" .. base .. flags else st
					end
					push(out, stop, stop, C.punctuation)
					j = stop + 1
					from = j
				end
			elseif format and b == LBRACE then
				local _, e = find(line, "^{[%w_:%?%.#<>%^%+%-%$]*}", j)
				if byte(line, j + 1) == LBRACE then
					e = j + 1
				end
				if e then
					push(out, from, j - 1, C.string)
					push(out, j, e, C.escape)
					j = e + 1
					from = j
				else
					j += 1
				end
			else
				j += 1
			end
		end
		push(out, from, n, C.string)
		if k.multi then
			return nil, "s" .. base .. flags
		end
		return nil, 0
	end

	-- A raw string or comment from `j`, waiting for `close`.
	local function raw(line: string, j: number, out: { Span }, close: string, color: string, tag: string): (number?, any)
		local _, e = find(line, close, j, true)
		if e then
			push(out, j, e, color)
			return e + 1, 0
		end
		push(out, j, #line, color)
		return nil, tag .. close
	end

	-- A nesting block comment from `j`, `depth` deep.
	local function nested(line: string, j: number, out: { Span }, depth: number, start: number?): (number?, any)
		local open, close = block[1], block[2]
		local from = start or j
		while true do
			local a = find(line, open, j, true)
			local b = find(line, close, j, true)
			if not b then
				push(out, from, #line, C.comment)
				if a then
					-- Openers after the last closer still deepen it.
					local k = a
					while k do
						depth += 1
						k = find(line, open, k + #open, true)
					end
				end
				return nil, "c" .. depth
			end
			if a and a < b then
				depth += 1
				j = a + #open
			else
				depth -= 1
				j = b + #close
				if depth == 0 then
					push(out, from, j - 1, C.comment)
					return j, 0
				end
			end
		end
	end

	local function is_key(line: string, e: number): boolean
		if not key_suffix then
			return false
		end
		local _, k = find(line, "^%s*", e + 1)
		local at = (k :: number) + 1
		return sub(line, at, at + #key_suffix - 1) == key_suffix
			and byte(line, at + #key_suffix) ~= byte(key_suffix)
	end

	-- A regex literal from `i` (its slash): where it ends, or nil if it
	-- isn't one (a division after all).
	local function regex_end(line: string, i: number): number?
		local j = i + 1
		local class = false
		local n = #line
		while j <= n do
			local b = byte(line, j)
			if b == BACKSLASH then
				j += 2
			elseif b == LBRACK then
				class = true
				j += 1
			elseif b == RBRACK then
				class = false
				j += 1
			elseif b == SLASH and not class then
				if j == i + 1 then
					return nil
				end
				local _, e = find(line, "^[a-z]*", j + 1)
				return e
			else
				j += 1
			end
		end
		return nil
	end

	-- The code of a line from `i`. With `stop` (a closing brace or paren),
	-- returns at the first unmatched one: the end of an interpolation or a
	-- JSX expression. Returns where it stopped (or nil at the line's end),
	-- what the line leaves, and how many braces are open.
	function scan(line: string, i: number, out: { Span }, stop: number?, depth: number?): (number?, any, number)
		local n = #line
		local open = depth or 0
		local opener = if stop == RPAREN then LPAREN else LBRACE
		-- Whether the last thing could end a value: a slash after one is a
		-- division, a `<` a comparison; elsewhere a regex or a tag.
		local value = false
		-- The keyword before (`fn`, `struct`): names the next word.
		local naming: string? = nil
		local first = (select(2, find(line, "^%s*")) :: number) + 1
		while i <= n do
			local b = byte(line, i)
			if b == 32 or b == 9 then
				i += 1
				continue
			end
			if starts[b] then
				local done = false
				for _, c in line_comments do
					if b == byte(c) and sub(line, i, i + #c - 1) == c then
						if spec.long and c == "--" then
							local _, e, eq = find(line, "^%-%-%[(=*)%[", i)
							if e and eq then
								local p, st = raw(line, i, out, "]" .. eq .. "]", C.comment, "C")
								if not p then
									return nil, st, open
								end
								i = p
								done = true
								break
							end
						end
						push(out, i, n, C.comment)
						return nil, 0, open
					end
				end
				if done then
					continue
				end
				if block and b == byte(block[1]) and sub(line, i, i + #block[1] - 1) == block[1] then
					local p, st
					if nest then
						p, st = nested(line, i + #block[1], out, 1, i)
					else
						p, st = raw(line, i, out, block[2], C.comment, "C")
					end
					if not p then
						return nil, st, open
					end
					i = p
					continue
				end
				for base, k in kinds do
					if b == byte(k.open) and sub(line, i, i + #k.open - 1) == k.open then
						local p, st
						if key_suffix then
							-- A key ("name": in JSON): its whole string in the
							-- key's colour.
							local body: { Span } = {}
							p, st = str(line, i + #k.open, body, base, "")
							if p and is_key(line, p - 1) then
								push(out, i, p - 1, C.property)
							else
								push(out, i, i + #k.open - 1, C.string)
								for _, s in body do
									push(out, s[1], s[2], s[3])
								end
							end
						else
							push(out, i, i + #k.open - 1, C.string)
							p, st = str(line, i + #k.open, out, base, "")
						end
						if not p then
							return nil, st, open
						end
						i = p
						value = true
						done = true
						break
					end
				end
				if done then
					continue
				end
			end
			if spec.long and b == LBRACK then
				local _, e, eq = find(line, "^%[(=*)%[", i)
				if e and eq then
					local p, st = raw(line, i, out, "]" .. eq .. "]", C.string, "R")
					if not p then
						return nil, st, open
					end
					i = p
					value = true
					continue
				end
			end
			if spec.headers and b == LBRACK and i == first then
				local _, e = find(line, "^%[%[?[^%]]*%]%]?", i)
				e = e or n
				push(out, i, e, C.type)
				i = e + 1
				continue
			end
			if is_digit(b) or (b == DOT and is_digit(byte(line, i + 1)) and not value) then
				local matched: number? = nil
				local pattern = number_pattern
				if pattern then
					matched = select(2, find(line, pattern, i))
				end
				local e: number = matched or number_end(line, i) or i
				push(out, i, e, C.number)
				i = e + 1
				value = true
				continue
			end
			local s, e = find(line, ident, i)
			if s and e then
				local w = sub(line, s, e)
				local after = byte(line, e + 1)
				-- A prefixed string: r"...", f'...', b"...".
				if (after == QUOTE or after == APOS) and (prefixes[w] or prefixes[lower(w)]) then
					for base, k in kinds do
						if sub(line, e + 1, e + #k.open) == k.open then
							local flags = (if find(lower(w), "r", 1, true) then "r" else "")
								.. (if find(lower(w), "f", 1, true) then "f" else "")
							push(out, s, e + #k.open, C.string)
							local p, st = str(line, e + 1 + #k.open, out, base, flags)
							if not p then
								return nil, st, open
							end
							i = p
							break
						end
					end
					if i > e then
						value = true
						continue
					end
				end
				if spec.rust_raw and (w == "r" or w == "br" or w == "cr" or w == "b" or w == "c") then
					local _, q, hashes = find(line, '^(#*)"', e + 1)
					if q and hashes and (hashes == "" or (w ~= "b" and w ~= "c")) then
						if w == "b" or w == "c" then
							-- b"..." is an ordinary string with escapes.
							push(out, s, q, C.string)
							local p, st = str(line, q + 1, out, 1, "")
							if not p then
								return nil, st, open
							end
							i = p
						else
							push(out, s, q, C.string)
							local p, st = raw(line, q + 1, out, '"' .. hashes, C.string, "R")
							if not p then
								return nil, st, open
							end
							i = p
						end
						value = true
						continue
					end
				end
				if spec.cpp_raw and after == QUOTE and (w == "R" or w == "u8R" or w == "LR" or w == "uR" or w == "UR") then
					local _, q, delim = find(line, '^"([^%(%s]*)%(', e + 1)
					if q and delim then
						push(out, s, q, C.string)
						local p, st = raw(line, q + 1, out, ")" .. delim .. '"', C.string, "R")
						if not p then
							return nil, st, open
						end
						i = p
						value = true
						continue
					end
				end
				local key = if fold then lower(w) else w
				local prev = byte(line, s - 1)
				local member = prev == DOT and byte(line, s - 2) ~= DOT
				local color: string? = nil
				local _, paren = find(line, "^%s*%(", e + 1)
				-- `function M.name`, `fun String.ext`: the name is after the dot.
				local qualified = naming and (after == DOT or (spec.long and after == COLON))
				if member then
					color = if paren then C["function"] else C.property
				elseif qualified then
					color = nil
				elseif naming then
					color = if defs[naming] then C["function"] else C.type
				elseif keywords[key] then
					color = C.keyword
				elseif constants[key] then
					color = C.constant
				elseif variables[key] then
					color = C.variable
				elseif types[key] then
					color = C.type
				elseif spec.macros and after == BANG and byte(line, e + 2) ~= EQ then
					color = C.macro
					e += 1
				elseif paren then
					color = C["function"]
				elseif is_key(line, e) then
					color = C.property
				elseif caps then
					local c = byte(w)
					if c >= 65 and c <= 90 then
						if #w > 1 and not find(w, "%l") then
							color = C.constant
						else
							color = C.type
						end
					end
				end
				if color then
					-- The solver leaves the refinement of this branch-assigned local
					-- unsimplified (`(string?) & ~(false?)`); the check above proves it.
					push(out, s, e, color :: string)
				end
				if not qualified then
					naming = if defs[key] or decls[key] then key else nil
				end
				value = not keywords[key]
				i = e + 1
				continue
			end
			if b == APOS and spec.chars then
				local _, e = find(line, "^'\\[^']*'", i)
				if not e then
					_, e = find(line, "^'[^'\\][\128-\191]*'", i)
				end
				if e then
					local x = find(line, "\\", i, true)
					if x and x < e then
						push(out, i, x - 1, C.string)
						push(out, x, e - 1, C.escape)
						push(out, e, e, C.string)
					else
						push(out, i, e, C.string)
					end
					i = e + 1
					value = true
					continue
				end
				if spec.lifetimes then
					local _, l = find(line, "^'[%a_][%w_]*", i)
					if l then
						push(out, i, l, C.label)
						i = l + 1
						continue
					end
				end
			end
			if b == HASH and spec.attributes and (byte(line, i + 1) == LBRACK or sub(line, i + 1, i + 2) == "![") then
				-- To its matching bracket.
				local level, j = 0, i + 1
				while j <= n do
					local c = byte(line, j)
					if c == LBRACK then
						level += 1
					elseif c == RBRACK then
						level -= 1
						if level == 0 then
							break
						end
					end
					j += 1
				end
				push(out, i, math.min(j, n), C.attribute)
				i = j + 1
				continue
			end
			if b == HASH and spec.preprocessor and i == first then
				local _, e, word = find(line, "^#%s*([%a_]+)", i)
				if e then
					push(out, i, e, C.macro)
					i = e + 1
					if word == "include" or word == "import" then
						local _, q = find(line, "^%s*<[^>]*>", i)
						if q then
							push(out, i, q, C.string)
							i = q + 1
						end
					end
					continue
				end
			end
			if b == AT and spec.decorators then
				local _, e = find(line, "^@[%a_][%w_%.]*", i)
				if e then
					push(out, i, e, C.attribute)
					i = e + 1
					continue
				end
			end
			if b == SLASH and spec.regex and not value then
				local e = regex_end(line, i)
				if e then
					push(out, i, e, C.regex)
					i = e + 1
					value = true
					continue
				end
			end
			if b == LT and spec.jsx and not value then
				local c = byte(line, i + 1)
				if is_alpha(c) or c == GT then
					local p, st = jsx(line, i, out, "tag", 0, 0)
					if not p then
						return nil, st, open
					end
					i = p
					value = true
					continue
				end
			end
			if stop and b == stop and open == 0 then
				return i, 0, 0
			end
			if b == opener then
				open += 1
			elseif stop and b == stop then
				open -= 1
			elseif not stop and (b == RBRACE or b == RPAREN) and open > 0 then
				open -= 1
			end
			if OPERATOR[b] then
				push(out, i, i, C.operator)
				value = false
			elseif PUNCT[b] then
				push(out, i, i, C.punctuation)
				value = b == RPAREN or b == RBRACK or b == RBRACE
			else
				value = false
			end
			naming = if b == LT or b == GT or b == STAR or b == 38 or b == DOT or b == COLON then naming else nil
			i += 1
		end
		return nil, 0, open
	end

	-- JSX: an element's tag and attributes, its children, and code in braces
	-- in either. `depth` counts the elements open around; reaching 0 again
	-- hands the line back to the code. States: "J<depth>" inside a tag,
	-- "X<depth>" in children, "j<depth>:<braces>" and "x<depth>:<braces>" in
	-- an attribute's or a child's braces.
	function jsx(line: string, j: number, out: { Span }, mode: string, depth: number, braces: number): (number?, any)
		local n = #line
		local closing = false
		while true do
			if mode == "tag" then
				-- `j` is at a `<`.
				closing = byte(line, j + 1) == SLASH
				local p = if closing then j + 1 else j
				push(out, j, p, C.punctuation)
				local s, e = find(line, "^[%w_%.%-:]+", p + 1)
				if s and e then
					local c = byte(line, s)
					push(out, s, e, if c >= 65 and c <= 90 then C.type else C.tag)
					p = e
				end
				j = p + 1
				mode = "attrs"
			elseif mode == "attrs" then
				while true do
					if j > n then
						return nil, "J" .. depth
					end
					local b = byte(line, j)
					if b == GT then
						push(out, j, j, C.punctuation)
						j += 1
						depth = if closing then depth - 1 else depth + 1
						mode = "children"
						break
					elseif b == SLASH and byte(line, j + 1) == GT then
						push(out, j, j + 1, C.punctuation)
						j += 2
						mode = "children"
						break
					elseif b == LBRACE then
						push(out, j, j, C.punctuation)
						local stop, st, open = scan(line, j + 1, out, RBRACE, braces)
						if not stop then
							return nil, if st == 0 then "j" .. depth .. ":" .. open else st
						end
						push(out, stop, stop, C.punctuation)
						j = stop + 1
						braces = 0
					elseif b == QUOTE or b == APOS then
						local q = find(line, string.char(b), j + 1, true) or n
						push(out, j, q, C.string)
						j = q + 1
					elseif b == EQ then
						push(out, j, j, C.operator)
						j += 1
					else
						local s, e = find(line, "^[%a_][%w_%-:]*", j)
						if s and e then
							push(out, s, e, C.attribute)
							j = e + 1
						else
							j += 1
						end
					end
				end
			elseif mode == "children" then
				if depth <= 0 then
					return j, 0
				end
				local p = find(line, "[<{]", j)
				if not p then
					return nil, "X" .. depth
				end
				if byte(line, p) == LBRACE then
					push(out, p, p, C.punctuation)
					local stop, st, open = scan(line, p + 1, out, RBRACE, braces)
					if not stop then
						return nil, if st == 0 then "x" .. depth .. ":" .. open else st
					end
					push(out, stop, stop, C.punctuation)
					j = stop + 1
					braces = 0
				else
					j = p
					mode = "tag"
				end
			end
		end
	end

	-- A line, picking up what the line before left open.
	return function(line: string, state: any): ({ Span }, any)
		local out: { Span } = {}
		local i = 1
		if state ~= 0 and type(state) == "string" then
			local tag = sub(state, 1, 1)
			local rest = sub(state, 2)
			local p, st
			if tag == "c" then
				p, st = nested(line, 1, out, tonumber(rest) or 1)
			elseif tag == "C" then
				p, st = raw(line, 1, out, rest, C.comment, "C")
			elseif tag == "R" then
				p, st = raw(line, 1, out, rest, C.string, "R")
			elseif tag == "s" or tag == "i" then
				local _, _, digits, letters = find(rest, "^(%d+)(%a*)")
				local base, flags = tonumber(digits) or 1, letters or ""
				if tag == "s" then
					p, st = str(line, 1, out, base, flags)
				else
					local stop, s2 = scan(line, 1, out, RBRACE)
					if stop then
						push(out, stop, stop, C.punctuation)
						p, st = str(line, stop + 1, out, base, flags)
					else
						p, st = nil, if s2 == 0 then state else s2
					end
				end
			elseif tag == "J" or tag == "X" then
				p, st = jsx(line, 1, out, if tag == "J" then "attrs" else "children", tonumber(rest) or 1, 0)
			elseif tag == "j" or tag == "x" then
				local _, _, d, b = find(rest, "^(%d+):(%d+)")
				local depth, braces = tonumber(d) or 1, tonumber(b) or 0
				local stop, s2, open = scan(line, 1, out, RBRACE, braces)
				if stop then
					push(out, stop, stop, C.punctuation)
					p, st = jsx(line, stop + 1, out, if tag == "j" then "attrs" else "children", depth, 0)
				else
					p, st = nil, if s2 == 0 then tag .. depth .. ":" .. open else s2
				end
			else
				p = 1
			end
			if not p then
				return out, st
			end
			i = p
		end
		local _, st = scan(line, i, out)
		return out, st
	end
end

return M
