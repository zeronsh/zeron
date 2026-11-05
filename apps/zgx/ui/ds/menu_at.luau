-- A context menu at a point in the window, for what has no box of its own
-- to hang one from: a link in a paragraph (`ui.span`'s `context_menu`). The
-- menus' own (ds/context_menu), opened by the one host in the window.
--
--   MenuAt.Host {}   -- once, in the window's root
--   ui.span { …, context_menu = function(e)
--       return MenuAt.show(e, { { label = …, icon = …, on_select = … }, … })
--   end }
--
-- `show` opens the entries where `e` was asked (the pointer, or under a
-- link focused from the keys) and gives back nil, so the press goes on to
-- nothing else (a ContextMenu around it stands aside: the host took it).
-- In a VM with no host mounted (a plugin's tab before it mounts one), the
-- entries come back as zgx's own menu rows instead, so a link still has
-- its menu.

local ContextMenu = require("@self/context_menu")

local M = {}

export type Entry = ContextMenu.Entry

-- The hosts mounted, the latest last: the one that opens.
local hosts: { (x: number, y: number) -> boolean } = {}
-- What the next opening shows.
local pending: { Entry } = {}

-- zgx's own rows for `entries` (no host to open the menus' panel).
local function rows(entries: { Entry }): { Node }
	local out: { Node } = {}
	for _, e in entries do
		if e.label and not e.disabled then
			table.insert(out, ui.menu_item { label = e.label, on_click = e.on_select })
		end
	end
	return out
end

function M.show(e: PointerEvent, entries: { Entry }?): { Node }?
	if not entries or #entries == 0 then
		return nil
	end
	local host = hosts[#hosts]
	if host then
		pending = entries
		host(e.x, e.y)
		return nil
	end
	return rows(entries)
end

-- The host: nothing to see or reach (no size, no pointer) until it opens.
function M.Host(_props: {}?): Node
	local function opener(open_at: (x: number, y: number) -> boolean)
		table.insert(hosts, open_at)
		ui.on_cleanup(function()
			local i = table.find(hosts, open_at)
			if i then
				table.remove(hosts, i)
			end
		end)
	end
	return ContextMenu {
		items = function(): { Entry }?
			return pending
		end,
		hook = opener,
		absolute = true,
		left = 0,
		top = 0,
		w = 0,
		h = 0,
		pointer_events = "none",
	}
end

return M
