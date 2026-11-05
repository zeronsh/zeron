-- The gallery's Context menu page: right-click (or Control-click, or focus
-- one and press Shift+F10 or the Menu key) a session, the empty list around
-- them, a message, a file; and, at the window's corner, one whose menu
-- flips to stay on screen. What a choice did shows under them. A page is
-- { id, title, about, demo, foot }.

local ds = require("@ds")
local ContextMenu = require("@ds/context_menu")

type Entry = ContextMenu.Entry
-- An entry onto a menu (typed as one, so its icon is checked as an icon).
local function add(menu: { Entry }, entry: Entry)
	table.insert(menu, entry)
end

-- Each menu's open state, by name (`menu:s1`, `menu:area`), so `zgx inspect
-- set` can open one without a pointer.
local opens: { [string]: State<boolean> } = {}
local function opened(id: string): State<boolean>
	if not opens[id] then
		opens[id] = ui.state(`menu:{id}`, false)
	end
	return opens[id]
end
-- What the last choice did.
local did = ui.state("context_menu_did", "")

type Session = { id: string, title: string, project: string, time: string, status: ds.StatusKind? }
local SESSIONS: { Session } = {
	{ id = "s1", title = "Port the context menu to zgx", project = "comet-native", time = "now", status = "working" },
	{ id = "s2", title = "Fix the flaky local_first shutdown test", project = "comet-native", time = "2h" },
	{ id = "s3", title = "Review the Tailscale MTU wrapper", project = "zgx", time = "1d", status = "input" },
	{ id = "s4", title = "Draft release notes for v0.2.129", project = "landing", time = "3d" },
}
local PROJECTS = { "comet-native", "zgx", "zui", "landing" }
local pinned = ui.state("context_menu_pinned", { s2 = true } :: { [string]: boolean })
local moved = ui.state("context_menu_moved", {} :: { [string]: string })
local selected = ui.state("context_menu_selected", "s1")
local sort = ui.state("context_menu_sort", "updated")
local show_branch = ui.state("context_menu_branch", true)
local show_pr = ui.state("context_menu_pr", false)

local function said(text: string): () -> ()
	return function()
		did:set(text)
	end
end

-- A session's menu, as Zeron's sidebar has it (shell.rs's chat menu), and
-- more: what it would copy, where it would move it, and the keys for each.
local function session_menu(s: Session): { Entry }
	local is_pinned = pinned:peek()[s.id] == true
	local here = moved:peek()[s.id] or s.project
	local move_to: { Entry } = { { heading = "Projects" } }
	for _, p in PROJECTS do
		add(move_to, {
			label = p,
			icon = "folder",
			checked = function(): boolean
				return (moved:get()[s.id] or s.project) == p
			end,
			-- Where it is already: nowhere to move it.
			disabled = p == here,
			on_select = function()
				local m = table.clone(moved:peek())
				m[s.id] = p
				moved:set(m)
				did:set(`Moved “{s.title}” to {p}`)
			end,
		})
	end
	add(move_to, { separator = true })
	add(move_to, { label = "New section…", icon = "plus", on_select = said("Asked for a new section") })
	return {
		{ label = "Open in side chat", icon = "chat-round-line", shortcut = "cmd+enter", on_select = said(`Opened “{s.title}” beside`) },
		{ label = "Rename", icon = "pen", shortcut = "enter", on_select = said(`Renaming “{s.title}”`) },
		{
			label = if is_pinned then "Unpin" else "Pin",
			icon = "pin",
			shortcut = "cmd+shift+p",
			on_select = function()
				local p = table.clone(pinned:peek())
				p[s.id] = if is_pinned then nil else true
				pinned:set(p)
				did:set(if is_pinned then `Unpinned “{s.title}”` else `Pinned “{s.title}”`)
			end,
		},
		{
			label = "Copy",
			icon = "copy",
			submenu = {
				{ label = "Path", icon = "folder", on_select = said("Copied the path") },
				{ label = "Session link", icon = "global", shortcut = "cmd+shift+c", on_select = said("Copied the session link") },
				{ label = "Session ID", icon = "document", on_select = said("Copied the session ID") },
			},
		},
		{ label = "Move to", icon = "folder-with-files", submenu = move_to },
		{ separator = true },
		{
			label = "Archive",
			icon = "archive-minimalistic",
			shortcut = "cmd+backspace",
			disabled = if s.status == "working" then "Stop it first: its agent is still working" else nil,
			on_select = said(`Archived “{s.title}”`),
		},
		{
			label = "Delete",
			icon = "trash-bin-minimalistic",
			danger = true,
			confirm = "Click again to delete",
			on_select = said(`Deleted “{s.title}”`),
		},
	}
end

-- The empty list's: what makes a session, and how the list is shown
-- (toggles that leave it open, to change several at once).
local function area_menu(): { Entry }
	return {
		{ label = "New session", icon = "pen-new-square", shortcut = "cmd+n", on_select = said("Started a new session") },
		{ label = "Paste", icon = "copy", shortcut = "cmd+v", disabled = "Nothing copied to paste" },
		{ separator = true },
		{ heading = "Sort by" },
		{
			label = "Last updated",
			icon = "clock-circle",
			keep_open = true,
			checked = function(): boolean
				return sort:get() == "updated"
			end,
			on_select = function()
				sort:set("updated")
				did:set("Sorted by last updated")
			end,
		},
		{
			label = "Created",
			icon = "calendar",
			keep_open = true,
			checked = function(): boolean
				return sort:get() == "created"
			end,
			on_select = function()
				sort:set("created")
				did:set("Sorted by created")
			end,
		},
		{ heading = "Show" },
		{
			label = "Branch",
			icon = "git-branch",
			keep_open = true,
			checked = function(): boolean
				return show_branch:get()
			end,
			on_select = function()
				show_branch:set(not show_branch:peek())
			end,
		},
		{
			label = "Pull request",
			icon = "pull-request",
			keep_open = true,
			checked = function(): boolean
				return show_pr:get()
			end,
			on_select = function()
				show_pr:set(not show_pr:peek())
			end,
		},
	}
end

type Message = { id: string, who: "user" | "agent", text: string, streaming: boolean? }
local MESSAGES: { Message } = {
	{ id = "m1", who = "user", text = "Can the context menu keep its submenu open while I move to it diagonally?" },
	{
		id = "m2",
		who = "agent",
		streaming = true,
		text = "Yes: while the pointer heads for the submenu, the rows it crosses wait before they take over…",
	},
}
local function message_menu(m: Message): { Entry }
	local entries: { Entry } = {
		{ label = "Copy", icon = "copy", shortcut = "cmd+c", on_select = said("Copied the message") },
		{ label = "Copy as Markdown", icon = "document", on_select = said("Copied the message as Markdown") },
		{ label = "Quote in reply", icon = "chat-round-line", on_select = said("Quoted it in the reply") },
	}
	if m.who == "user" then
		add(entries, { label = "Edit", icon = "pen", on_select = said("Editing the message") })
	else
		add(entries, {
			label = "Fork from here",
			icon = "git-branch",
			disabled = if m.streaming then "Once the reply has finished" else nil,
			on_select = said("Forked a new session from here"),
		})
	end
	add(entries, { separator = true })
	add(entries, {
		label = "Delete message",
		icon = "trash-bin-minimalistic",
		danger = true,
		confirm = "Click again to delete",
		on_select = said("Deleted the message"),
	})
	return entries
end

type File = { path: string, change: string? }
local FILES: { File } = {
	{ path = "ui/ds/context_menu.luau", change = "A" },
	{ path = "ui/ds/tokens.luau", change = "M" },
	{ path = "ui/demos/context_menu.luau", change = "M" },
	{ path = "README.md" },
}
local function file_menu(f: File): { Entry }
	local name = string.match(f.path, "[^/]+$") or f.path
	return {
		{ label = "Open", icon = "document", shortcut = "enter", on_select = said(`Opened {name}`) },
		{
			label = "Open with",
			icon = "arrow-up-right",
			submenu = {
				{ label = "Zed", icon = "pen", on_select = said(`Opened {name} in Zed`) },
				{ label = "Visual Studio Code", icon = "pen", on_select = said(`Opened {name} in Visual Studio Code`) },
				{ label = "Xcode", icon = "pen", on_select = said(`Opened {name} in Xcode`) },
				{ separator = true },
				{
					label = "Other",
					icon = "more-horizontal",
					submenu = {
						{ label = "TextEdit", icon = "document", on_select = said(`Opened {name} in TextEdit`) },
						{ label = "Terminal", icon = "terminal", on_select = said(`Opened {name} in Terminal`) },
					},
				},
			},
		},
		{ label = "Reveal in Finder", icon = "folder", shortcut = "cmd+shift+r", on_select = said(`Revealed {name} in Finder`) },
		{ heading = "Copy" },
		{ label = "Path", icon = "copy", shortcut = "cmd+alt+c", on_select = said("Copied its path") },
		{ label = "Relative path", icon = "copy", shortcut = "cmd+shift+alt+c", on_select = said("Copied its relative path") },
		{ separator = true },
		{ heading = "Changes" },
		{
			label = "Stage",
			icon = "check",
			disabled = if f.change then nil else "No changes to stage",
			on_select = said(`Staged {name}`),
		},
		{
			label = "Discard changes",
			icon = "refresh",
			danger = true,
			disabled = if f.change then nil else "No changes to discard",
			confirm = "Click again to discard",
			on_select = said(`Discarded the changes to {name}`),
		},
	}
end

-- A session row's fill: a wash under the pointer and when pressed; selected,
-- the selected fill throughout (as Surface's flat selection: a wash under it
-- would dim it).
local SESSION_ROW = ui.variants {
	base = { bg = "#00000000", hover = { bg = "$hover" }, active = { bg = "$pressed" } },
	variants = { selected = { [true] = {}, [false] = {} } },
	compound = {
		{
			selected = true,
			bg = "$zeron.selected.flat.bg",
			hover = { bg = "$zeron.selected.flat.bg" },
			active = { bg = "$zeron.selected.flat.bg" },
		},
	},
	defaults = { selected = false },
}

-- A session's row, as the sidebar's (its harness, its title, how long ago),
-- ringed while a menu opened from the keys is open.
local function session_row(s: Session): Node
	return ContextMenu {
		label = "Session actions",
		open = opened(s.id),
		ring = "$radius.md",
		w_full = true,
		items = function()
			return session_menu(s)
		end,
		ui.button {
			label = s.title,
			w_full = true,
			h = 32,
			px = "$space.2",
			gap = "$space.2",
			items = "center",
			rounded = "$radius.md",
			variants = SESSION_ROW {
				selected = function(): boolean
					return selected:get() == s.id
				end,
			},
			on_click = function()
				selected:set(s.id)
			end,
			ds.Mark { name = "claude", size = 14, bare = true },
			ds.FadeText { s.title, grow = 1, text_size = "$font.size.md", color = "$text.primary" },
			ui.show(function(): boolean
				return pinned:get()[s.id] == true
			end, ui.div { color = "$text.tertiary", ds.Icon { name = "pin", size = 12 } }),
			if s.status then ds.StatusDot { kind = s.status, size = 8 } else nil,
			ui.text { s.time, shrink = 0, text_size = "$font.size.sm", color = "$text.tertiary" },
		},
	}
end

-- A message, as the transcript draws one: the user's in a bubble at the
-- end, the agent's on the page.
local function message(m: Message): Node
	local user = m.who == "user"
	return ui.row {
		w_full = true,
		justify = if user then "end" else "start",
		ContextMenu {
			label = "Message actions",
			open = opened(m.id),
			ring = "$radius.lg",
			items = function()
				return message_menu(m)
			end,
			ui.div {
				focusable = true,
				label = m.text,
				max_w = if user then 360 else nil,
				px = if user then "$space.3" else "$space.1",
				py = "$space.2",
				rounded = "$radius.lg",
				bg = if user then "$zeron.secondary.bg" else "#00000000",
				ds.Text { m.text, wrap = true, tone = if m.streaming then "secondary" else "primary" },
			},
		},
	}
end

local function file_row(f: File): Node
	local name = string.match(f.path, "[^/]+$") or f.path
	local dir = string.sub(f.path, 1, #f.path - #name)
	return ContextMenu {
		label = "File actions",
		open = opened(f.path),
		ring = "$radius.sm",
		w_full = true,
		items = function()
			return file_menu(f)
		end,
		ui.button {
			label = f.path,
			w_full = true,
			h = 28,
			px = "$space.2",
			gap = "$space.2",
			items = "center",
			rounded = "$radius.sm",
			bg = "#00000000",
			hover = { bg = "$hover" },
			active = { bg = "$pressed" },
			ui.div { color = "$text.tertiary", ds.Icon { name = "document", size = 14 } },
			ui.text { name, shrink = 0, text_size = "$font.size.md", color = "$text.primary" },
			ds.FadeText { dir, grow = 1, text_size = "$font.size.sm", color = "$text.tertiary" },
			ui.text {
				f.change or "",
				shrink = 0,
				font = "$font.mono",
				text_size = "$font.size.xs",
				color = if f.change == "A" then "$success" else "$warning",
			},
		},
	}
end

-- (A specimen opens nothing.)
local function none(): { Entry }?
	return nil
end

local function block(title: string, ...: Child): Node
	return ui.column {
		gap = "$space.3",
		ds.SectionTitle { title },
		...,
	}
end

return {
	id = "context-menu",
	title = "Context menu",
	about = "Right-click anything: a session, the list around them, a message, a file. Or focus one and press Shift+F10.",
	demo = function(): Node
		local sessions: { Node } = {}
		for _, s in SESSIONS do
			table.insert(sessions, session_row(s))
		end
		local messages: { Node } = {}
		for _, m in MESSAGES do
			table.insert(messages, message(m))
		end
		local files: { Node } = {}
		for _, f in FILES do
			table.insert(files, file_row(f))
		end
		return ui.column {
			gap = "$space.6",
			ui.row {
				gap = "$space.6",
				items = "start",
				flex_wrap = true,
				block(
					"SESSIONS",
					-- The list and the room under it: the empty list's own menu.
					ContextMenu {
						label = "Session list",
						open = opened("area"),
						ring = "$radius.lg",
						w = 320,
						items = function()
							return area_menu()
						end,
						ui.column {
							w_full = true,
							h = 220,
							p = "$space.1",
							gap = "$zeron.list.gap",
							rounded = "$radius.lg",
							bg = "$zeron.secondary.bg",
							table.unpack(sessions),
						},
					}
				),
				block(
					"FILES",
					ui.column { w = 320, gap = "$zeron.list.gap", table.unpack(files) }
				),
			},
			block("MESSAGES", ui.column { w = 664, gap = "$space.3", table.unpack(messages) }),
			-- The panels as they open, in place: to see them without a click.
			block(
				"PANELS",
				ui.row {
					gap = "$space.6",
					items = "start",
					flex_wrap = true,
					ContextMenu { still = session_menu(SESSIONS[1]), items = none },
					ContextMenu { still = file_menu(FILES[4]), items = none },
					ContextMenu { still = area_menu(), items = none },
				}
			),
			ds.Text {
				function(): string
					local d = did:get()
					return if d == "" then "Choose something and what it did shows here." else d
				end,
				style = "caption",
				tone = "tertiary",
			},
		}
	end,
	-- At the window's bottom-right corner: its menu has no room to the right
	-- or below, so it opens to the left of the pointer and above it.
	foot = function(): Node
		return ui.row {
			w_full = true,
			justify = "end",
			ContextMenu {
				label = "Session actions",
				open = opened("corner"),
				ring = "$radius.md",
				items = function()
					return session_menu(SESSIONS[2])
				end,
				ui.button {
					label = "Right-click at the corner",
					h = 28,
					px = "$space.3",
					items = "center",
					rounded = "$radius.md",
					bg = "$zeron.secondary.bg",
					hover = { bg = "$zeron.secondary.hover" },
					active = { bg = "$zeron.secondary.active" },
					ds.Text { "Right-click at the corner", style = "caption", tone = "secondary" },
				},
			},
		}
	end,
}
