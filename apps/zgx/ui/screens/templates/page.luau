--!strict
-- Settings → Templates: the whole interfaces Zeron can run in, laid out as
-- Settings → Plugins is (screens/plugins/page), both through
-- ds.Settings.Catalog: the same search, tabs, card of rows and empty states.
-- Its tabs are what's installed (Zeron's own, built in, first; yours; and
-- others', imported, each by who signed it) and what's yours. Zeron's
-- official templates come after its own, built in as it is: each recolours
-- Zeron and changes nothing else (src/plugins/theme_templates.rs). Every
-- row's preview is drawn from its template's colours (src/plugins/looks.rs):
-- a swatch of each appearance it changes, or Zeron's own.
--
-- Templates are shared as signed .zeron-template files: Export… (a row's
-- ⋯) signs one as yours; Install brings one in, as Plugins' does: from a
-- link (a .zeron-template's, or a GitHub repository or release with one)
-- or a file, the dialog open and its Install spinning until it's done.
-- Someone else's waits, named above the rows, until you allow it in
-- Zeron's consent window (its own window: nothing here can allow it). One
-- from a package is checked where it came from for a newer version as the
-- page opens (the engine checks every few hours too); a row with one says
-- so and has Update. One whose maker Zeron can't vouch for says
-- Unverified beside its version, and where it came from under its name.
--
-- Each row is its template's as the list changes: its colours' preview, its
-- name and version, who made it and how it stands (in use, broken with why,
-- made for an earlier Zeron), Use (or "In use") and a ⋯ menu of its folder,
-- Duplicate and Delete. Zeron's own can't be deleted. What's at a row's end
-- swaps only when the template's use does.
--
-- Agents make and change templates (Zeron's MCP tools), so the page says to
-- ask one.

local ds = require("@ds")
local Menu = require("@ds/menu")
local control = require("@ds/control")
local state = require("@app/screens/templates/state")
local words = require("@app/screens/plugins/words")

local M = {}

-- A preview's box in a row: a window's shape, a row's tile high.
local THUMB_W = 48
local THUMB_H = 32
local THUMB_RADIUS = "$radius.md"

local tab = ui.state("templates_tab", "installed")
local query = ui.state("templates_query", "")
local deleting = ui.state("templates_deleting", nil :: string?)
local deleting_open = ui.state("templates_deleting_open", false)
local naming = ui.state("templates_naming", false)
local new_name = ui.state("templates_new_name", "")
-- Install's dialog, its link, and an install on its way (the dialog stays
-- open, its Install spinning, until it's done or fails).
local installing = ui.state("templates_installing", false)
local install_link = ui.state("templates_install_link", "")
local install_busy = ui.state("templates_install_busy", false)
-- The template being updated, and when the page last had sources checked
-- (at most every ten minutes).
local updating = ui.state("templates_updating", nil :: string?)
local checked_at = ui.state("templates_checked_at", 0)

-- Its maker can't be vouched for: someone else's, signed by a key Zeron
-- doesn't know (as a plugin's: words.unverified).
local function unverified(t: any): boolean
	return not t.builtin and t.imported ~= nil and words.unverified(t.imported)
end

local function author(t: state.Template): string
	if t.builtin then
		return ui.t("templates.page.author.zeron"):get()
	elseif state.yours(t) then
		return ui.t("templates.page.author.you"):get()
	end
	local signed = t.imported and t.imported.author
	if signed then
		return if signed.kind == "verified" and signed.username
			then ui.t("templates.page.author.verified", { username = signed.username }):get()
			else words.from(t.imported and t.imported.source) or ui.t("templates.page.author.unknown"):get()
	end
	return ui.t("templates.page.author.named", { name = tostring(t.author) }):get()
end

-- What a template's row says under its name.
local function about(t: state.Template): string
	if t.status == "broken" then
		return ui.t("templates.page.about.broken", { error = t.error or "" }):get()
	elseif t.status == "errors" then
		return ui.t("templates.page.about.errors", { error = t.error or "" }):get()
	elseif t.status == "out_of_date" then
		local made_for = t.zeron_version
		return if made_for and made_for ~= ""
			then ui.t("templates.page.about.out_of_date", { version = made_for }):get()
			else ui.t("templates.page.about.out_of_date_unversioned"):get()
	end
	if t.description and t.description ~= "" then
		return t.description
	end
	return ui.t("templates.page.about.no_description"):get()
end

local function matches(t: state.Template, q: string): boolean
	if q == "" then
		return true
	end
	local hay = string.lower(`{t.name} {t.description or ""} {author(t)}`)
	return string.find(hay, q, 1, true) ~= nil
end

-- The templates the tab holds that the search finds.
local found = ui.computed(function(): { state.Template }
	local q = string.lower(query:get())
	local mine = tab:get() == "yours"
	local out: { state.Template } = {}
	for _, t in state.list:get() do
		if (not mine or state.yours(t)) and matches(t, q) then
			table.insert(out, t)
		end
	end
	return out
end)

local function count(which: string): () -> number
	return function(): number
		local n = 0
		for _, t in state.list:get() do
			if which == "installed" or state.yours(t) then
				n += 1
			end
		end
		return n
	end
end

-- What went wrong asking the host, said once.
local function failed(title: string | Message): (err: any) -> ()
	return function(err: any)
		ds.toast.show {
			title = title,
			body = if type(err) == "table" and err.message then tostring(err.message) else nil,
			tone = "warning",
		}
	end
end

-- A host call's outcome: `ok` with its value, or a toast saying what
-- failed. (A stub answers nothing.)
local function settle(p: any, title: string | Message, ok: ((value: any) -> ())?)
	if p == nil then
		return
	end
	local promise: any = p
	promise:after(ok or function(_: any) end, failed(title))
end

local function new_template()
	new_name:set("")
	naming:set(true)
end

-- (Its name read as the dialog opens, in the locale then.)
local function package_filter(): { any }
	return { { ui.t("templates.page.package_filter"):get(), { "zeron-template" } } }
end

-- Export…: where to save it, then it, signed as yours.
local function export_file(id: string, name: string)
	ui.task(function()
		local file = ui.save_file({ title = ui.t("templates.page.export_title"):get(), name = `{id}.zeron-template`, filters = package_filter() }):await()
		if not file then
			return
		end
		settle(state.export(id, file.path), ui.t("templates.page.couldnt_export", { name = name }), function(_: any)
			ds.toast.show {
				title = ui.t("templates.page.exported", { name = name }),
				body = ui.t("templates.page.exported_body"),
				tone = "success",
			}
		end)
	end)
end

-- Install: a link or a .zeron-template file. Yours installs; someone
-- else's opens Zeron's consent window (the host's), and waits above the
-- rows. The dialog closes once it's done; failing, it stays, to try again.
local function install(from: { path: string?, url: string? })
	local p: any = state.import(from)
	if p == nil then
		installing:set(false)
		return
	end
	install_busy:set(true)
	p:after(function(done: any)
		install_busy:set(false)
		installing:set(false)
		install_link:set("")
		if type(done) == "table" and done.installed then
			tab:set("installed")
			ds.toast.show { title = ui.t("templates.page.installed_toast", { name = tostring(done.name or done.installed) }), tone = "success" }
		end
	end, function(err: any)
		install_busy:set(false)
		failed(ui.t("templates.page.couldnt_install"))(err)
	end)
end

local function choose_file()
	if install_busy:peek() then
		return
	end
	ui.task(function()
		local files = ui.open_files({ title = ui.t("templates.page.install_title"):get(), filters = package_filter() }):await()
		local file = files and files[1]
		if file then
			install { path = file.path }
		end
	end)
end

-- The update found for it, installed in its place.
local function update(id: string, name: string)
	updating:set(id)
	local p: any = state.update(id)
	if p == nil then
		updating:set(nil)
		return
	end
	p:after(function(done: any)
		updating:set(nil)
		local version = if type(done) == "table" and type(done.version) == "string" and done.version ~= "" then done.version else nil
		ds.toast.show {
			title = if version
				then ui.t("templates.page.updated_toast_version", { name = name, version = version })
				else ui.t("templates.page.updated_toast", { name = name }),
			tone = "success",
		}
	end, function(err: any)
		updating:set(nil)
		failed(ui.t("templates.page.couldnt_update", { name = name }))(err)
	end)
end

-- Update, at the end of the line saying a newer version was found:
-- link-like, as Plugins' is.
local UpdateLink = ui.component(function(props: { now: State<any> }): Node
	local now = props.now
	local function busy(): boolean
		return updating:get() == now:get().id
	end
	return ui.show(function(): boolean
		local it: any = now:get()
		return type(it.update) == "table" and it.update.refused == nil and not it.builtin
	end, ui.row {
		role = "button",
		label = ui.t("templates.page.update"),
		focusable = function(): boolean
			return not busy()
		end,
		disabled = busy,
		shrink = 0,
		cursor = "pointer",
		color = "$text.primary",
		hover = { color = "$accent" },
		on_click = function()
			if not busy() then
				update(now:peek().id, now:peek().name)
			end
		end,
		ui.text {
			function(): string
				return if busy() then ui.t("templates.page.updating"):get() else ui.t("templates.page.update"):get()
			end,
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			weight = "$font.weight.medium",
			underline = true,
			underline_offset = 4,
		},
	})
end)

-- Someone else's packages, waiting for you: each with Review (its consent
-- window). Nothing when none waits.
local function waiting(): Node
	return ui.each(state.pending, function(p: state.Pending): Node
		return ui.column {
			id = `template-pending-{p.pending}`,
			class = "template-pending",
			w_full = true,
			ds.Callout {
				tone = "info",
				title = ui.t("templates.page.waiting", { name = p.name }),
				body = ui.t("templates.page.waiting_body"),
				action = {
					label = ui.t("templates.page.review"),
					on_click = function()
						state.review(p.pending)
					end,
				},
			},
		}
	end, {
		key = function(p: state.Pending): string
			return p.pending
		end,
		w_full = true,
		gap = "$space.2",
	})
end

-- A template's preview, drawn from its colours (no picture needed): each
-- appearance it changes, side by side (light, then dark), or Zeron's own
-- two for one that changes none, one alone across the whole card. Each half
-- wears that appearance's theme (state.previews) and draws its window, a
-- raised bar, its text's two inks, and its accent and code's colours as
-- dots.
local function swatch_half(id: string, i: number): Node
	local function names(): { string }
		return state.previews:get()[id] or {}
	end
	local function bar(color: string, width: string): Node
		return ui.div { w = width, h = 3, shrink = 0, rounded = "$radius.sm", bg = color }
	end
	local dots: { Node } = {}
	for _, color in { "$accent", "$zeron.syntax.keyword", "$zeron.syntax.string", "$zeron.syntax.function" } do
		table.insert(dots, ui.div { w = 4, h = 4, shrink = 0, rounded = "$radius.sm", bg = color })
	end
	return ui.show(
		function(): boolean
			return #names() >= i
		end,
		ui.column {
			theme = function(): string
				return names()[i] or "zeron-dark"
			end,
			-- (Inside the box's 1pt edge, the halves sharing it.)
			w = function(): number
				return (THUMB_W - 2) / math.max(#names(), 1)
			end,
			shrink = 0,
			h_full = true,
			bg = "$zeron.window",
			class = "template-swatch-half",
			ui.div { w_full = true, h = 6, shrink = 0, bg = "$popover.bg" },
			ui.column {
				p = "$zeron.space.hair",
				gap = "$zeron.space.hair",
				pt = "$space.1",
				bar("$text.primary", "70%"),
				bar("$text.secondary", "45%"),
				ui.row { gap = "$zeron.space.hair", pt = "$zeron.space.hair", table.unpack(dots) },
			},
		}
	)
end

local function preview(t: state.Template): Node
	return ui.row {
		w = THUMB_W,
		h = THUMB_H,
		shrink = 0,
		rounded = THUMB_RADIUS,
		overflow = "hidden",
		border = 1,
		border_color = "$zeron.settings.edge",
		aria_hidden = true,
		class = "template-swatch",
		swatch_half(t.id, 1),
		swatch_half(t.id, 2),
	}
end

-- Use and "In use" take the same width, the wider of the two as laid out
-- (each reports its own; it only grows), so a row's end never shifts as
-- the one in use changes.
local slot = ui.state("templates_use_w", 0)
local function slot_w(): number
	return slot:get()
end
local function grow(size: any)
	if type(size) == "table" and type(size.w) == "number" and size.w > slot:peek() then
		slot:set(size.w)
	end
end

-- Use, or "In use"; then its ⋯ menu.
local function controls(t: state.Template, now: State<any>): Node
	local id = t.id
	local menu_open = ui.use_state(false)
	local more = ds.IconButton {
		icon = "more-horizontal",
		label = ui.t("templates.page.more_for", { name = t.name }),
		size = "sm",
		tone = "quiet",
		on_click = function()
			menu_open:set(not menu_open:peek())
		end,
	}
	local function pick(run: () -> ()): () -> ()
		return function()
			menu_open:set(false)
			run()
		end
	end
	local rows: { Node } = {
		Menu.Group {
			Menu.Row {
				icon = "folder",
				label = ui.t("templates.page.show_folder"),
				role = "menuitem",
				on_click = pick(function()
					state.open_folder(id)
				end),
			},
			Menu.Row {
				icon = "copy",
				label = ui.t("templates.page.duplicate"),
				role = "menuitem",
				on_click = pick(function()
					settle(state.duplicate(id), ui.t("templates.page.couldnt_duplicate", { name = now:peek().name }), function(new_id: any)
						if type(new_id) == "string" then
							tab:set("yours")
						end
					end)
				end),
			},
			-- (Not Zeron's own: it's everyone's already.)
			if t.builtin
				then nil
				else Menu.Row {
					icon = "archive-minimalistic",
					label = ui.t("templates.page.export"),
					role = "menuitem",
					on_click = pick(function()
						export_file(id, now:peek().name)
					end),
				},
		},
	}
	if not t.builtin then
		table.insert(rows, Menu.Rule {})
		table.insert(
			rows,
			Menu.Group {
				Menu.Row {
					icon = "trash-bin-minimalistic",
					label = ui.t("common.delete"),
					role = "menuitem",
					danger = true,
					on_click = pick(function()
						deleting:set(id)
						deleting_open:set(true)
					end),
				},
			}
		)
	end
	local panel: { [any]: any } = {
		open = menu_open,
		anchor = more,
		placement = { side = "bottom", align = "end", offset = 4 },
		w = 200,
	}
	for _, r in rows do
		table.insert(panel, r)
	end
	return ui.row {
		gap = "$space.2",
		items = "center",
		-- Unverified, before its version, for one whose maker Zeron can't
		-- vouch for.
		ui.show(function(): boolean
			return unverified(now:get())
		end, ds.Badge { ui.t("plugins.words.unverified"), tone = "warning", class = "template-unverified" }),
		-- Its version, centred on the row.
		ui.show(function(): boolean
			local v = now:get().version
			return v ~= nil and v ~= ""
		end, ds.Badge {
			function(): string
				return now:get().version or ""
			end,
			tone = "neutral",
			tabular = true,
		}),
		ui.switch(function(): string
			return if now:get().active then "using" else "use"
		end, {
			-- (In use, the name says so: Use's room kept empty, so the
			-- versions and ⋯ stay in one column down the list.)
			using = ui.div { min_w = slot_w, h = control.H.sm },
			use = ds.Button {
				tone = "secondary",
				size = "sm",
				min_w = slot_w,
				on_resize = grow,
				on_click = function()
					settle(state.use(id), ui.t("templates.page.couldnt_use", { name = now:peek().name }))
				end,
				ui.t("templates.page.use"),
			},
		}),
		-- (The menu draws its anchor, the ⋯, in place.)
		ui.menu(Menu.panel(panel)),
	}
end

local function row(t: state.Template, _i: number, now: State<any>): Node
	return ui.column {
		id = `template-{t.id}`,
		w_full = true,
		ds.Settings.Row {
			label = function(): string
				return now:get().name
			end,
			description = function(): string
				return about(now:get())
			end,
			-- Who made it, on its own line.
			byline = function(): string
				return author(now:get())
			end,
			lead = preview(t),
			badge = ui.show(function(): boolean
				return now:get().active == true
			end, ds.Badge { ui.t("templates.page.in_use"), tone = "accent", class = "template-in-use" }),
			-- A newer version found where it came from.
			detail = function(): string?
				local it: any = now:get()
				return if it.builtin then nil else words.update(it)
			end,
			detail_action = UpdateLink { now = now },
			truncate = true,
			control = controls(t, now),
		},
	}
end

-- What Yours says with nothing in it (Installed always holds Zeron's own).
local function none(_tab: string): Node
	return ds.EmptyState {
		icon = "gallery",
		title = ui.t("templates.page.none_yours"),
		body = ui.t("templates.page.none_body"),
		size = "sm",
		action = {
			label = ui.t("templates.page.show_templates_folder"),
			on_click = function()
				state.open_folder("")
			end,
		},
	}
end

local function dialogs(): { Node }
	return {
		ds.Dialog {
			open = installing,
			title = ui.t("templates.page.install_dialog_title"),
			body = ui.t("templates.page.install_dialog_body"),
			size = "sm",
			confirm = {
				label = ui.t("templates.page.install"),
				keep_open = true,
				busy = function(): boolean
					return install_busy:get()
				end,
				on_click = function()
					local link = string.match(install_link:peek(), "^%s*(.-)%s*$") or ""
					if link ~= "" then
						install { url = link }
					end
				end,
			},
			ds.TextField {
				value = install_link,
				label = ui.t("templates.page.link"),
				placeholder = "github.com/owner/template",
				select_on_focus = true,
				w_full = true,
			},
			ds.Button {
				id = "templates-choose-file",
				tone = "secondary",
				size = "sm",
				disabled = function(): boolean
					return install_busy:get()
				end,
				on_click = choose_file,
				ui.t("templates.page.choose_file"),
			},
		},
		ds.Dialog {
			open = deleting_open,
			title = ui.t("templates.page.delete_title"),
			body = ui.t("templates.page.delete_body"),
			tone = "danger",
			confirm = {
				label = ui.t("common.delete"),
				on_click = function()
					local id = deleting:peek()
					deleting:set(nil)
					if id then
						settle(state.delete(id), ui.t("templates.page.couldnt_delete"))
					end
				end,
			},
		},
		ds.Dialog {
			open = naming,
			title = ui.t("templates.page.new_title"),
			body = ui.t("templates.page.new_body"),
			size = "sm",
			confirm = {
				label = ui.t("templates.page.make_it"),
				on_click = function()
					local name = string.match(new_name:peek(), "^%s*(.-)%s*$") or ""
					if name ~= "" then
						tab:set("yours")
						settle(state.create(name), ui.t("templates.page.couldnt_make"))
					end
				end,
			},
			ds.TextField {
				value = new_name,
				label = ui.t("templates.page.name"),
				placeholder = ui.t("templates.page.name_placeholder"),
				w_full = true,
				select_on_focus = true,
			},
		},
	}
end

function M.View(): Node
	-- As it opens: each one from a package checked where it came from.
	if os.time() - checked_at:peek() > 600 then
		checked_at:set(os.time())
		local p: any = state.check_updates()
		if p ~= nil then
			p:after(function(_: any) end, function(_: any) end)
		end
	end
	return ds.Settings.Catalog {
		title = ui.t("templates.page.title"),
		description = ui.t("templates.page.description"),
		search = { value = query, placeholder = ui.t("templates.page.search") },
		tabs = {
			value = tab,
			label = ui.t("templates.page.tabs"),
			options = {
				{ value = "installed", label = ui.t("templates.page.installed"), count = count("installed") },
				{ value = "yours", label = ui.t("templates.page.yours"), count = count("yours") },
			},
		},
		-- (No New for now: templates of yours arrive by an agent or as a
		-- copy of Zeron's. Install brings in someone else's, as Plugins'.)
		import = {
			label = ui.t("templates.page.install_more"),
			on_click = function()
				installing:set(true)
			end,
		},
		notice = waiting,
		-- (Only while one waits: none, it takes no room over the card.)
		notice_when = function(): boolean
			return #state.pending:get() > 0
		end,
		status = function(): string
			return if state.now:get().state == "ready" then "ready" else "loading"
		end,
		found = found,
		row = row,
		key = function(t: state.Template): string
			return t.id
		end,
		none = none,
		overlays = dialogs(),
	}
end

return M
