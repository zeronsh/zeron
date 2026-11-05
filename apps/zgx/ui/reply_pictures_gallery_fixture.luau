-- The gallery's Reply pictures page on its own, for
-- tests/reply_pictures.test.luau's mount check.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local page = require("@demos/reply_pictures")

return ui.component(function(): Node
	return ui.scroll { w_full = true, h_full = true, page.demo() }
end) {}
