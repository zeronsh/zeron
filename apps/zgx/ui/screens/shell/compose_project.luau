-- New project from repositories: a project of repositories from anywhere
-- on one device, each staying where it is. New project's "From
-- repositories…" on a device opens it, as does Edit project's "Combine with
-- other repositories…" for a project that is one repository.
--
--   New project
--   Repositories from anywhere on MacBook Pro, worked on together.
--   Name  [acme                          ]
--         ~/.zeron/projects/acme
--   [🔍 Search repositories     ] [+ New] [↓ Clone]
--   ┌───────────────────────────────────────────┐
--   │ ☑ api        ~/code/api       acme/api ⑂ main │
--   │ ☐ web-app    ~/work/web-app            ⑂ main │
--   └───────────────────────────────────────────┘
--   2 chosen                                  Clear
--                               [Cancel] [Create project]
--
-- New and Clone turn the dialog to their form, and back to the list with
-- the repository made there chosen. The name, untouched, follows what's
-- chosen (their GitHub owner when they share one, else the first one's
-- name). Created (`ComposeProject` on the device, then `createSpace`), the
-- window lands in a new chat in it.
--
--   compose.open(device, preset?)   preset: { chosen = { path }, name? }
--   compose.Dialog()                placed once in the window

local ds = require("@ds")
local Icon = require("@ds/icon")
local control = require("@ds/control")
local locale = require("@ds/locale")
local projects = require("@account/projects")
local repositories = require("@app/screens/shell/repositories")

local M = {}

local open = ui.state("shell_compose_open", false)
local device = ui.state("shell_compose_device", "")
local name = ui.state("shell_compose_name", "")
local chosen = ui.state("shell_compose_chosen", {} :: { [string]: boolean })
-- The order they were chosen in (the project lists them so).
local order = ui.state("shell_compose_order", {} :: { string })
local query = ui.state("shell_compose_query", "")
local view = ui.state("shell_compose_view", "choose")
local busy = ui.state("shell_compose_busy", false)
local failed = ui.state("shell_compose_failed", nil :: string?)

-- Keep the order chosen in step with what's chosen.
ui.effect(function()
	local now = chosen:get()
	local before = order:peek()
	local next: { string } = {}
	local seen: { [string]: boolean } = {}
	for _, path in before do
		if now[path] then
			table.insert(next, path)
			seen[path] = true
		end
	end
	for path in now do
		if not seen[path] then
			table.insert(next, path)
		end
	end
	if #next ~= #before then
		order:set(next)
	else
		for i, p in next do
			if before[i] ~= p then
				order:set(next)
				break
			end
		end
	end
end)

function M.open(on: string, preset: { chosen: { string }?, name: string? }?)
	local lib = repositories.library(on)
	lib.load()
	local picked: { [string]: boolean } = {}
	for _, path in (preset and preset.chosen) or {} do
		picked[path] = true
	end
	ui.batch(function()
		device:set(on)
		name:set((preset and preset.name) or "")
		order:set((preset and preset.chosen) or {})
		chosen:set(picked)
		query:set("")
		view:set("choose")
		busy:set(false)
		failed:set(nil)
		open:set(true)
	end)
end

local function by_path(): { [string]: repositories.Repository }
	local out = {}
	for _, r in repositories.library(device:get()).list:get() do
		out[r.path] = r
	end
	return out
end

-- The name it gets when none is typed.
local function suggested(): string
	local known = by_path()
	local picked: { repositories.Repository } = {}
	for _, path in order:get() do
		local r = known[path]
		if r then
			table.insert(picked, r)
		end
	end
	if #picked == 0 then
		return ""
	end
	local owner: string? = nil
	local shared = #picked > 1
	for _, r in picked do
		local o = if r.github then string.match(r.github, "^([^/]+)/") else nil
		if not o or (owner and string.lower(owner) ~= string.lower(o)) then
			shared = false
			break
		end
		owner = o
	end
	if shared and owner then
		return owner
	end
	return picked[1].name
end

local function the_name(): string
	local typed = string.match(name:get(), "^%s*(.-)%s*$") or ""
	return if typed ~= "" then typed else suggested()
end

local function count(): number
	return #order:get()
end

