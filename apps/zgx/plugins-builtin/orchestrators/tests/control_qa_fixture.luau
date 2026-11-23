-- The Control room's page (screens/control/page) with its Q&A full: the
-- sample's world (control_world), plus two earlier dumps whose questions
-- went to threads. Its entries, newest asked first:
--
--   dp-2/rt-7   answered, not read yet          (the sample's)
--   dp-2/rt-9   waiting, its thread at work     (the sample's)
--   dp-qa2/rq-5 waiting
--   dp-qa/rq-1  answered at length, not read    (asked in a real dump)
--   dp-qa/rq-3  answered, read
--
-- and, not listed: dp-qa2/rq-6 (answered, put away), dp-qa/rq-2 (work in a
-- question's shape: a task) and dp-qa/rq-4 (rhetorical: info).

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
require("@app/screens/shell/route").launched:set(true)
require("./control_world")()
require("./control_qa_world")()
local page = require("@orchestrators/screens/control/page")

return ui.column {
	w_full = true,
	h_full = true,
	bg = "$background",
	page.Page(),
	ds.toast.Host {},
}
