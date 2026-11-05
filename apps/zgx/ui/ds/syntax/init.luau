-- Syntax colours for code: which language a file is, a line highlighter for
-- a textarea's `highlight` (zgx lays a highlighted text out by its lines and
-- re-colours from an edited line only until the state a line leaves settles),
-- and a whole text as coloured runs for drawing outside a textarea.
--
--   local Syntax = require("@ds/syntax")
--   Syntax.language("src/main.rs")          --> "rust"
--   ui.textarea { highlight = Syntax.highlighter("rust"), ... }
--   Syntax.lines(source, "rust")            --> { { { text, color }, ... }, ... }
--
-- Each language is a small lexer: the C-like ones a table of rules for one
-- shared lexer (code.luau over lex.luau), the rest their own (markup.luau).
-- Colours are the theme's `zeron.syntax.*` tokens.

local lex = require("@self/lex")
local code = require("@self/code")
local markup = require("@self/markup")

export type Span = lex.Span
export type Highlighter = lex.Highlighter
export type Run = { text: string, color: string }
type Comment = { line: string?, block: { string }? }
type Language = { id: string, name: string }

local M = {}

local highlighters: { [string]: Highlighter } = {}
for id, h in code do
	highlighters[id] = h
end
highlighters.shell = markup.shell
highlighters.css = markup.css
highlighters.yaml = markup.yaml
highlighters.diff = markup.diff
highlighters.html = markup.html(code.javascript)

-- Names a language goes by (a fence's tag, a file's extension) beyond its id.
local ALIASES: { [string]: string } = {
	rs = "rust",
	ts = "typescript",
	mts = "typescript",
	cts = "typescript",
	js = "javascript",
	mjs = "javascript",
	cjs = "javascript",
	node = "javascript",
	lua = "lua",
	py = "python",
	pyi = "python",
	pyw = "python",
	python3 = "python",
	golang = "go",
	kt = "kotlin",
	kts = "kotlin",
	cs = "csharp",
	["c#"] = "csharp",
	h = "c",
	objc = "c",
	m = "c",
	cc = "cpp",
	cxx = "cpp",
	hpp = "cpp",
	hh = "cpp",
	hxx = "cpp",
	["c++"] = "cpp",
	mm = "cpp",
	ino = "cpp",
	rb = "ruby",
	jsonc = "json",
	json5 = "json",
	jsonl = "json",
	geojson = "json",
	ipynb = "json",
	webmanifest = "json",
	yml = "yaml",
	md = "markdown",
	mdx = "markdown",
	markdown = "markdown",
	sh = "shell",
	bash = "shell",
	zsh = "shell",
	ksh = "shell",
	fish = "shell",
	console = "shell",
	shellsession = "shell",
	env = "shell",
	dockerfile = "shell",
	docker = "shell",
	make = "shell",
	makefile = "shell",
	htm = "html",
	xhtml = "html",
	xml = "html",
	svg = "html",
	plist = "html",
	vue = "html",
	svelte = "html",
	scss = "css",
	sass = "css",
	less = "css",
	patch = "diff",
	frag = "wgsl",
}

-- Whole file names with a language of their own.
local NAMES: { [string]: string } = {
	dockerfile = "shell",
	containerfile = "shell",
	makefile = "shell",
	gnumakefile = "shell",
	justfile = "shell",
	[".bashrc"] = "shell",
	[".zshrc"] = "shell",
	[".profile"] = "shell",
	[".bash_profile"] = "shell",
	[".zprofile"] = "shell",
	[".env"] = "shell",
	[".envrc"] = "shell",
	["cargo.lock"] = "toml",
	["pipfile"] = "toml",
	[".luaurc"] = "json",
	[".prettierrc"] = "json",
	[".eslintrc"] = "json",
	["tsconfig.json"] = "json",
}

local NAMES_SHOWN: { [string]: string } = {
	rust = "Rust",
	typescript = "TypeScript",
	tsx = "TSX",
	javascript = "JavaScript",
	jsx = "JSX",
	luau = "Luau",
	lua = "Lua",
	python = "Python",
	go = "Go",
	swift = "Swift",
	kotlin = "Kotlin",
	java = "Java",
	csharp = "C#",
	c = "C",
	cpp = "C++",
	ruby = "Ruby",
	json = "JSON",
	toml = "TOML",
	yaml = "YAML",
	markdown = "Markdown",
	shell = "Shell",
	html = "HTML",
	css = "CSS",
	sql = "SQL",
	wgsl = "WGSL",
	diff = "Diff",
}

local COMMENTS: { [string]: Comment } = {}
do
	local slashes: Comment = { line = "//", block = { "/*", "*/" } }
	for _, id in { "rust", "typescript", "tsx", "javascript", "jsx", "go", "swift", "kotlin", "java", "csharp", "c", "cpp", "wgsl" } do
		COMMENTS[id] = slashes
	end
	local hash: Comment = { line = "#" }
	for _, id in { "python", "ruby", "shell", "toml", "yaml" } do
		COMMENTS[id] = hash
	end
	COMMENTS.luau = { line = "--", block = { "--[[", "]]" } }
	COMMENTS.lua = COMMENTS.luau
	COMMENTS.sql = { line = "--", block = { "/*", "*/" } }
	COMMENTS.css = { block = { "/*", "*/" } }
	COMMENTS.html = { block = { "<!--", "-->" } }
	COMMENTS.markdown = COMMENTS.html
	COMMENTS.json = { line = "//", block = { "/*", "*/" } }
