-- A diff as the design system reads it (ds/git), on ui/git_diff_fixture.luau:
-- its lines coloured by their language with the editor's own highlighters
-- (ds/syntax), worked out once per file; side by side, what was taken out
-- paired with what was put in; a hunk's header with the code it falls in.
-- The list that draws them (ds/git_changes: wrapped, split, sticky file
-- rows) is tested with the Git plugin (src/plugins/examples_tests.rs).

test("a diff's lines are coloured as their language is, once per file", function()
	local app = mount("git_diff_fixture.luau")
	app:frames(5)
	local said = app:text("#hl")
	-- `fn`, a keyword: a colour of its own, not plain text's.
	expect(string.find(said, "fn=$zeron.syntax.", 1, true) ~= nil).to_be(true)
	expect(string.find(said, "fn=$zeron.syntax.text", 1, true)).to_be(nil)
	-- A hunk's header has none; asked again, the same answer, kept.
	expect(string.find(said, "hunk=none", 1, true) ~= nil).to_be(true)
	expect(string.find(said, "same=true", 1, true) ~= nil).to_be(true)
	-- A language Zeron doesn't colour: none at all.
	expect(string.find(said, "unknown=true", 1, true) ~= nil).to_be(true)
end)

test("side by side pairs what was taken out with what was put in", function()
	local app = mount("git_diff_fixture.luau")
	app:frames(5)
	-- Header; unchanged; two out beside two in; the third in alone; unchanged.
	expect(app:text("#pairs")).to_be("H = LR LR _R =")
end)

test("a hunk's header says the code it falls in apart from its ranges", function()
	local app = mount("git_diff_fixture.luau")
	app:frames(5)
	expect(app:has('"@@ -10,4 +10,5 @@"')).to_be(true)
	expect(app:has('"impl Lanes {"')).to_be(true)
end)
