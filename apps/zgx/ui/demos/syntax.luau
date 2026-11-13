-- The gallery's Syntax highlighting page: a sample of every language in an
-- editable, highlighted textarea (type in it: only the edited line and those
-- whose state it changes are coloured again), the same sample drawn
-- read-only from `Syntax.lines`, and a file of 10,000 lines to type in, with
-- what colouring all of it costs. The lexing checks run as it opens; what
-- they found shows at the top right. A page is { id, title, about, demo }:
-- `demo` builds what the page shows.

local Syntax = require("@ds/syntax")
local SelectionMenu = require("@ds/selection_menu")
local Dropdown = require("@ds/dropdown")
local Segmented = require("@ds/segmented")
local Text = require("@ds/text")
local samples = require("@demos/syntax_samples")
local tests = require("@demos/syntax_tests")

local language = ui.state("syntax_language", "rust" :: string?)
local view = ui.state("syntax_view", "edit")

-- The editors' height: room for a sample whole.
local EDITOR_H = 560
local STRESS_LINES = 10000

local items = {}
for _, l in Syntax.languages() do
	table.insert(items, { value = l.id, label = l.name })
end

-- A long Rust file: the sample over and over, each copy its own module, so
-- it has a real file's mix of comments, strings and code.
local function large(): string
	local parts = {}
	local lines = 0
	local copy = 0
	local per = select(2, string.gsub(samples.rust, "\n", ""))
	while lines < STRESS_LINES do
		copy += 1
		table.insert(parts, `mod copy_{copy} \{\n`)
		table.insert(parts, samples.rust)
		table.insert(parts, "}\n")
		lines += per + 2
	end
	return table.concat(parts)
end

-- What colouring every line of `text` costs, from the first line to the
-- last, carrying each line's state to the next (what opening the file
-- costs if every line were in view; the editor colours only those that
-- are).
local function measure(text: string, id: string): { total: number, per: number, lines: number }
	local h = Syntax.highlighter(id)
	local start = os.clock()
	local state: any = 0
	local n = 0
	if h then
		for line in string.gmatch(text, "([^\n]*)\n") do
			local _, st = h(line, state)
			state = st
			n += 1
		end
	end
	local total = (os.clock() - start) * 1000
	return { total = total, per = if n > 0 then total * 1000 / n else 0, lines = n }
end

local function editor(id: string, value: State<string>): Node
	return ui.textarea {
		label = `{Syntax.name(id)} sample`,
		value = value,
		highlight = Syntax.highlighter(id),
		line_numbers = true,
		font = "$font.mono",
		-- The textarea takes its type size as a number, not a token.
		text_size = ui.theme.get("zeron.prose.code.size") :: number,
		tab = "    ",
		current_line = "$hover",
		-- No inset on the scrolling sides: a textarea clips inside its padding.
		py = 0,
		pr = 0,
		line_height = "$zeron.editor.line",
		w_full = true,
		h = EDITOR_H,
		bg = "$surface.sunken",
		rounded = "$radius.lg",
	}
end

-- The sample drawn as text, line by line, from Syntax.lines: what Files
-- shows a file with, and markdown a code block.
local function reading(id: string): Node
	local rows = {}
	for _, line in Syntax.lines(samples[id] or "", id) do
		local rich: RichProps = {
			font = "$font.mono",
			text_size = "$zeron.prose.code.size",
			line_height = "$zeron.prose.code.line",
			literal = true,
		}
		for _, run in line do
			table.insert(rich, ui.span { run.text, color = run.color, literal = true })
		end
		if #line == 0 then
			table.insert(rich, " ")
		end
		table.insert(rows, ui.rich(rich))
	end
	return SelectionMenu {
		w_full = true,
		h = EDITOR_H,
		p = "$space.4",
		bg = "$surface.sunken",
		rounded = "$radius.lg",
		overflow = "hidden",
		rows,
	}
end

local stress_text = ui.state("syntax_stress", "")
-- What colouring the whole file cost, measured in a call of its own once
-- the page shows (a script call has 50 ms; building the page takes some).
local stress_cost = ui.state("syntax_stress_cost", "")

local function stress(): Node
	if stress_text:peek() == "" then
		stress_text:set(large())
	end
	stress_cost:set("Colouring every line…")
	ui.after(60, function()
		local m = measure(stress_text:peek(), "rust")
		stress_cost:set(
			string.format(
				"%s lines coloured top to bottom in %.1f ms, %.2f µs a line. The editor colours only the lines in view, and after a keystroke only the edited line and those its state reaches.",
				(string.reverse((string.gsub(string.reverse(tostring(m.lines)), "(%d%d%d)(%d)", "%1,%2")))),
				m.total,
				m.per
			)
		)
	end)
	return ui.column {
		gap = "$space.3",
		w_full = true,
		Text { stress_cost, style = "caption", tone = "secondary", wrap = true, w_full = true },
		editor("rust", stress_text),
	}
end

local function content(key: string): Node
	local id, mode = string.match(key, "^(.-)|(.*)$")
	id = id or "rust"
	if mode == "stress" then
		return stress()
	elseif mode == "read" then
		return reading(id :: string)
	end
	local value = ui.state(`syntax_text_{id}`, samples[id :: string] or "")
	return editor(id :: string, value)
end

-- The checks, run once as the page opens.
local result = tests.run()
local verdict = if #result.failed == 0
	then `{result.passed} lexing checks pass`
	else `{#result.failed} of {result.passed + #result.failed} lexing checks fail`

return {
	id = "syntax",
	title = "Syntax highlighting",
	about = "Code coloured as it's typed: every language Zeron shows, incremental line by line.",
	demo = function(): Node
		return ui.column {
			gap = "$space.4",
			w_full = true,
			ui.row {
				gap = "$space.3",
				w_full = true,
				align = "center",
				Dropdown {
					value = language,
					items = items,
					icon = "file-code",
					search = "Search languages…",
					label = "Language",
				},
				Segmented {
					value = view,
					label = "View",
					options = {
						{ value = "edit", label = "Edit" },
						{ value = "read", label = "Read-only" },
						{ value = "stress", label = "10,000 lines" },
					},
				},
				ui.div { grow = 1 },
				Text { verdict, style = "caption", tone = if #result.failed == 0 then "tertiary" else "danger" },
			},
			ui.show(
				#result.failed > 0,
				ui.column {
					gap = "$space.1",
					(function()
						local rows = {}
						for _, f in result.failed do
							table.insert(rows, Text { f, style = "mono", tone = "danger", literal = true })
						end
						return rows
					end)(),
				}
			),
			ui.presence(function()
				return `{language:get() or "rust"}|{view:get()}`
			end, content, { w_full = true }),
		}
	end,
}
