-- A reply's work (ds/activity) whose rows open to what each tool did
-- (ds/tool_detail), for tests/tool_detail.test.luau: an edit (its files'
-- counts, its whole diff in the sidecar), a command (a 30-line summary, its
-- whole output in the sidecar), a read, and an older doc's edit carrying
-- its diff. Fetches are stubbed (`test_tool_fetches`, one line each; the
-- output's first fetch fails while `test_tool_fail` is set). The work is
-- made again, as a reload makes it, when `test_tool_generation` moves.
-- Here, beside app.luau, as a mounted script loads modules from its own
-- directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local Activity = require("@ds/activity")
local ToolDetail = require("@ds/tool_detail")

local fetches = ui.state("test_tool_fetches", "")
local fail = ui.state("test_tool_fail", false)
local generation = ui.state("test_tool_generation", 1)

local function settled(value: any, err: any?): any
	return {
		after = function(_self: any, ok: (any) -> (), bad: (any) -> ())
			if err then
				bad(err)
			else
				ok(value)
			end
		end,
	}
end

local THIRTY = {}
for i = 1, 30 do
	table.insert(THIRTY, `line {i}`)
end

ToolDetail.fetch = function(blob_ref: string): any
	fetches:set(fetches:peek() .. blob_ref .. "\n")
	if blob_ref == "c/p1.diff" then
		return settled({
			kind = "diff",
			path = "src/app.rs",
			added = 1,
			removed = 1,
			lines = {
				{ kind = "hunk", text = "@@ -1,3 +1,3 @@" },
				{ kind = " ", old = 1, new = 1, text = "fn main() {" },
				{ kind = "-", old = 2, text = "    old_call();" },
				{ kind = "+", new = 2, text = "    new_call();" },
				{ kind = " ", old = 3, new = 3, text = "}" },
			},
		})
	end
	if fail:peek() then
		fail:set(false)
		return settled(nil, "offline")
	end
	return settled({ kind = "output", lines = THIRTY, more = 0 })
end

local TOOLS = {
	{ id = "p1", kind = "edit", verb = "Edited", target = "app.rs", added = 1, removed = 1, stats = { { path = "src/app.rs", added = 1, removed = 1 } }, diff_ref = "c/p1.diff" },
	{ id = "p2", kind = "exec", verb = "Ran", target = "cargo test", output = table.concat(THIRTY, "\n"), output_ref = "c/p2", output_bytes = 12345 },
	{ id = "p3", kind = "read", verb = "Read", target = "notes.md", output = "# Notes\nThe first line." },
	{
		id = "p4",
		kind = "edit",
		verb = "Edited",
		target = "old.txt",
		diff = { kind = "diff", path = "old.txt", lines = { { kind = "hunk", text = "@@ -1 +1 @@" }, { kind = "-", old = 1, text = "before" }, { kind = "+", new = 1, text = "after" } } },
	},
}

return ui.column {
	w_full = true,
	h_full = true,
	p = "$space.4",
	ui.presence(function(): string
		return tostring(generation:get())
	end, function(): Node
		return ui.scroll {
			w_full = true,
			h_full = true,
			Activity {
				key = "work-1",
				tools = function()
					return TOOLS
				end,
			},
		}
	end),
}
