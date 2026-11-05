-- Checks of the tricky lexing (nested comments, escapes, raw strings,
-- interpolation over lines, heredocs, embedded languages, states that
-- settle), run by the Syntax highlighting page as it opens: it shows how
-- many pass and names any that fail. zgx has no Luau test runner, so the
-- page is the runner; the checks run in the app's own Luau.

local Syntax = require("@ds/syntax")

type Result = { passed: number, failed: { string } }

-- The colour (a token's last part: "keyword") of the first byte of `needle`
-- on line `at` of `lines`, highlighted in order; "text" where uncoloured.
local function color(language: string, lines: { string }, at: number, needle: string): string
	local h = Syntax.highlighter(language)
	assert(h, language)
	local state: any = 0
	for n, line in lines do
		local spans
		spans, state = h(line, state)
		if n == at then
			local pos = string.find(line, needle, 1, true)
			assert(pos, `"{needle}" not on line {at}`)
			for _, s in spans do
				if s[1] <= pos and pos <= s[2] then
					return (string.match(s[3], "([%w_]+)$") :: string)
				end
			end
			return "text"
		end
	end
	return "none"
end

-- What the last of `lines` leaves the next, as the lexer names it (the
-- textarea gets it as a number: `Syntax.state` names a number back).
local function leaves(language: string, lines: { string }): any
	local h = Syntax.highlighter(language)
	assert(h, language)
	local state: any = 0
	for _, line in lines do
		local _, st = h(line, state)
		state = st
	end
	assert(type(state) == "number", "a state is a number")
	return Syntax.state(state)
end

local CASES: { { any } } = {
	-- name, language, lines, line, needle, colour
	{ "rust: nested block comment", "rust", { "/* a /* b */ still", "comment */ fn x() {}" }, 1, "still", "comment" },
	{ "rust: nested comment closes", "rust", { "/* a /* b */ still", "comment */ fn x() {}" }, 2, "fn", "keyword" },
	{ "rust: raw string keeps quotes", "rust", { [[let s = r#"a "q" \n"#; let t = 1;]] }, 1, "q", "string" },
	{ "rust: no escapes in raw", "rust", { [[let s = r#"a "q" \n"#; let t = 1;]] }, 1, "\\n", "string" },
	{ "rust: code after raw", "rust", { [[let s = r#"a "q" \n"#; let t = 1;]] }, 1, "let t", "keyword" },
	{ "rust: raw over lines", "rust", { 'let s = r##"', 'one "# still', '"##; let x = 1;' }, 2, "still", "string" },
	{ "rust: raw closes", "rust", { 'let s = r##"', 'one "# still', '"##; let x = 1;' }, 3, "let", "keyword" },
	{ "rust: escape", "rust", { [[let s = "a\nb";]] }, 1, "\\n", "escape" },
	{ "rust: string over lines", "rust", { 'let s = "one', 'two";', "let y = 2;" }, 2, "two", "string" },
	{ "rust: lifetime", "rust", { "fn f<'a>(c: char) -> char { 'x' }" }, 1, "'a", "label" },
	{ "rust: char", "rust", { "fn f<'a>(c: char) -> char { 'x' }" }, 1, "'x'", "string" },
	{ "rust: format placeholder", "rust", { [[println!("{name:?} {}", x);]] }, 1, "{name", "escape" },
	{ "rust: macro", "rust", { [[println!("{name:?} {}", x);]] }, 1, "println", "macro" },
	{ "rust: attribute", "rust", { "#[derive(Debug)]" }, 1, "derive", "attribute" },
	{ "rust: fn name", "rust", { "pub fn send<T>(x: T) {}" }, 1, "send", "function" },
	{ "rust: type", "rust", { "let v: Vec<Peer> = Vec::new();" }, 1, "Peer", "type" },
	{ "rust: number suffix", "rust", { "let n = 0x1F_u8;" }, 1, "0x1F", "number" },
	{ "rust: range is not a float", "rust", { "for i in 0..10 {}" }, 1, "..", "punctuation" },
	{ "luau: long comment by level", "luau", { "--[==[ a ]] b", "]==] local x = 1" }, 1, "b", "comment" },
	{ "luau: long comment closes", "luau", { "--[==[ a ]] b", "]==] local x = 1" }, 2, "local", "keyword" },
	{ "luau: long string", "luau", { "local s = [[one", "two]] .. x" }, 2, "two", "string" },
	{ "luau: interpolation", "luau", { "print(`a {x + 1} b`)" }, 1, "1", "number" },
	{ "luau: method def", "luau", { "function Queue:push(item)" }, 1, "push", "function" },
	{ "luau: qualified def", "luau", { "function M.new()" }, 1, "M", "text" },
	{ "python: raw prefix", "python", { [[x = r"\d+" + "\n"]] }, 1, "\\d", "string" },
	{ "python: escape", "python", { [[x = r"\d+" + "\n"]] }, 1, "\\n", "escape" },
	{ "python: f-string code", "python", { 'f"{n + 1} {{lit}}"' }, 1, "1", "number" },
	{ "python: f-string braces", "python", { 'f"{n + 1} {{lit}}"' }, 1, "{{", "escape" },
	{ "python: triple over lines", "python", { 'doc = """one', "def not_code", '"""', "def f(): pass" }, 2, "def", "string" },
	{ "python: triple closes", "python", { 'doc = """one', "def not_code", '"""', "def f(): pass" }, 4, "def", "keyword" },
	{ "python: decorator", "python", { "@dataclass(frozen=True)" }, 1, "@dataclass", "attribute" },
	{ "ts: template over lines", "typescript", { "const a = `x ${", '  b ? "y" : 1', "} z`; let q = 2;" }, 2, '"y"', "string" },
	{ "ts: template resumes", "typescript", { "const a = `x ${", '  b ? "y" : 1', "} z`; let q = 2;" }, 3, "z", "string" },
	{ "ts: template closes", "typescript", { "const a = `x ${", '  b ? "y" : 1', "} z`; let q = 2;" }, 3, "let", "keyword" },
	{ "ts: unterminated string stays on its line", "typescript", { 'const s = "oops', "const t = 1;" }, 2, "const", "keyword" },
	{ "ts: division", "typescript", { "const r = a / b / c;" }, 1, "/", "operator" },
	{ "ts: regex", "typescript", { "const r = /ab+c/gi.test(s);" }, 1, "/ab", "regex" },
	{ "ts: property", "typescript", { "this.presence.lastSeen" }, 1, "lastSeen", "property" },
	{ "tsx: tag", "tsx", { "return <Button variant=\"x\" onClick={() => go(1)}>Hi</Button>;" }, 1, "Button", "type" },
	{ "tsx: attribute", "tsx", { "return <Button variant=\"x\" onClick={() => go(1)}>Hi</Button>;" }, 1, "onClick", "attribute" },
	{ "tsx: code in braces", "tsx", { "return <Button variant=\"x\" onClick={() => go(1)}>Hi</Button>;" }, 1, "go", "function" },
	{ "tsx: children are text", "tsx", { "return <p>if this</p>;" }, 1, "if", "text" },
	{ "tsx: tag over lines", "tsx", { "<Button", "  disabled={n > 1}", ">", "  Clicked if", "</Button>", "const x = 1;" }, 2, "disabled", "attribute" },
	{ "tsx: children over lines", "tsx", { "<Button", "  disabled={n > 1}", ">", "  Clicked if", "</Button>", "const x = 1;" }, 4, "if", "text" },
	{ "tsx: code after element", "tsx", { "<Button", "  disabled={n > 1}", ">", "  Clicked if", "</Button>", "const x = 1;" }, 6, "const", "keyword" },
	{ "tsx: braces over lines", "tsx", { "<a onClick={() => {", "  go();", "}}>x</a>", "let y;" }, 4, "let", "keyword" },
	{ "go: raw string over lines", "go", { "q := `SELECT", "FROM x`", "return nil" }, 2, "FROM", "string" },
	{ "go: raw string closes", "go", { "q := `SELECT", "FROM x`", "return nil" }, 3, "return", "keyword" },
	{ "swift: interpolation", "swift", { [[Text("\(name) online")]] }, 1, "name", "text" },
	{ "swift: around it", "swift", { [[Text("\(name) online")]] }, 1, "online", "string" },
	{ "kotlin: $name", "kotlin", { 'val s = "hi $name"' }, 1, "$name", "variable" },
	{ "cpp: raw string", "cpp", { [[auto s = R"x(a )" b)x"; int y;]] }, 1, "b", "string" },
	{ "cpp: after raw", "cpp", { [[auto s = R"x(a )" b)x"; int y;]] }, 1, "int", "type" },
	{ "c: include", "c", { "#include <stdio.h>" }, 1, "<stdio", "string" },
	{ "json: key", "json", { [[{ "name": "zeron", "ok": true }]] }, 1, '"name"', "property" },
	{ "json: value", "json", { [[{ "name": "zeron", "ok": true }]] }, 1, '"zeron"', "string" },
	{ "toml: header", "toml", { "[package]", 'name = "x"' }, 1, "package", "type" },
	{ "toml: key", "toml", { "[package]", 'name = "x"' }, 2, "name", "property" },
	{ "toml: date", "toml", { "at = 1979-05-27T07:32:00Z" }, 1, "1979", "number" },
	{ "sql: keywords any case", "sql", { "select Id FROM t WHERE x = 'it''s'" }, 1, "select", "keyword" },
	{ "sql: doubled quote", "sql", { "select Id FROM t WHERE x = 'it''s'" }, 1, "s'", "string" },
	{ "shell: heredoc body", "shell", { "cat <<EOF", "if then $x", "EOF", "echo hi" }, 2, "if", "string" },
	{ "shell: heredoc ends", "shell", { "cat <<EOF", "if then $x", "EOF", "echo hi" }, 4, "echo", "function" },
	{ "shell: <<- with tabs", "shell", { "cat <<-'END'", "\tbody", "\tEND", "ls" }, 4, "ls", "function" },
	{ "shell: variable in string", "shell", { 'echo "a $HOME b"' }, 1, "$HOME", "variable" },
	{ "shell: flag", "shell", { "cargo build --release" }, 1, "--release", "attribute" },
	{ "shell: string over lines", "shell", { "echo 'one", "two' && ls" }, 2, "ls", "function" },
	{ "css: property", "css", { ".a { color: red; }" }, 1, "color", "property" },
	{ "css: class", "css", { ".a { color: red; }" }, 1, ".a", "type" },
	{ "css: comment over lines", "css", { ".a { /* one", "two */ width: 1px; }" }, 2, "width", "property" },
	{ "css: unit", "css", { ".a { width: 12px; }" }, 1, "12px", "number" },
	{ "html: script is javascript", "html", { "<script>", "const a = `x", "y`;</script><p>" }, 3, "y", "string" },
	{ "html: after script", "html", { "<script>", "const a = `x", "y`;</script><p>" }, 3, "p", "tag" },
	{ "html: attribute", "html", { '<a href="/x" disabled>' }, 1, "href", "attribute" },
	{ "html: comment over lines", "html", { "<!-- one", "two --> <b>" }, 2, "b", "tag" },
	{ "markdown: fence is its language", "markdown", { "```rust", "/* open", "still */ fn", "```", "# Title" }, 3, "fn", "keyword" },
	{ "markdown: fence closes", "markdown", { "```rust", "/* open", "still */ fn", "```", "# Title" }, 5, "Title", "heading" },
	{ "markdown: inline code", "markdown", { "Run `cargo test` now" }, 1, "cargo", "string" },
	{ "markdown: snake_case isn't emphasis", "markdown", { "a snake_case_name here" }, 1, "case", "text" },
	{ "markdown: link", "markdown", { "See [docs](https://x.y)." }, 1, "docs", "link" },
	{ "yaml: block scalar", "yaml", { "run: |", "  if x", "next: 1" }, 2, "if", "string" },
	{ "yaml: block ends", "yaml", { "run: |", "  if x", "next: 1" }, 3, "next", "property" },
	{ "yaml: number", "yaml", { "run: |", "  if x", "next: 1" }, 3, "1", "number" },
	{ "yaml: alias", "yaml", { "  <<: *defaults" }, 1, "*defaults", "label" },
	{ "diff: inserted", "diff", { "+added", "-gone" }, 1, "added", "inserted" },
	{ "diff: deleted", "diff", { "+added", "-gone" }, 2, "gone", "deleted" },
}

-- States: what a line leaves, exactly, so the engine stops re-colouring.
local STATES: { { any } } = {
	{ "a plain line leaves 0", "rust", { "let x = 1;" }, 0 },
	{ "inside a nested comment", "rust", { "/* a /* b" }, "c2" },
	{ "a raw string's closer", "rust", { 'r##"' }, 'R"##' },
	{ "a Lua long comment's closer", "luau", { "--[=[" }, "C]=]" },
	{ "a heredoc's delimiter", "shell", { "cat <<EOF" }, "HEOF" },
	{ "markdown carries the fence's state", "markdown", { "```rust", "/* x" }, "F```|rust|c1" },
	{ "settled after a comment closes", "typescript", { "/* a", "b */" }, 0 },
}

local M = {}

function M.run(): Result
	local passed = 0
	local failed: { string } = {}
	local function check(name: string, ok: boolean, detail: string)
		if ok then
			passed += 1
		else
			table.insert(failed, `{name}: {detail}`)
		end
	end
	for _, c in CASES do
		local ok, got = pcall(color, c[2], c[3], c[4], c[5])
		check(c[1], ok and got == c[6], `wanted {c[6]}, got {tostring(got)}`)
	end
	for _, c in STATES do
		local ok, got = pcall(leaves, c[2], c[3])
		check(c[1], ok and got == c[4], `wanted {tostring(c[4])}, got {tostring(got)}`)
	end
	-- A path and its language (nil: none).
	local langs: { { path: string, language: string? } } = {
		{ path = "src/main.rs", language = "rust" },
		{ path = "App.tsx", language = "tsx" },
		{ path = "Dockerfile", language = "shell" },
		{ path = "/a/b/Cargo.lock", language = "toml" },
		{ path = "ui/ds/syntax.luau", language = "luau" },
		{ path = "notes.MD", language = "markdown" },
		{ path = "README", language = nil },
	}
	for _, l in langs do
		local got = Syntax.language(l.path)
		check(`language of {l.path}`, got == l.language, `wanted {tostring(l.language)}, got {tostring(got)}`)
	end
	-- Every sample's spans are well formed: numbers, in order, inside
	-- the line, never overlapping, each a token.
	local samples = require("@demos/syntax_samples")
	for _, l in Syntax.languages() do
		local h = Syntax.highlighter(l.id)
		local text = samples[l.id]
		if h and text then
			local state: any = 0
			local bad: string? = nil
			local n = 0
			for line in string.gmatch(text, "([^\n]*)\n") do
				n += 1
				local ok, spans, st = pcall(h, line, state)
				if not ok then
					bad = `line {n}: {spans}`
					break
				end
				state = st
				if type(st) ~= "number" then
					bad = `line {n} leaves a {type(st)}`
				end
				local last = 0
				for _, s in spans do
					if type(s[1]) ~= "number" or type(s[2]) ~= "number" or type(s[3]) ~= "string" then
						bad = `line {n}: span {tostring(s[1])}..{tostring(s[2])} {tostring(s[3])}`
					elseif s[1] <= last or s[2] < s[1] or s[2] > #line then
						bad = `line {n}: span {s[1]}..{s[2]} after {last} in {#line}`
					end
					last = s[2]
				end
				if bad then
					break
				end
			end
			check(`{l.id} sample spans`, bad == nil, bad or "")
		end
	end
	-- Lines give back the text, run for run.
	local text = "fn a() {}\n\n/* b */ let c = \"d\";"
	local joined = {}
	for _, line in Syntax.lines(text, "rust") do
		local parts = {}
		for _, r in line do
			table.insert(parts, r.text)
		end
		table.insert(joined, table.concat(parts))
	end
	check("lines give back the text", table.concat(joined, "\n") == text, table.concat(joined, "\\n"))
	return { passed = passed, failed = failed }
end

return M
