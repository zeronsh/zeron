-- The gallery's git journey, drawn from the design system's git pieces
-- (ds/git, ds/git_changes, which the GitHub plugin draws from too): each
-- page in each of its states, at the window's width and a side pane's,
-- mounts without an error and says what it should. On ui/git_pages_fixture.luau.

local function settle(app: any)
	app:frames(20)
end

local PAGES = {
	{ demo = "git_commit", state = "ex_gc_state", states = { "ready", "clean", "conflicts", "pushing", "push_failed" } },
	{ demo = "git_pr", state = "ex_gp_state", states = { "ready", "draft", "creating", "created", "failed" } },
	{ demo = "git_review", state = "ex_gr_state", states = { "running", "failed", "changes", "approved" } },
	{ demo = "git_merge", state = "ex_gm_state", states = { "ready", "blocked", "conflicts", "merging", "merged", "failed" } },
}

test("every git page mounts in every state at both widths", function()
	local app = mount("git_pages_fixture.luau")
	app:resize(1100, 820)
	for _, page in PAGES do
		app:set_state("test_git_page", page.demo)
		for _, width in { "full", "narrow" } do
			app:set_state("ex_git_width", width)
			for _, state in page.states do
				app:set_state(page.state, state)
				settle(app)
				expect(`{page.demo} {width} {state}: {app:error() or "ok"}`).to_be(`{page.demo} {width} {state}: ok`)
			end
		end
	end
end)

test("the review's three tabs show the header, the diff and the activity", function()
	local app = mount("git_pages_fixture.luau")
	app:resize(1100, 820)
	app:set_state("test_git_page", "git_review")
	app:set_state("ex_git_width", "full")
	app:set_state("ex_gr_state", "failed")
	app:set_state("ex_gr_tab", "summary")
	settle(app)
	expect(app:has('~"Add the design system\'s primitives"')).to_be(true)
	expect(app:has('role:button "Checks: 2 of 3 passed, Tests failed"')).to_be(true)
	app:set_state("ex_gr_tab", "code")
	settle(app)
	expect(app:error()).to_be_nil()
	expect(app:has('#diff-content >> ~"local ink = ink_of(props)"')).to_be(true)
	app:set_state("ex_gr_tab", "activity")
	settle(app)
	expect(app:has('~"pushed 3 commits to zeron/primitives"')).to_be(true)
	-- Narrow: the files open their diffs in place, in the virtualized list.
	app:set_state("ex_git_width", "narrow")
	app:set_state("ex_gr_tab", "code")
	settle(app)
	expect(app:error()).to_be_nil()
	expect(app:has('#changes-list >> role:button "ui/ds/badge.luau"')).to_be(true)
end)
