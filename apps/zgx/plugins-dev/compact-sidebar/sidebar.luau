-- The sidebar, as one short list: every session, newest first, its title
-- and where it stands. It keeps Zeron's own module's interface (pin,
-- archive, rename, the rail) by wrapping it, and draws only the full
-- sidebar itself. Rows are keyed by session, so a session's state or title
-- moving changes its row in place.

local original = require("@app-original/screens/shell/sidebar")
local ds = require("@app/ds")
local sessions = require("@app/account/sessions")
local route = require("@app/screens/shell/route")

local M: any = {}

-- The 40 most recent, as the engine has them.
local recent = ui.computed(function(): { any }
	local list = table.clone(sessions.now:get().chats or {})
	table.sort(list, function(a, b)
		return (a.updated or 0) > (b.updated or 0)
	end)
	local out = {}
	for i = 1, math.min(#list, 40) do
		table.insert(out, list[i])
	end
	return out
end)

local WORDS = { working = "Working", input = "Needs you", approval = "Needs you", failed = "Failed", done = "Done" }

local function row(chat: any, _i: number, now: State<any>): Node
	return ds.Surface {
		depth = "flat",
		label = chat.title,
		w_full = true,
		h = 30,
		border = 0,
		selected = function(): boolean
			return route.current:get().session == now:get().id
		end,
		on_click = function()
			route.open_session(now:peek().id)
		end,
		ui.row {
			w_full = true,
			h_full = true,
			px = "$space.2",
			gap = "$space.2",
			items = "center",
			ui.text {
				function(): string
					return now:get().title
				end,
				grow = 1,
				min_w = 0,
				truncate = true,
				text_size = "$font.size.md",
				color = "$text.primary",
			},
			ui.text {
				function(): string
					return WORDS[now:get().status or ""] or ""
				end,
				shrink = 0,
				text_size = "$font.size.sm",
				color = "$text.tertiary",
			},
		},
	}
end

function M.Sidebar(props: any): Node
	if props.island then
		return original.Sidebar(props)
	end
	return ui.column {
		id = "compact-sidebar",
		w = 240,
		shrink = 0,
		h_full = true,
		pt = ui.raw(props.top or 0, "the title bar's height, the shell's"),
		px = "$space.2",
		gap = "$zeron.list.gap",
		ui.show(function(): boolean
			return not props.collapsed:get()
		end, ui.scroll {
			w_full = true,
			grow = 1,
			ui.each(recent, row, {
				key = function(chat: any): string
					return chat.id
				end,
				w_full = true,
				gap = "$zeron.list.gap",
			}),
		}),
	}
end

return setmetatable(M, {
	__index = original,
	__call = function(_, props: any): Node
		return M.Sidebar(props)
	end,
})
