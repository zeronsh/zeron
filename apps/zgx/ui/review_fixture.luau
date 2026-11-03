-- The pull request review example's Code tab at the window's width, for
-- tests/review_in_place.test.luau: the diff of the file chosen beside the
-- list, a comment opened (`ex_gr_composing`) and made under its lines.
-- Here, beside app.luau, as a mounted script loads modules from its own
-- directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local example = require("@demos/example_git_review")

return example.demo()