end

-- A language id from an id or any name it goes by, or nil.
local function canonical(name: string?): string?
	if not name or name == "" then
		return nil
	end
	local l = string.lower(name)
	if highlighters[l] or l == "markdown" then
		return l
	end
	return ALIASES[l]
end

-- Markdown's fenced code takes any language, markdown too.
local md = markup.markdown(function(lang: string): Highlighter?
	local id = canonical(lang)
	return if id then highlighters[id] else nil
end)
highlighters.markdown = md

-- The language for a path or file name ("rust", "luau", "typescript", ...),
-- or nil.
function M.language(path: string): string?
	local name = string.lower(string.match(path, "([^/\\]+)$") or path)
	local named = NAMES[name]
	if named then
		return named
	end
	local ext = string.match(name, "%.([^%.]+)$")
	if not ext then
		return nil
	end
	return canonical(ext)
end

-- What a line leaves, as the textarea holds it: a number (it takes no
-- other). Each state string the lexers name gets a number of its own the
-- first time it is seen, the same one ever after, so two lines leaving the
-- same thing still compare equal and the editor's re-colouring settles.
-- There are few: a nesting depth, a raw string's closer, a heredoc's
-- delimiter, a fence's language.
local numbers: { [string]: number } = {}
local named: { string } = {}

local function number_of(state: any): number
	if state == 0 or state == nil then
		return 0
	end
	local n = numbers[state]
	if not n then
		table.insert(named, state)
		n = #named
		numbers[state] = n
	end
	return n
end

local function state_of(n: any): any
	if type(n) == "number" then
		return if n == 0 then 0 else named[n] or 0
	end
	return n or 0
end

-- The textarea's highlighters, by language: each lexer with its states as
-- numbers, made once, so a language's highlighter is the same function
-- every time it is asked for.
local public: { [string]: Highlighter } = {}
for id, h in highlighters do
	public[id] = function(line: string, state: any): ({ Span }, any)
		local spans, st = h(line, state_of(state))
		return spans, number_of(st)
	end
end

-- zgx's textarea line highlighter for a language (an id or an alias, as a
-- fence's tag), or nil for an unknown one. The same function each time.
-- What it leaves a line is 0 or a number (see above).
function M.highlighter(language: string?): Highlighter?
	local id = canonical(language)
	return if id then public[id] else nil
end

-- What a state number stands for, as the lexer names it ("c2": a block
-- comment two deep): for tests and debugging.
function M.state(n: number): any
	return state_of(n)
end

-- A language's name as shown ("TypeScript", "C++"), "Plain Text" without one.
function M.name(language: string?): string
	local id = canonical(language)
	return if id then NAMES_SHOWN[id] or id else "Plain Text"
end

-- How a language comments out a line or a block, for toggling comments, or nil.
function M.comment(language: string?): Comment?
	local id = canonical(language)
	return if id then COMMENTS[id] else nil
end

-- Every language, by id, with its name: for pickers.
function M.languages(): { Language }
	local out: { Language } = {}
	for id, name in NAMES_SHOWN do
		table.insert(out, { id = id, name = name })
	end
	table.sort(out, function(a: Language, b: Language): boolean
		return a.name < b.name
	end)
	return out
end

local TEXT = lex.C.text

-- One line as runs: its spans' colours, the text's own between them.
local function runs(line: string, spans: { Span }): { Run }
	local out: { Run } = {}
	local at = 1
	for _, s in spans do
		local from, to = math.max(s[1], at), math.min(s[2], #line)
		if from > at then
			table.insert(out, { text = string.sub(line, at, from - 1), color = TEXT })
		end
		if to >= from then
			table.insert(out, { text = string.sub(line, from, to), color = s[3] })
			at = to + 1
		end
	end
	if at <= #line then
		table.insert(out, { text = string.sub(line, at), color = TEXT })
	end
	return out
end

-- A whole text as styled runs for read-only drawing (ui.rich spans): a list
-- of lines, each a list of { text, color }. An empty line is an empty list.
function M.lines(text: string, language: string?): { { Run } }
	local id = canonical(language)
	local h = if id then highlighters[id] else nil
	local out: { { Run } } = {}
	local state: any = 0
	for line in string.gmatch(if string.sub(text, -1) == "\n" then text else text .. "\n", "(.-)\r?\n") do
		if h then
			local spans
			spans, state = h(line, state)
			table.insert(out, runs(line, spans))
		elseif line == "" then
			table.insert(out, {} :: { Run })
		else
			table.insert(out, { { text = line, color = TEXT } })
		end
	end
	return out
end

return M