local function create()
	if busy:peek() then
		return
	end
	if view:peek() == "new" then
		repositories.submit_new(device:peek(), function(r)
			chosen:set_key({ r.path }, true)
			view:set("choose")
		end)
		return
	elseif view:peek() == "clone" then
		repositories.submit_clone(device:peek(), function(r)
			chosen:set_key({ r.path }, true)
			view:set("choose")
		end)
		return
	end
	local title = the_name()
	if count() == 0 or title == "" then
		return
	end
	local members = {}
	for _, path in order:peek() do
		table.insert(members, { path = path })
	end
	failed:set(nil)
	busy:set(true)
	local on = device:peek()
	projects.compose(on, title, members):after(function(made: any)
		if type(made) ~= "table" or type(made.path) ~= "string" then
			busy:set(false)
			return
		end
		local add_project = require("@app/screens/shell/add_project")
		add_project.add_named(on, made.path, title, function(err: any?)
			busy:set(false)
			if err then
				failed:set(repositories.says(err))
				return
			end
			open:set(false)
		end)
	end, function(err: any)
		busy:set(false)
		failed:set(repositories.says(err))
	end)
end

-- A form's way back to the list.
local function back(words: Message): Node
	return ui.button {
		id = "compose-back",
		label = words :: any,
		h = 24,
		px = "$space.1",
		ml = -4,
		rounded = "$radius.sm",
		gap = "$space.1",
		items = "center",
		text_size = "$font.size.sm",
		color = "$text.secondary",
		bg = "#00000000",
		hover = { bg = "$hover", color = "$text.primary" },
		focus = control.ring,
		on_click = function()
			if not repositories.busy:peek() then
				view:set("choose")
			end
		end,
		Icon { name = "alt-arrow-left", size = 14 },
		ui.text { words },
	}
end

local function choose_view(): Node
	return ui.column {
		w_full = true,
		gap = "$space.4",
		ds.TextField {
			value = name,
			label = locale.t("shell.compose.name"),
			placeholder = function(): string
				local s = suggested()
				return if s ~= "" then s else locale.s("shell.compose.name_placeholder")
			end,
			w_full = true,
			hint = function(): string
				local title = the_name()
				if title == "" then
					return locale.s("shell.compose.folder_hint")
				end
				return locale.s("shell.compose.folder", { path = `~/.zeron/projects/{title}` })
			end,
		},
		ui.column {
			w_full = true,
			gap = "$space.2",
			ui.text {
				locale.t("shell.compose.repositories"),
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = "$text.secondary",
			},
			repositories.Picker {
				id = "compose-picker",
				device = function(): string
					return device:get()
				end,
				chosen = chosen,
				query = query,
				on_new = function()
					repositories.reset_new(device:peek(), string.match(query:peek(), "^%s*(.-)%s*$"))
					view:set("new")
				end,
				on_clone = function()
					repositories.reset_clone(device:peek())
					view:set("clone")
				end,
			},
		},
		ui.show(function(): boolean
			return failed:get() ~= nil
		end, ui.row {
			id = "compose-error",
			role = "alert",
			w_full = true,
			gap = "$space.2",
			items = "center",
			color = "$danger",
			Icon { name = "danger-triangle", size = 14 },
			ui.text {
				function(): string
					return failed:get() or ""
				end,
				text_size = "$font.size.sm",
				wrap = true,
				grow = 1,
			},
		}),
	}
end

function M.Dialog(): Node
	local function on(): string
		return device:get()
	end
	return ds.Dialog {
		open = open,
		size = "lg",
		title = function(): string
			local v = view:get()
			if v == "new" then
				return locale.s("shell.repositories.new_title")
			elseif v == "clone" then
				return locale.s("shell.repositories.clone_title")
			end
			return locale.s("shell.compose.title")
		end,
		body = function(): string
			local v = view:get()
			local where = repositories.device_name(device:get())
			if v == "new" then
				return locale.s("shell.repositories.new_description", { device = where })
			elseif v == "clone" then
				return locale.s("shell.repositories.clone_description", { device = where })
			end
			return locale.s("shell.compose.description", { device = where })
		end,
		hold = function(): boolean
			return busy:get() or repositories.busy:get()
		end,
		confirm = {
			label = function(): string
				local v = view:get()
				if v == "new" then
					return locale.s("shell.repositories.new_form.go")
				elseif v == "clone" then
					return locale.s("shell.repositories.clone_form.go")
				end
				local n = count()
				return if n == 0 then locale.s("shell.compose.create") else locale.s("shell.compose.create_n", { n = n })
			end,
			keep_open = true,
			busy = function(): boolean
				return busy:get() or repositories.busy:get()
			end,
			disabled = function(): boolean
				return view:get() == "choose" and (count() == 0 or the_name() == "")
			end,
			on_click = create,
		},
		ui.switch(function(): string
			return view:get()
		end, {
			choose = choose_view(),
			new = ui.column {
				w_full = true,
				gap = "$space.3",
				back(locale.t("shell.compose.back")),
				repositories.NewForm(on),
			},
			clone = ui.column {
				w_full = true,
				gap = "$space.3",
				back(locale.t("shell.compose.back")),
				repositories.CloneForm(on),
			},
		}),
	}
end

return M
