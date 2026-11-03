-- What runs wider than a reply's column (ds/wide), in one transcript: a
-- narrow table, a wide one that takes the page's room, a very wide one
-- that scrolls sideways, cells whose words wrap, code short and long
-- (both kept to the column, the long scrolling), a
-- table inside a list (kept to the column) and a visual whose widths ask
-- for more than the column. For tests/wide_content.test.luau and for
-- captures: `fixture_width` sets the page's width.
--
--   ./target/release/zeron-zgx fixture=wide_fixture.luau --inspect

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local T = require("@ds/transcript")

local width = ui.state("fixture_width", 1280)

local TEXT = table.concat({
	"Here's where each service stands. The prose keeps to the reading column; what's wider takes the page.",
	"",
	"| Service | Owner |",
	"|---|---|",
	"| edge | Ana |",
	"| engine | Bo |",
	"",
	"The rollout, with each service's status written out in full:",
	"",
	"| Service | Status | Next step |",
	"|---|---|---|",
	"| edge | Deployed to every region on Tuesday; the canary in eu-west held for six hours with no errors and the old fleet is drained | Remove the old fleet's DNS records once the TTL has run out |",
	"| engine | Waiting on the database migration, which locks the sessions table for about a minute on the largest tenants | Run the migration in the Sunday window |",
	"| sync | Healthy | None |",
	"",
	"Every region's numbers, which run past even the page:",
	"",
	"| Region | Version | Deployed | Requests | p50 | p95 | p99 | Errors | Saturation | CPU | Memory | Disk | Network in | Network out | Owner |",
	"|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|",
	"| us-east-1 | 2026.11.2-rc3 | Tuesday 14:02 UTC | 1,204,331 | 41 ms | 158 ms | 402 ms | 0.02% | 61% | 48% | 71% | 33% | 1.2 GB/s | 2.4 GB/s | Ana Lindqvist |",
	"| eu-west-1 | 2026.11.2-rc3 | Tuesday 15:40 UTC | 884,102 | 39 ms | 149 ms | 377 ms | 0.01% | 54% | 44% | 65% | 29% | 0.9 GB/s | 1.8 GB/s | Bo Okafor |",
	"",
	"A long line of code keeps to the column and scrolls:",
	"",
	"```rust",
	"fn main() {",
	'    let report = Report::builder().title("Weekly rollout status for every region and every service we run").owner("platform").include_history(true).build().expect("a report");',
	"    println!(\"{report}\");",
	"}",
	"```",
	"",
	"A short one, the same width:",
	"",
	"```sh",
	"cargo test -p engine",
	"```",
	"",
	"1. Inside a list, a table keeps to the column:",
	"",
	"   | Step | What happens when it runs, written out at some length so it would want more room |",
	"   |---|---|",
	"   | 1 | Drain the old fleet and wait for the connections to close before going on |",
}, "\n")

local VISUAL = [[
return function()
  return ui.row { gap = 16,
    ui.stat { width = 280, value = "1.2M", label = "requests" },
    ui.stat { width = 280, value = "158 ms", label = "p95 latency" },
    ui.stat { width = 280, value = "0.02%", label = "errors" },
    ui.stat { width = 280, value = "61%", label = "saturation" },
  }
end
]]

local messages = ui.state("wide_fixture_messages", {
	{ id = "u1", role = "user", text = "How's the rollout going?", at = "Nov 3, 9:12 AM" },
	{
		id = "a1",
		role = "assistant",
		text = TEXT,
		at = "Nov 3, 9:13 AM",
		model = "Opus",
		blocks = {
			{ id = "t1", kind = "text", text = TEXT },
			{ id = "v1", kind = "visual", title = "Rollout", source = VISUAL },
			{ id = "t2", kind = "text", text = "That's all of it." },
		},
	},
} :: { any })

local Page = ui.component(function(): Node
	return ui.column {
		w_full = true,
		h_full = true,
		bg = "$zeron.window",
		items = "center",
		ui.column {
			id = "page",
			h_full = true,
			w = function(): number
				return width:get()
			end,
			T.Transcript { messages = messages },
		},
	}
end)

return Page {}
