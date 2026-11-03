-- Mermaid fences in a reply streaming in, for tests/mermaid.test.luau. A
-- test's host has no `mermaid.render` (src/mermaid.rs): a stand-in draws
-- any source naming a diagram type as a picture from assets, and fails the
-- rest as the renderer would. `test_mermaid_calls` counts what it was
-- asked to draw; `test.append` adds the next piece of the reply.
-- (`zeron-zgx fixture=mermaid_fixture.luau --inspect` shows the same reply
-- drawn by the real host: the stand-in is only put in where there's none.)

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local Markdown = require("@ds/markdown")
local Lightbox = require("@ds/lightbox")
local Mermaid = require("@ds/mermaid")

local calls = ui.state("test_mermaid_calls", 0)
local real = Mermaid.render
local stubbed = ui.state("test_mermaid_stubbed", false)
ui.command {
	id = "test.stub",
	title = "Draw diagrams with a stand-in",
	run = function()
		stubbed:set(true)
	end,
}
Mermaid.render = function(arg: { [string]: any }): any
	calls:set(calls:peek() + 1)
	if not stubbed:peek() then
		return real(arg)
	end
	local source = arg.source :: string
	local ok = string.match(source, "^%s*flowchart") or string.match(source, "^%s*sequenceDiagram")
	local box = `<svg xmlns="http://www.w3.org/2000/svg" width="434" height="308"><rect x="8" y="8" width="418" height="292" rx="8" fill="#808080"/></svg>`
	local value = { svg = box, plate_svg = box, plate = "assets/composer/transcript.png", w = 434, h = 308 }
	return {
		after = function(_, done: (any) -> (), fail: ((any) -> ())?)
			ui.after(16, function()
				if ok then
					done(value)
				elseif fail then
					fail({ message = "Parse error on line 1: unknown diagram type" })
				end
			end)
		end,
	}
end

-- Whether fences are told as still being written (tests' last case).
local flow = "flowchart TD\n  A --> B"
local told = {}
for _, case in {
	{ "```mermaid\n" .. flow, flow },
	{ "```mermaid\n", "" },
	{ "```mermaid\n" .. flow .. "\n```\n", flow },
	{ "```mermaid\n" .. flow .. "\n```\n\nThen words.", flow },
	{ "~~~mermaid\n" .. flow .. "\n~~~", flow },
} do
	table.insert(told, tostring(Mermaid.writing(case[1], case[2])))
end
ui.state("test_mermaid_writing", ""):set(table.concat(told, " "))

-- The reply, a piece at a time.
local PIECES = {
	"Here's the flow:\n\n```mermaid\nflowchart TD\n  A[Start] --> B{Ready?}\n",
	"  B -->|yes| C[Ship it]\n  B -->|no| D[Fix]\n",
	"```\n\nAnd who talks to whom:\n\n```mermaid\nsequenceDiagram\n  participant U as User\n  participant Z as Zeron\n  U->>Z: Ask\n  Z-->>U: Answer\n```\n",
	"\nThis one isn't a diagram:\n\n```mermaid\nnot a diagram at all\n```\n",
}
local reply = ui.state("test_mermaid_reply", "")
local at = ui.state("test_mermaid_at", 0)
ui.command {
	id = "test.append",
	title = "Next piece of the reply",
	run = function()
		local n = at:peek() + 1
		if PIECES[n] then
			at:set(n)
			reply:append(PIECES[n])
		end
	end,
}
ui.command {
	id = "test.all",
	title = "The whole reply",
	run = function()
		at:set(#PIECES)
		reply:set(table.concat(PIECES))
	end,
}
ui.command { id = "theme_light", title = "Light theme", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-light")
end }
ui.command { id = "theme_dark", title = "Dark theme", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-dark")
end }

return ui.overlay {
	w_full = true,
	h_full = true,
	ui.scroll {
		w_full = true,
		h_full = true,
		bg = "$background",
		ui.column {
			id = "reply",
			selectable = true,
			w_full = true,
			max_w = 720,
			p = "$space.6",
			Markdown {
				text = function(): string
					return reply:get()
				end,
				key = "reply:mermaid",
			},
		},
	},
	Lightbox.Host(),
}
