-- The gallery's git journey, one page at a time, for
-- tests/git_pages.test.luau: the page named by `test_git_page` (the review,
-- to begin with). Here, beside app.luau, as a mounted script loads modules
-- from its own directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()

local PAGES: { [string]: string } = {
	git_commit = "@demos/example_git_commit",
	git_pr = "@demos/example_git_pr",
	git_review = "@demos/example_git_review",
	git_merge = "@demos/example_git_merge",
}
local which = ui.state("test_git_page", "git_review")

-- (Each its own page.)
return ui.presence(which, function(name: string): Node
	return (require :: any)(PAGES[name]).demo()
end, { w_full = true, h_full = true })
