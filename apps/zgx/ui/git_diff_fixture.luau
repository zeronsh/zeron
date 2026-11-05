-- The design system's diff pieces (ds/git) worked out on a small diff, for
-- tests/git_diff.test.luau: its colours (ds/syntax's highlighters), its
-- lines side by side, a hunk's header. What they came to is said as text
-- (`#hl`, `#pairs`), the header drawn as it is.

local ds = require("@ds")
ds.setup()
local git = require("@ds/git")

local DIFF = {
	"@@ -10,4 +10,5 @@ impl Lanes {",
	"     fn open(&mut self) -> usize {",
	"-        self.lanes.len()",
	"-        // gone",
	"+        let n = self.lanes.len();",
	"+        self.lanes.push(None);",
	"+        n",
	"     }",
}

local lines = git.parse(DIFF)
local hl = git.highlight(lines, "src/lanes.rs")
local said: { string } = {}
if hl then
	for _, r in (hl :: any)[2] do
		if r.text == "fn" then
			table.insert(said, `fn={r.color}`)
		end
	end
	table.insert(said, `hunk={if (hl :: any)[1] == nil then "none" else "some"}`)
	table.insert(said, `same={tostring(git.highlight(lines, "src/lanes.rs") == hl)}`)
end
table.insert(said, `unknown={tostring(git.highlight(git.parse(DIFF), "notes.unknownext") == nil)}`)

local paired: { string } = {}
for _, p in git.pairs(lines) do
	if p.hunk then
		table.insert(paired, "H")
	elseif p.left == p.right then
		table.insert(paired, "=")
	else
		table.insert(paired, `{if p.left then "L" else "_"}{if p.right then "R" else "_"}`)
	end
end

return ui.column {
	id = "root",
	w = 600,
	gap = 8,
	ui.text { table.concat(said, " "), id = "hl" },
	ui.text { table.concat(paired, " "), id = "pairs" },
	git.hunk_row(lines[1]),
	git.DiffLine { line = lines[2], runs = if hl then (hl :: any)[2] else nil },
}
