-- A status icon following a kind ds/status has no entry for (nil while a
-- row goes, or a newer engine's) draws no glyph and raises nothing: it
-- once indexed KINDS[kind()] and failed with "attempt to index nil with
-- 'glyph'" (control_qa, a thread coming back to working).

test("Status unknown: a kind with no entry draws no glyph and no error", function()
	local app = mount("status_unknown_fixture.luau")
	app:frames(3)
	expect(app:error()).to_be_nil()
	-- (`false` stands for nil: a list holds no nil.)
	for _, odd in { "done", "no-such-kind", false, "failed", "working" } :: { any } do
		app:set_state("status_unknown_kind", if odd == false then nil else odd)
		app:frames(3)
		expect(app:error()).to_be_nil()
		expect(app:has("#status-unknown-row")).to_be(true)
	end
end)
