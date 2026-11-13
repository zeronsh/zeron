-- The session page's right panel, mounted on its own over the made-up disk
-- of ui/files_stub.luau: the panel tests/panel_in_place.test.luau drives.
-- `test_panel_session` is the session open; moving `test_panel_generation`
-- makes the panel again as a reload of the scripts does (its modules' own
-- tables gone, their states kept). Here, beside app.luau, as a mounted
-- script loads modules from its own directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local stub = require("@app/files_stub")
local settle = stub.settle

-- The terminal's host and zgx's terminal, stood in for: an engine terminal
-- a number, its session a table, its view a column saying which.
local tapi = require("@app/screens/panel/terminal_api") :: any
local term_said: { string } = {}
local term_calls = ui.state("test_term_calls", "")
local function term_note(what: string)
	table.insert(term_said, what)
	term_calls:set(table.concat(term_said, ","))
end
local busy = ui.state("test_term_busy", false)
local opened = 0
local sessions: { [string]: any } = {}
-- A new one: its bridge opens it (term-N), its id claimed after.
tapi.start = function(_target: any): any
	opened += 1
	term_note(`open:term-{opened}`)
	return settle({ command = `bridge term-{opened}`, claim = `claim-{opened}` })
end
tapi.claimed = function(claim: string): any
	return settle(`term-{string.match(claim, "%d+")}`)
end
tapi.command = function(id: string, _device: string?): any
	return settle(`bridge {id}`)
end
tapi.status = function(_id: string, _device: string?): any
	return settle({ busy = busy:peek(), exited = false })
end
tapi.close = function(id: string, _device: string?): any
	term_note(`close:{id}`)
	return settle(nil)
end
tapi.term = function(key: string, options: any?): any
	local s = sessions[key]
	if not s and options then
		term_note(`start:{key}:{options.command}`)
		s = {
			title = nil,
			close = function()
				term_note(`end:{key}`)
				sessions[key] = nil
			end,
		}
		-- (The keyboard to its view, as zgx's terminal takes it.)
		s.focus = function(self: any)
			local el = if self.node then self.node:peek() else nil
			if el then
				el:focus()
			end
		end
		sessions[key] = s
	end
	return s
end
-- Its view: what it's called, and each key it gets (`key:<key>`, with the
-- calls), as typed into its shell.
tapi.view = function(props: any): Node
	local r = ui.ref()
	if props.term then
		props.term.node = r
	end
	return ui.column {
		id = "stub-terminal",
		ref = r,
		focusable = true,
		label = "Terminal",
		grow = 1,
		on_key = function(e): boolean?
			if e.cmd or e.ctrl then
				return nil
			end
			term_note(`key:{e.key}`)
			return true
		end,
		ui.text { `terminal {props.label}`, literal = true },
	}
end

-- The browser's host and zgx's web page, stood in for: a page whose fields
-- are states (read as the tab reads zgx's), loading what it's told at
-- once, keeping the scripts run in it; its view a column saying which. What
-- the page would post (a pick, the pick's end) comes from the test
-- (`test.web_message`, with `test_web_message` as JSON-shaped Luau).
local bapi = require("@app/screens/panel/browser_api") :: any
local web_said: { string } = {}
local web_calls = ui.state("test_web_calls", "")
local function web_note(what: string)
	table.insert(web_said, what)
	web_calls:set(table.concat(web_said, ","))
end
local pages: { [string]: any } = {}
local page_handlers: { [string]: any } = {}
local web_nonce = ui.state("test_web_nonce", 0)
local find_at = 1
local function fake_web(id: string): any
	local fields: { [string]: any } = {
		url = ui.state(`test_web_url_{id}`, ""),
		title = ui.state(`test_web_title_{id}`, ""),
		loading = ui.state(`test_web_loading_{id}`, false),
		progress = ui.state(`test_web_progress_{id}`, 0),
		can_go_back = ui.state(`test_web_back_{id}`, false),
		can_go_forward = ui.state(`test_web_forward_{id}`, false),
		error = ui.state(`test_web_error_{id}`, nil :: string?),
		focused = ui.state(`test_web_focused_{id}`, false),
	}
	local methods: { [string]: any } = {
		load = function(_self: any, url: string)
			web_note(`load:{url}`)
			fields.url:set(url)
			fields.title:set(`Page at {string.match(url, "^%a+://([^/]+)") or url}`)
			fields.can_go_back:set(true)
		end,
		back = function()
			web_note("back")
		end,
		forward = function()
			web_note("forward")
		end,
		reload = function()
			web_note("reload")
		end,
		stop = function()
			web_note("stop")
		end,
		focus = function() end,
		blur = function() end,
		-- (The page's zoom, as a percentage: `zoom:125`.)
		zoom = function(_self: any, z: number)
			web_note(`zoom:{math.floor(z * 100 + 0.5)}`)
		end,
		devtools = function()
			web_note("devtools")
		end,
		run = function(_self: any, js: string)
			-- (The picker's whole script, or a call into it: its first word.)
			if string.find(js, "__zeronFind", 1, true) then
				web_note("find:clear")
			elseif string.find(js, "__zeronAnnotator.start", 1, true) then
				web_note("run:start")
				-- (The pick's nonce, for the test to post with.)
				web_nonce:set(tonumber(string.match(js, "__zeronAnnotator%.start%((%d+)") or "0") or 0)
			elseif string.find(js, ".stop()", 1, true) then
				web_note("run:stop")
			elseif string.find(js, ".sync(", 1, true) then
				web_note(`run:sync{string.match(js, "sync%((%b[])%)") or ""}`)
			elseif string.find(js, ".keep(", 1, true) then
				web_note(`run:keep{string.match(js, "keep%((%b[])%)") or ""}`)
			elseif string.find(js, ".label(", 1, true) then
				web_note(`run:label{string.match(js, "label(%b())") or ""}`)
			else
				web_note("run:other")
			end
		end,
		close = function()
			web_note(`close:{id}`)
			pages[id] = nil
		end,
		-- A script asked for its answer: the find's (three matches, the
		-- first current; a step moves it), else nothing.
		eval = function(_self: any, js: string, reply: ((value: any, err: string?) -> ())?)
			if string.find(js, "__zeronFind", 1, true) then
				local q = string.match(js, "%.find%((%b\"\")") or string.match(js, "%.find%(('[^']*')")
				local by = string.match(js, "%.step%((%-?%d+)%)")
				web_note(if q then `find:{string.sub(q, 2, -2)}` elseif by then `step:{by}` else "find:clear")
				if reply then
					if q and q ~= '""' and q ~= "''" then
						find_at = 1
						reply({ count = 3, current = 1 })
					elseif by then
						find_at = ((find_at - 1 + tonumber(by) :: number + 3) % 3) + 1
						reply({ count = 3, current = find_at })
					else
						reply({ count = 0, current = 0 })
					end
				end
			elseif string.find(js, "icon", 1, true) then
				web_note("eval:icon")
				if reply then
					reply(nil, nil)
				end
			elseif reply then
				reply(nil, nil)
			end
		end,
	}
	return setmetatable({}, {
		__index = function(_, k: string): any
			local s = fields[k]
			if s then
				return s:get()
			end
			return methods[k]
		end,
	})
end
bapi.web = function(id: string, _options: any?): any
	local w = pages[id]
	if not w then
		w = fake_web(id)
		pages[id] = w
		-- (Its id, for a test to reach its fields: `test_web_error_<id>`.)
		ui.state("test_web_last_id", ""):set(id)
	end
	return w
end
bapi.view = function(props: any): Node
	page_handlers[props.id or "page"] = props
	return ui.column {
		id = "stub-web",
		grow = 1,
		ui.text {
			function(): string
				return `page {props.web.url}`
			end,
		},
	}
end
bapi.open_externally = function(url: string)
	web_note(`external:{url}`)
end
-- The page asking for a window (a popup, target=_blank): `test_web_window`
-- to the view shown.
local web_window = ui.state("test_web_window", "")
ui.command {
	id = "test.web_new_window",
	title = "Web New Window",
	run = function()
		for _, h in page_handlers do
			if h.on_new_window then
				h.on_new_window(web_window:peek())
			end
		end
	end,
}
-- A chip taken off as the composer's × does (`test_remove_cargo`, its id):
-- the rest renumbered.
local remove_id = ui.state("test_remove_cargo", "")
ui.command {
	id = "test.remove_cargo",
	title = "Remove Chip",
	run = function()
		local shared: any = require("@app/screens/chat/shared")
		shared.remove_cargo(remove_id:peek())
	end,
}
-- A download the page starts (`test_web_download_mode`: "save" picks
-- /tmp/report.pdf, then 50 of 100 bytes, then done; "cancel" closes the
-- dialog; "fail" saves and fails), to the view shown.
local web_download_mode = ui.state("test_web_download_mode", "save")
local web_download_step = ui.state("test_web_download_step", 0)
local download_opts: any = nil
ui.command {
	id = "test.web_download",
	title = "Web Download",
	run = function()
		local mode = web_download_mode:peek()
		local d = {
			url = "https://example.com/report.pdf",
			name = "report.pdf",
			mime = "application/pdf",
		}
		function d.ask(_self: any, opts: any): any
			download_opts = opts
			web_note(`download:ask:{d.name}`)
			return {
				after = function(_p: any, ok: (any) -> (), _err: any)
					if mode == "cancel" then
						ok(nil)
					else
						ok({ path = "/tmp/report.pdf", name = "report.pdf", size = 0, type = "application/pdf" })
					end
				end,
			}
		end
		for _, h in page_handlers do
			if h.on_download then
				h.on_download(d)
			end
		end
	end,
}
-- The download's next step: its bytes half there, then its end (as mode).
ui.command {
	id = "test.web_download_step",
	title = "Web Download Step",
	run = function()
		if not download_opts then
			return
		end
		local step = web_download_step:peek() + 1
		web_download_step:set(step)
		if step == 1 then
			download_opts.on_progress(50, 100)
		else
			local fail = web_download_mode:peek() == "fail"
			download_opts.on_finish(not fail, if fail then "disk full" else nil)
		end
	end,
}
-- The page moving on (`test_web_nav_kind`: "started", "committed",
-- "finished"), to the view shown.
local web_nav_kind = ui.state("test_web_nav_kind", "committed")
ui.command {
	id = "test.web_navigate",
	title = "Web Navigate",
	run = function()
		for _, h in page_handlers do
			if h.on_navigate then
				h.on_navigate({ kind = web_nav_kind:peek(), url = "https://example.com/next" })
			end
		end
	end,
}
-- The page posting to the tab: what `test_web_message` holds, to the view
-- shown.
local web_message = ui.state("test_web_message", nil :: any)
ui.command {
	id = "test.web_message",
	title = "Web Message",
	run = function()
		for _, h in page_handlers do
			if h.on_message then
				h.on_message(web_message:peek(), "https://example.com", true)
			end
		end
	end,
}

local Panel = require("@app/screens/panel")
local Workspace = require("@app/screens/files/workspace")

local session = ui.state("test_panel_session", { id = "chat-1", cwd = "/proj" } :: any)
local generation = ui.state("test_panel_generation", 1)
-- How many times closing the last tab closed the panel.
local panel_closed = ui.state("test_panel_closed", 0)
-- The app's ⌘W, as the shell calls it: what the panel answered.
local visible = ui.state("test_panel_visible", true)
local closed = ui.state("test_panel_closed_focused", "")
ui.command {
	id = "test.close_focused_tab",
	title = "Close Focused Tab",
	run = function()
		closed:set(tostring(Panel.close_focused_tab()))
	end,
}
ui.command {
	id = "test.close_active_tab",
	title = "Close Active Tab",
	run = function()
		closed:set(tostring(Panel.close_active_tab()))
	end,
}
-- ⌃Tab as the shell calls it: what the panel answered.
local stepped = ui.state("test_panel_stepped", "")
ui.command {
	id = "test.step_tab",
	title = "Next Tab",
	run = function()
		stepped:set(tostring(Panel.step_tab(1)))
	end,
}
-- ⌘J as the shell calls it (Panel.toggle_terminal).
ui.command {
	id = "test.toggle_terminal",
	title = "Terminal",
	run = function()
		Panel.toggle_terminal()
	end,
}
-- `/files` and `/terminal` as the chat page calls them.
ui.command {
	id = "test.show_files",
	title = "Files",
	run = function()
		Panel.show_files()
	end,
}
ui.command {
	id = "test.show_terminal",
	title = "New Terminal",
	run = function()
		Panel.show_terminal()
	end,
}
-- The app's ⌘F, as the shell calls it: what the panel answered.
local found = ui.state("test_find_in_browser", "")
ui.command {
	id = "test.find",
	title = "Find",
	run = function()
		found:set(tostring(Panel.find_in_browser()))
	end,
}
-- A web link from a chat, opened in its session's panel (Panel.open_url).
local open_url = ui.state("test_open_url", "")
ui.command {
	id = "test.open_url",
	title = "Open Link",
	run = function()
		Panel.open_url(session:peek(), open_url:peek())
	end,
}
-- A file mentioned in a chat, opened in its session's panel (Panel.open_file).
local open_path = ui.state("test_open_path", "")
-- (And where in it, as a link to `path:line` gives it: `{ line, column?,
-- end_line? }`, or nil.)
local open_at = ui.state("test_open_at", nil :: any)
ui.command {
	id = "test.open_file",
	title = "Open File",
	run = function()
		Panel.open_file(session:peek(), open_path:peek(), nil, open_at:peek())
	end,
}
-- A plugin's tab (the Git plugin's, from ZERON_ZGX_DEV_PLUGINS), opened as
-- its own rows open it (Panel.open_plugin_tab): for captures beside the
-- panel's own tabs.
-- Git's Changes and History, each its own kind of tab, as the plugin asks
-- for them (plugins-builtin/git repo.luau `open_tab`).
ui.command {
	id = "test.open_git",
	title = "Open Git's Changes",
	run = function()
		local s = session:peek()
		Panel.open_plugin_tab(s.id, { plugin = "git", kind = "git-changes", title = "Changes", icon = "file-diff", data = { session = s.id } })
	end,
}
ui.command {
	id = "test.open_git_history",
	title = "Open Git's History",
	run = function()
		local s = session:peek()
		Panel.open_plugin_tab(s.id, { plugin = "git", kind = "git-history", title = "History", icon = "git-commit", data = { session = s.id } })
	end,
}
-- Two commits whose subjects are the same, each in a tab of its own: the
-- panel tells them apart by `data.key`.
for _, which in { "a", "b" } do
	ui.command {
		id = `test.open_commit_{which}`,
		title = `Open commit {which}`,
		run = function()
			local s = session:peek()
			Panel.open_plugin_tab(s.id, { plugin = "git", kind = "git-commit", title = "Fix the build", icon = "git-commit", data = { session = s.id, key = `commit:{which}` } })
		end,
	}
end
local built = generation:peek()

return ui.column {
	w_full = true,
	h_full = true,
	-- (As the shell's root: every focus reaches it, after the panel.)
	on_focus = function()
		Panel.focus_moved()
	end,
	-- (As the shell's root: a key nothing took, the panel home's letter.)
	on_key = function(e): boolean
		return Panel.home_key(e)
	end,
	ui.presence(function(): string
		return tostring(generation:get())
	end, function(): Node
		if generation:peek() ~= built then
			built = generation:peek()
			Workspace.forget()
		end
		return Panel.Panel {
			visible = function(): boolean
				return visible:get()
			end,
			session = session,
			project = { id = "p1", path = "/proj", name = "proj" },
			-- (As the shell closes the panel: here, how many times.)
			on_last_closed = function()
				panel_closed:set(panel_closed:peek() + 1)
			end,
			w_full = true,
			h_full = true,
		}
	end, { w_full = true, grow = 1, min_h = 0 }),
	-- Something outside the panel that takes the keyboard (the chat's).
	ui.div { id = "outside", label = "Outside", role = "button", focusable = true, h = 4, w_full = true },
}
