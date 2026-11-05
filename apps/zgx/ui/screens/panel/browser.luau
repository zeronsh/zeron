-- A browser tab: a web page beside the chat, as Zeron's right panel has it
-- (crates/ui/src/browser: the address bar, back and forward, reload, open
-- in the default browser), and the way a page's parts become attachments:
-- annotation mode (the bar's cursor, ⌘⇧I) runs a picker in the page
-- (screens/panel/annotate_script); pointing at an element rings it, a click
-- picks it, a drag over the page picks a region, Shift adds more, Escape
-- leaves (as do the bar's cursor and ⌘⇧I again, another page, another tab,
-- and the 24th pick). Each pick is a chip in the composer at once
-- (ds/composer_cargo's annotation chip: the annotate glyph and the
-- element's words, its markup in the card under the pointer) and a mark on
-- the page with the same glyph until its chip leaves; the chip under the
-- pointer lights its mark. A session that picked something hands the
-- keyboard to the composer, after the chips, to say what to do with them
-- (the shipping app's flow: no note of its own over the page).
--
-- The page lives in the host by the tab's id (ui.web), so switching tabs,
-- sessions or a reload shows it as it was, history and all; its address
-- rides with the tab (screens/panel/state), so a tab made again from a
-- saved panel loads where it left off.
--
--   Browser.Browser { key, tab, project, session, open_url }
--   Browser.title(tab)        the page's title, else its host
--   Browser.close(tab)        the tab's page let go

local ds = require("@ds")
local Icon = require("@ds/icon")
local IconButton = require("@ds/icon_button").IconButton
local FileIcon = require("@ds/file_icons")
local Annotation = require("@ds/annotation")
local FadeText = require("@ds/fade_text")
local Cargo = require("@ds/composer_cargo")
local state = require("@app/screens/panel/state")
local api = require("@app/screens/panel/browser_api")
local Previews = require("@app/screens/panel/previews")
local data = require("@app/screens/panel/browser_data")
local Suggest = require("@app/screens/panel/browser_suggest")
local ContextMenu = require("@ds/context_menu")
local SCRIPT = require("@app/screens/panel/annotate_script")
local FIND_SCRIPT = require("@app/screens/panel/find_script")
-- The composer's cargo (screens/chat/shared), if the chat page is here (a
-- fixture mounts the panel alone).
local shared: any = nil
do
	local ok, found = pcall(function(): any
		return (require :: any)("@app/screens/chat/shared")
	end)
	if ok then
		shared = found
	end
end
-- The pages' cookie profile, one per account (browser_data.profile; nil
-- while the launch reads the Keychain), followed: the tab is made again
-- for another account.
local function profile_name(): string?
	return data.profile()
end

type Tab = state.Tab

local M = {}

-- Each tab's page title as it reports it, by tab id (for the strip).
local titles = ui.state("panel_browser_titles", {} :: { [string]: string })
-- Whether a tab is in annotation mode, by tab id.
local annotating = ui.state("panel_browser_annotating", {} :: { [string]: boolean })
-- Each tab's downloads, in the order they came: the file's name, how far
-- it is ("asking": the save dialog is up; "saving", "saved", "failed"),
-- where it went, why it failed.
export type Download = { id: string, name: string, phase: string, received: number?, total: number?, path: string?, why: string? }
local downloads = ui.state("panel_browser_downloads", {} :: { [string]: { Download } })
local download_seq = 0

-- Tabs whose page has marks on it (picks numbered there), by tab id.
local marked = ui.state("panel_browser_marked", {} :: { [string]: boolean })
-- Picks made in each tab's annotation session so far (the cap's count).
local session_picks = ui.state("panel_browser_session_picks", {} :: { [string]: number })
-- Tabs whose empty address field has had the keyboard once (not again on a
-- reload).
local focused_once = ui.state("panel_browser_focused", {} :: { [string]: boolean })
-- The pages, by tab id (the host keeps them; this is a lookup saver).
local webs: { [string]: any } = {}
-- Each tab's pick's nonce (what its page's picker posts with), by tab id:
-- kept, so a reload mid-pick goes on hearing the page.
local nonces = ui.state("panel_browser_nonces", {} :: { [string]: number })
-- Each tab's last pick's chip, by tab id.
local last_chips = ui.state("panel_browser_last_chip", {} :: { [string]: string })
-- The find bar's field at its widest, and the room its count takes in it
-- (the editor's: ds/code_editor FIND_FIELD and FIND_TALLY).
local FIND_FIELD, FIND_TALLY = 360, 96
-- Most picks one mode takes (a page's own script answers; it could
-- otherwise stream chips in without a click).
local MOST_PICKS = 24

-- The bar's own ⋯, drawn to sit with refresh and the cursor (a 16-point
-- square, ink across ~15 of its 24 units): the design system's dots draw
-- at 2 points beside their 1.5 strokes; these are of the strokes' weight.
local DOTS = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor"><circle cx="5.25" cy="12" r="1.75"/><circle cx="12" cy="12" r="1.75"/><circle cx="18.75" cy="12" r="1.75"/></svg>'
local GLYPH = require("@ds/icon_button").glyph_size("md")
local function glyph(svg: string): Node
	return ui.svg { src = `code:{svg}`, w = GLYPH, h = GLYPH, shrink = 0, aria_hidden = true, tint = "currentColor" }
end

-- The address field's corner: a small field's pill, half its 28 height
-- (the suggestions' bottom corners match it).
local FIELD_CORNER = 14

-- What a page last posted (a look, for devtools and tests).
local heard = ui.state("panel_browser_heard", nil :: any)

-- The browser tab showing now, for the app's ⌘⇧I.
-- The browser tab showing now, for the app's keys (the shipping app's
-- Browser context: ⌘L the address, ⌘T a new tab, ⌘⇧R reload, ⌘⇧I
-- annotate; nothing when no browser tab shows). ⌘[ and ⌘] are the
-- router's here; back and forward are the bar's and WebKit's swipe.
type Showing = {
	id: string,
	toggle: () -> (),
	focus_address: () -> (),
	reload: () -> (),
	new_tab: () -> (),
	-- Zoom in (1), out (-1) or back to the site's default (0).
	zoom: (by: number) -> (),
	-- The page (or its bar) has the keyboard: the zoom keys are the page's
	-- then, else whatever has it (an image's zoom in the lightbox).
	keyboard: () -> boolean,
}
local showing: Showing? = nil

-- Each on its key now, as Settings → Shortcuts binds it (screens/shell/keys
-- lists these four), else its own.
local shell_keys: any = (function()
	local ok, k = pcall(require, "@app/screens/shell/keys")
	return if ok and type(k) == "table" and k.key then k else nil
end)()
local function key_of(id: string, key: string): string | () -> string
	return if shell_keys then shell_keys.key(id) else key
end
local function command(id: string, title: string, menu: string, key: string, act: (Showing) -> ())
	ui.command {
		id = id,
		title = title,
		menu = menu,
		key = key_of(id, key) :: any,
		run = function(): boolean
			if showing then
				act(showing)
				return true
			end
			return false
		end,
	}
end
-- (A command's title is a string, read once as it's made.)
command("panel.annotate", ui.t("panel.browser.command.annotate"):get(), "View", "cmd+shift+i", function(s)
	s.toggle()
end)
command("panel.new_browser_tab", ui.t("panel.browser.command.new_tab"):get(), "File", "cmd+t", function(s)
	s.new_tab()
end)
-- (A palette open over the page takes ⌘L, as its own: New project's folder
-- path to edit.)
local CommandPalette = require("@ds/command_palette")
ui.command {
	id = "panel.focus_address",
	title = ui.t("panel.browser.command.open_location"):get(),
	menu = "View",
	key = key_of("panel.focus_address", "cmd+l") :: any,
	run = function(): boolean
		if CommandPalette.type_path() then
			return true
		elseif showing then
			showing.focus_address()
			return true
		end
		return false
	end,
}
command("panel.reload_page", ui.t("panel.browser.command.reload"):get(), "View", "cmd+shift+r", function(s)
	s.reload()
end)
-- ⌘+ ⌘− ⌘0, the page's while it has the keyboard (each site keeps its
-- own: browser_data), else the window's, as before. (Binding them takes
-- zgx's own window-zoom items out of the View menu: these stand in.)
local function zoom_window(by: number)
	local ok = pcall(function()
		if not ui.window.zoomable:peek() then
			return
		end
		local z = ui.window.zoom:peek()
		local next = if by == 0 then 1 else data.step_zoom(z, by)
		ui.window.zoom:set(next)
	end)
	return ok
end
local function zoom_command(id: string, title: string, key: string, by: number)
	ui.command {
		id = id,
		title = title,
		menu = "View",
		key = key,
		run = function(): boolean
			if showing and showing.keyboard() then
				showing.zoom(by)
				return true
			end
			zoom_window(by)
			return true
		end,
	}
end
zoom_command("panel.zoom_in", ui.t("panel.browser.command.zoom_in"):get(), "cmd+=", 1)
zoom_command("panel.zoom_out", ui.t("panel.browser.command.zoom_out"):get(), "cmd+-", -1)
zoom_command("panel.zoom_reset", ui.t("panel.browser.command.zoom_reset"):get(), "cmd+0", 0)

-- Each tab's page icon (a file the host fetched: src/favicons.rs), by tab
-- id, for the strip.
local icons = ui.state("panel_browser_icons", {} :: { [string]: string })
-- The icon a page names, else the site's /favicon.ico.
local ICON_JS = "(() => { const link = document.querySelector('link[rel~=\"icon\"]'); return link ? link.href : new URL('/favicon.ico', location.href).href; })()"

function M.icon(tab: Tab): string?
	return icons:get()[tab.id]
end

-- Which tabs have their find bar open (the app's ⌘F while the tab has the
-- keyboard, and the bar's own field, count, arrows and close).
local finding = ui.state("panel_browser_finding", {} :: { [string]: boolean })
-- A bump that hands the open bar's field the keyboard (⌘F again).
local find_focus = ui.state("panel_browser_find_focus", 0)

-- Whether the tab has the keyboard of its own (its page, or its find bar):
-- the panel's own focus tracking doesn't see into a native page.
function M.has_keyboard(tab: Tab): boolean
	local web = webs[tab.id]
	local ok, focused = pcall(function(): any
		return if web then web.focused else false
	end)
	return (ok and focused == true) or finding:peek()[tab.id] == true
end

function M.open_find(tab: Tab)
	if not finding:peek()[tab.id] then
		finding:set_key(tab.id, true)
	end
	find_focus:add(1)
end

-- Settings → Browser: its History page ("history"), else Browsing. (The
-- shell's, when it's here: a fixture mounts the panel alone.)
function M.open_settings(page: string?)
	pcall(function()
		local route: any = (require :: any)("@app/screens/shell/route")
		route.settings(if page == "history" then "browser_history" else "browser")
	end)
end

local function has_page(web: any): boolean
	local u = web.url
	return type(u) == "string" and u ~= "" and u ~= "about:blank"
end

function M.title(tab: Tab): string?
	local t = titles:get()[tab.id]
	if t and t ~= "" then
		return t
	end
	return api.host_of(tab.url)
end

function M.close(tab: Tab)
	local web = webs[tab.id] or api.web(`panel-browser-{tab.id}`)
	webs[tab.id] = nil
	pcall(function()
		web:close()
	end)
	titles:set_key(tab.id, nil)
	icons:set_key(tab.id, nil)
	finding:set_key(tab.id, nil)
	annotating:set_key(tab.id, nil)
	nonces:set_key(tab.id, nil)
	last_chips:set_key(tab.id, nil)
	session_picks:set_key(tab.id, nil)
	marked:set_key(tab.id, nil)
end

-- A prop's value now: a function's answer, a state's value (tracked), or
-- the value itself (as screens/panel reads them).
local function read(v: any): any
	local t = type(v)
	if t == "function" then
		return v()
	elseif t == "table" or t == "userdata" then
		local ok, getter = pcall(function()
			return (v :: any).get
		end)
		if ok and type(getter) == "function" then
			return (v :: any):get()
		end
	end
	return v
end

-- A JavaScript string literal.
local function js_string(v: string): string
	return `"{string.gsub(string.gsub(string.gsub(v, "\\", "\\\\"), '"', '\\"'), "\n", "\\n")}"`
end

-- A JavaScript literal for a flat table of strings, numbers and booleans.
local function js(value: { [string]: any }): string
	local parts = {}
	for k, v in value do
		local lit: string
		if type(v) == "string" then
			lit = `"{string.gsub(string.gsub(v, "\\", "\\\\"), '"', '\\"')}"`
		elseif type(v) == "boolean" then
			lit = tostring(v)
		elseif type(v) == "number" then
			lit = tostring(v)
		else
			continue
		end
		table.insert(parts, `{k}:{lit}`)
	end
	return `\{{table.concat(parts, ",")}}`
end

-- The design system's look, for the page's overlay: a colour token's value
-- when it is a plain colour (the script keeps its own where one isn't).
local function page_theme(): { [string]: any }
	local function colour(name: string): string?
		local v = ui.theme.get(name)
		return if type(v) == "string" and string.sub(v, 1, 1) == "#" then v else nil
	end
	local kind = ui.theme.kind()
	local font = ui.theme.get("font.sans")
	return {
		accent = colour("accent"),
		on_accent = colour("accent.text"),
		surface = colour("popover.bg"),
		border = colour("border"),
		text = colour("text.primary"),
		font = if type(font) == "string" then font else nil,
		dark = kind == "dark" or kind == "high-contrast",
	}
end

-- The picker's look and words: the design system's colours, and its hint
-- in the locale shown.
local function annotate_theme(): { [string]: any }
	local t = page_theme()
	t.hint = ui.t("panel.browser.annotate_page.hint"):get()
	t.hint_keys = ui.t("panel.browser.annotate_page.keys"):get()
	t.selected = ui.t("panel.browser.annotate_page.selected", { n = "{n}" }):get()
	t.region = ui.t("panel.browser.annotate_page.region"):get()
	return t
end

export type Props = { key: string, tab: string, project: any?, session: any?, open_url: ((url: string) -> ())?, profile: string?, [any]: any }

-- Which cookie profile each tab's page was made with, by tab id (kept over
-- a reload, as the host keeps the page): a page made for another account
-- is let go and made again.
local made_for = ui.state("panel_browser_profile", {} :: { [string]: string })

-- The page for a tab, made with the profile it's for (and the browser's
-- settings: whether it may be inspected).
local function page_for(id: string, profile: string): any
	local was = made_for:peek()[id]
	local web = webs[id]
	if was and was ~= profile then
		local old = web or api.web(`panel-browser-{id}`)
		pcall(function()
			old:close()
		end)
		web = nil
		webs[id] = nil
	end
	if not web then
		web = api.web(`panel-browser-{id}`, {
			profile = profile,
			messages = true,
			new_windows = true,
			inspectable = data.inspectable:peek(),
		})
		webs[id] = web
	end
	if was ~= profile then
		made_for:set_key(id, profile)
	end
	return web
end

-- What a page that turned out to be a file says (WebKit stops a load it
-- can't show: a download, a zip), as the shipping app's "This file can't
-- be previewed here".
local function is_file_error(e: any): boolean
	if type(e) ~= "string" then
		return false
	end
	local l = string.lower(e)
	return string.find(l, "frame load interrupted", 1, true) ~= nil
		or string.find(l, "cannot show content", 1, true) ~= nil
		or string.find(l, "can’t show", 1, true) ~= nil
end

local Page: (props: any) -> Node
-- The tab, once it's known whose sign-ins its pages keep (not while the
-- launch reads the Keychain), made again for another account.
M.Browser = ui.component(function(props: Props): Node
	return ui.presence(profile_name, function(profile: string): Node
		local with = table.clone(props) :: any
		with.profile = profile
		return Page(with)
	end, { w_full = true, grow = 1, min_h = 0 })
end)

Page = ui.component(function(props: Props): Node
	local profile: string = props.profile
	local key, id = props.key, props.tab
	local function tab(): Tab?
		state.of(key)
		return state.find(key, id)
	end
	local t0 = tab()
	local web = page_for(id, profile)
	-- What the field says: the page's address, or what's being typed.
	local address = ui.use_state(if t0 and t0.url then t0.url else "")
	-- The address the field last showed of the page's, so typing over it
	-- is told from the page moving on.
	local last_url: string = address:peek()
	-- An address that couldn't be taken, and why.
	local problem = ui.use_state(nil :: string?)
	-- Annotation mode, as the bar's toggle shows it.
	local mode = ui.use_state(annotating:peek()[id] == true)

	-- Made with an address and no page yet (a saved tab, a link opened):
	-- the page loads. (A reload finds the page where it was: nothing moves.)
	-- A new tab with a home page set opens on it (once: the start page is
	-- the way back to the project's previews when it's left).
	if t0 and t0.url and not has_page(web) then
		local url = t0.url
		ui.after(0, function()
			web:load(url)
		end)
	end

	-- What was typed and taken: the field keeps it until the page answers
	-- (set to the address at once, the field would commit again on blur,
	-- as changed). Words that don't read as an address search them, on the
	-- engine Settings → Browser chooses.
	local submitted: string? = nil
	local function navigate(text: string)
		local url, err = api.resolve(text, data.engine_url())
		if not url then
			problem:set(err)
			return
		end
		problem:set(nil)
		submitted = text
		last_url = url
		state.set_url(key, id, url)
		web:load(url)
	end

	-- Each site's zoom (⌘+ ⌘− ⌘0, the bar's percentage, Settings' default),
	-- put on the page as it reaches the site.
	local function site(): string?
		local u = web.url
		return if type(u) == "string" then api.host_of(u) else nil
	end
	local function zoom_now(): number
		return data.zoom_of(site())
	end
	-- (A page starts at 100%: only a zoom off it goes on.)
	local shown_zoom: number = 1
	ui.effect(function()
		local z = zoom_now()
		if z ~= shown_zoom then
			shown_zoom = z
			pcall(function()
				web:zoom(z)
			end)
		end
	end)
	local function zoom(by: number)
		local host = site()
		if not host then
			return
		end
		local z = if by == 0 then data.default_zoom:peek() else data.step_zoom(data.zoom_of(host), by)
		data.set_zoom(host, z)
	end

	-- History: each page reached, kept for its account (and its title, once
	-- the page says it).
	local function record(url: string?)
		if type(url) == "string" and has_page(web) and not web.error then
			local t = web.title
			data.visit(profile, url, if type(t) == "string" then t else nil)
		end
	end
	ui.effect(function()
		local t = web.title
		local u = web.url
		if type(t) == "string" and t ~= "" and type(u) == "string" then
			-- (Peeks only: the effect follows the page alone.)
			data.retitle(profile, u, t)
		end
	end)

	-- The address bar's suggestions (made with the bar, below): told when
	-- the field's words are the page's, not typed.
	local suggestions: any = nil

	-- The page moving on: the field follows (unless something else is being
	-- typed), the tab keeps the address.
	ui.effect(function()
		local u = web.url
		if type(u) ~= "string" or u == "" or u == "about:blank" then
			return
		end
		local typed = address:peek()
		if typed == last_url or typed == submitted or typed == "" then
			if suggestions then
				suggestions.quiet(u)
			end
			address:set(u)
		end
		submitted = nil
		last_url = u
		state.set_url(key, id, u)
		-- (On the address, not the load's end: a page's own moves, which
		-- finish nothing, are visits too. The title follows as it comes.
		-- After, so the page's title and error it reads aren't followed here.)
		ui.after(0, function()
			record(u)
		end)
	end)
	ui.effect(function()
		local t = web.title
		local now = titles:peek()[id]
		local next_title = if type(t) == "string" and t ~= "" then t else nil
		if now ~= next_title then
			titles:set_key(id, next_title)
		end
	end)

	-- Annotation mode: the picker into the page, and out again.
	local function stop_annotating()
		web:run("window.__zeronAnnotator && window.__zeronAnnotator.stop();")
		if annotating:peek()[id] then
			annotating:set_key(id, nil)
		end
		if mode:peek() then
			mode:set(false)
		end
	end
	local function start_annotating()
		if not has_page(web) or web.error then
			if mode:peek() then
				mode:set(false)
			end
			return
		end
		local nonce = math.random(1, 2147483647)
		nonces:set_key(id, nonce)
		session_picks:set_key(id, nil)
		web:run(`{SCRIPT}\n;window.__zeronAnnotator.start({nonce}, {js(annotate_theme())});`)
		annotating:set_key(id, true)
		if not mode:peek() then
			mode:set(true)
		end
		web:focus()
	end
	local function toggle_annotating()
		if mode:peek() then
			stop_annotating()
		else
			start_annotating()
		end
	end
	local address_ref = ui.ref()
	showing = {
		id = id,
		toggle = toggle_annotating,
		focus_address = function()
			local el = address_ref:peek()
			if el then
				el:focus()
				el:select_all()
			end
		end,
		reload = function()
			if has_page(web) then
				web:reload()
			end
		end,
		new_tab = function()
			state.open(key, "browser")
		end,
		zoom = zoom,
		keyboard = function(): boolean
			local ok, focused = pcall(function(): any
				return web.focused
			end)
			return ok and focused == true
		end,
	}
	ui.on_cleanup(function()
		if showing and showing.id == id then
			showing = nil
		end
		-- (Hidden for another tab, or closed: no session goes on unseen.)
		if annotating:peek()[id] then
			stop_annotating()
		end
	end)
	-- Another tab of the panel showing (or this one closing): annotating
	-- stops, as the shipping app's does when its browser hides.
	ui.effect(function()
		local s = state.of(key)
		if s.active ~= id and annotating:peek()[id] then
			stop_annotating()
		end
	end)

	local on_state: (ended: string) -> ()
	local take_download: (d: any) -> ()
	-- A pick, as the page posts it: a chip, and the mark numbered as the
	-- chip is (the number it rides the prompt by; the mark shows the
	-- annotate glyph, as the chip does).
	local function on_pick(pick: { [string]: any })
		local made = session_picks:peek()[id] or 0
		if not shared or made >= MOST_PICKS then
			return
		end
		made += 1
		session_picks:set_key(id, made)
		local marker = if type(pick.marker) == "number" then pick.marker else nil
		-- (Numbered on the page before the chip is there: the marks follow
		-- the chips, and an unnumbered one would be let go.)
		if marker then
			web:run(`window.__zeronAnnotator && window.__zeronAnnotator.label({marker}, {shared.next_annotation()});`)
			marked:set_key(id, true)
		end
		local chip_id = shared.attach_annotation(pick, { tab = id, marker = marker })
		last_chips:set_key(id, chip_id)
		-- The most one session takes: it ends there, as the shipping app's
		-- (the page's picker stops; its "cancelled" finds the mode ended).
		if made >= MOST_PICKS then
			web:run("window.__zeronAnnotator && window.__zeronAnnotator.stop();")
			on_state("done")
		end
	end
	-- The session over (a pick without Shift, Escape, the cap): the mode
	-- off and, if it picked anything, the keyboard to the composer, after
	-- the chips, to write about them (the shipping app's; no note of its
	-- own over the page).
	on_state = function(_ended: string)
		annotating:set_key(id, nil)
		-- (Its nonce forgotten: what the page might still post is no pick.)
		nonces:set_key(id, nil)
		if mode:peek() then
			mode:set(false)
		end
		local made = session_picks:peek()[id] or 0
		session_picks:set_key(id, nil)
		last_chips:set_key(id, nil)
		if made > 0 and shared then
			web:blur()
			shared.focus:add(1)
		end
	end

	-- The marks follow the chips: one whose chip left the draft goes, and
	-- the rest take the chips' numbers as they close up (shared.renumber).
	-- The page is told each kept mark's number then and now; what it was
	-- last told is kept by chip, here.
	local told: { [string]: number } = {}
	local synced: string? = nil
	ui.effect(function()
		if not shared then
			return
		end
		local pairs_: { string } = {}
		local now_told: { [string]: number } = {}
		for _, item in shared.cargo:get() do
			if item.kind == "annotation" and item.tab == id and item.index then
				local was = told[item.id] or item.index
				table.insert(pairs_, `[{was},{item.index}]`)
				now_told[item.id] = item.index
			end
		end
		local now = table.concat(pairs_, ",")
		-- (The first time too, on a page with marks: chips sent or let go
		-- while the tab wasn't showing take their marks with them as it
		-- shows.)
		if now ~= synced and (synced ~= nil or marked:peek()[id]) then
			web:run(`window.__zeronAnnotator && window.__zeronAnnotator.sync([{now}]);`)
			if now == "" and marked:peek()[id] then
				marked:set_key(id, nil)
			end
		end
		told = now_told
		-- (Said as now: the next change is told from these.)
		local same: { string } = {}
		for _, item in shared.cargo:peek() do
			if item.kind == "annotation" and item.tab == id and item.index then
				table.insert(same, `[{item.index},{item.index}]`)
			end
		end
		synced = table.concat(same, ",")
	end)

	-- The chip under the pointer in the composer: its mark firms on the
	-- page (and comes into view), the others as they were.
	local lit: number? = nil
	ui.effect(function()
		local over = Cargo.hovered:get()
		local n: number? = nil
		if over and shared then
			for _, item in shared.cargo:peek() do
				if item.id == over and item.kind == "annotation" and item.tab == id then
					n = item.index
				end
			end
		end
		if n ~= lit then
			lit = n
			web:run(`window.__zeronAnnotator && window.__zeronAnnotator.focus({n or 0});`)
		end
	end)

	-- The page loaded again (reloaded, or come back to): the picks' marks
	-- are put back, by their selectors (else where their boxes were).
	-- The page's icon: what it names (else /favicon.ico), fetched by the
	-- host to a file the strip shows. A page without one keeps the globe.
	local function find_icon()
		web:eval(ICON_JS, function(value: any, err: string?)
			if err or type(value) ~= "string" or not api.allowed(value) then
				if icons:peek()[id] then
					icons:set_key(id, nil)
				end
				return
			end
			ui.call("favicon.fetch", { url = value }):after(function(path: any)
				local now = if type(path) == "string" and path ~= "" then path else nil
				if icons:peek()[id] ~= now then
					icons:set_key(id, now)
				end
				-- (History shows it beside the visit.)
				local u = web.url
				if now and type(u) == "string" then
					data.set_icon(profile, u, now)
				end
			end, function() end)
		end)
	end

	-- Two addresses of one page (its fragment aside).
	local function same_page(a: string?, b: any): boolean
		if type(a) ~= "string" or type(b) ~= "string" then
			return false
		end
		return (string.match(a, "^[^#]*") or a) == (string.match(b, "^[^#]*") or b)
	end
	local function restore_marks()
		if not shared then
			return
		end
		local items = {}
		for _, item in shared.cargo:peek() do
			local a = item.annotation
			-- (Only this page's, by their selectors: a box was where it was
			-- in the viewport then, not where it is in the page now.)
			if item.kind == "annotation" and item.tab == id and item.index and a and same_page(a.url, web.url) then
				table.insert(items, `\{n:{item.index},selector:{js_string(a.selector)},rect:[]}`)
			end
		end
		if #items == 0 then
			return
		end
		web:run(`{SCRIPT}\n;window.__zeronAnnotator.restore([{table.concat(items, ",")}]);`)
		marked:set_key(id, true)
	end

	local function loading(): boolean
		return web.loading == true
	end
	local function can_leave(): boolean
		return has_page(web)
	end

	-- The site's zoom is off the default (its percentage shows in the field).
	local function zoomed(): boolean
		return has_page(web) and math.abs(zoom_now() - data.default_zoom:get()) > 0.001
	end

	-- The page's other actions, under ⋯: zoom, find, the address copied,
	-- the inspector, the browser's settings.
	local menu_open = ui.use_state(false)
	local more_menu = ContextMenu {
		open = menu_open,
		label = ui.t("panel.browser.more"),
		shrink = 0,
		items = function(): { any }
			local page = can_leave()
			-- Greyed with no page, without a reason under each: the empty tab says it.
			local no_page = not page
			local entries: { any } = {
				{ heading = ui.t("panel.browser.menu.zoom", { level = data.percent(zoom_now()) }):get() },
				{ label = ui.t("panel.browser.command.zoom_in"):get(), icon = "plus", shortcut = "cmd+=", disabled = no_page, keep_open = true, on_select = function()
					zoom(1)
				end },
				{ label = ui.t("panel.browser.command.zoom_out"):get(), icon = "minus", shortcut = "cmd+-", disabled = no_page, keep_open = true, on_select = function()
					zoom(-1)
				end },
				{ label = ui.t("panel.browser.command.zoom_reset"):get(), shortcut = "cmd+0", disabled = no_page, on_select = function()
					zoom(0)
				end },
				{ separator = true },
				{ label = ui.t("panel.browser.find.label"):get(), icon = "search", shortcut = "cmd+f", disabled = no_page, on_select = function()
					local t = tab()
					if t then
						M.open_find(t)
					end
				end },
				{ label = ui.t("panel.browser.menu.copy_link"):get(), icon = "copy", disabled = no_page, on_select = function()
					local u = web.url
					if type(u) == "string" and u ~= "" then
						ui.clipboard.write(u)
					end
				end },
				{ label = ui.t("panel.browser.open_in_browser"):get(), icon = "arrow-up-right", disabled = no_page, on_select = function()
					local u = web.url
					if type(u) == "string" then
						api.open_externally(u)
					end
				end },
			}
			if data.inspectable:peek() then
				table.insert(entries, { label = ui.t("panel.browser.menu.inspect"):get(), icon = "file-code", disabled = no_page, on_select = function()
					web:devtools()
				end })
			end
			table.insert(entries, { separator = true })
			table.insert(entries, { label = ui.t("panel.browser.menu.history"):get(), icon = "clock-circle", on_select = function()
				M.open_settings("history")
			end })
			table.insert(entries, { label = ui.t("panel.browser.menu.settings"):get(), icon = "settings-minimalistic", on_select = function()
				M.open_settings(nil)
			end })
			return entries
		end,
		IconButton {
			label = ui.t("panel.browser.more"),
			tooltip_side = "bottom",
			tone = "quiet",
			glyph(DOTS),
			on_click = function()
				menu_open:set(not menu_open:peek())
			end,
		},
	}

	-- The address bar's suggestions (browser_suggest): where Enter could go
	-- as you type, the likeliest site finished in the field. The project's
	-- dev servers are the start page's (`services`, made further down).
	local preview_list: () -> { any } = function(): { any }
		return {}
	end
	local field_box = ui.ref()
	local root_ref = ui.ref()
	suggestions = Suggest.use {
		value = address,
		input = address_ref,
		profile = profile,
		id = id,
		previews = function(): { any }
			return preview_list()
		end,
		choose = function(url: string, _shown: string)
			address:set(url)
			navigate(url)
		end,
	}

	-- The address field, in a box the suggestions hang from (as wide).
	local field_row = ui.row {
		ref = field_box,
		grow = 1,
		min_w = 0,
		items = "center",
		ds.TextField {
			value = address,
			input_ref = address_ref,
			name = ui.t("panel.browser.address") :: any,
			size = "sm",
			icon = "global",
			placeholder = ui.t("panel.browser.address_placeholder") :: any,
			rounded = "$radius.full",
			inputmode = "url",
			select_on_focus = true,
			autofocus = not (t0 and t0.url) and not focused_once:peek()[id],
			grow = 1,
			role = "combobox",
			expanded = suggestions.open,
			-- (Open, the field and its suggestions are one shape.)
			joined = suggestions.open,
			active_descendant = suggestions.descendant,
			on_focus_change = suggestions.on_focus_change,
			trailing = ui.row {
				items = "center",
				gap = "$space.1",
				-- The site's zoom, while it's off the default: its percentage,
				-- which puts it back.
				ui.show(zoomed, ui.button {
					id = `panel-browser-zoom-{id}`,
					label = function(): string
						return ui.t("panel.browser.zoom.reset_to", { level = data.percent(zoom_now()) }):get()
					end,
					-- (A string, read as it's built, as the bar's other tooltips.)
					-- zgx-lint: allow(untracked-get)
					tooltip = ui.t("panel.browser.zoom.reset"):get(),
					h = 20,
					px = "$space.2",
					rounded = "$radius.full",
					bg = "$zeron.secondary.bg",
					hover = { bg = "$zeron.secondary.hover" },
					active = { bg = "$zeron.secondary.active" },
					cursor = "pointer",
					on_click = function()
						zoom(0)
					end,
					ui.text {
						function(): string
							return data.percent(zoom_now())
						end,
						literal = true,
						tabular = true,
						text_size = "$font.size.xs",
						weight = "$font.weight.medium",
						color = "$text.secondary",
					},
				}),
				ui.show(loading, ds.ProgressRing {
					value = function(): number?
						local p = web.progress
						return if type(p) == "number" and p > 0 then p else nil
					end,
					size = 12,
					tone = "neutral",
					-- (A string, read as it's built: the ring says it with its
					-- progress.)
					-- zgx-lint: allow(untracked-get)
					label = ui.t("common.loading"):get(),
				}),
			},
			-- (Enter goes, here: zgx's commit comes on leaving the field too,
			-- once the page's address has replaced what was typed, and that
			-- would load the page again.)
			on_key = function(e: any): boolean?
				-- (The list's keys first, while it's out.)
				if suggestions.keys(e) then
					return true
				end
				if e.key == "Enter" then
					navigate(address:peek())
					return true
				end
				if e.key == "Escape" then
					-- (Back to the page's address, as a browser's.)
					suggestions.quiet(last_url)
					address:set(last_url)
					problem:set(nil)
					return true
				end
				return nil
			end,
		},
	}

	-- The bar: back, forward and reload (stop while loading), the address,
	-- annotate, and the rest under ⋯ (open in the default browser among it).
	-- (Three Tab stops: the history pair, the address, the page's controls;
	-- each pair or trio a toolbar of its own, so the arrows move within it
	-- and the field keeps its own arrows for the caret.)
	local bar = ui.row {
		id = `panel-browser-bar-{id}`,
		label = ui.t("panel.browser.bar"),
		w_full = true,
		h = 40,
		shrink = 0,
		px = "$space.2",
		gap = "$space.1",
		items = "center",
		ui.row {
		role = "toolbar",
		label = ui.t("panel.browser.navigation"),
		shrink = 0,
		gap = "$space.1",
		items = "center",
		IconButton {
			icon = "arrow-left",
			label = ui.t("common.back"),
			tooltip_side = "bottom",
			tone = "quiet",
			disabled = function(): boolean
				return not web.can_go_back
			end,
			on_click = function()
				web:back()
			end,
		},
		IconButton {
			icon = "arrow-right",
			label = ui.t("panel.browser.forward"),
			tooltip_side = "bottom",
			tone = "quiet",
			disabled = function(): boolean
				return not web.can_go_forward
			end,
			on_click = function()
				web:forward()
			end,
		},
		-- Reload beside them (Stop while it loads), as a browser has it.
		ui.show(loading, IconButton {
			icon = "stop",
			label = ui.t("panel.browser.stop"),
			tooltip_side = "bottom",
			tone = "quiet",
			on_click = function()
				web:stop()
			end,
		}, IconButton {
			icon = "refresh",
			label = ui.t("common.reload"),
			tooltip_side = "bottom",
			tone = "quiet",
			disabled = function(): boolean
				return not can_leave()
			end,
			on_click = function()
				web:reload()
			end,
		}),
		},
		field_row,
		ui.row {
		role = "toolbar",
		label = ui.t("panel.browser.page_controls"),
		shrink = 0,
		gap = "$space.1",
		items = "center",
		ds.ToggleButton {
			id = `panel-browser-annotate-{id}`,
			on = mode,
			icon = "cursor",
			-- (Quiet, as the bar's other glyphs: on, it lights all the same.)
			tone = "quiet",
			label = ui.t("panel.browser.annotate"),
			-- zgx-lint: allow(untracked-get)
			tooltip = ui.t("panel.browser.annotate_tooltip", { key = ui.shortcut_text(if shell_keys then shell_keys.combo("panel.annotate") else "cmd+shift+i") }):get(),
			tooltip_side = "bottom",
			disabled = function(): boolean
				return not can_leave() or web.error ~= nil
			end,
			on_change = function(on: boolean)
				if on then
					start_annotating()
				else
					stop_annotating()
				end
			end,
		},
		more_menu,
		},
	}
	if not (t0 and t0.url) and not focused_once:peek()[id] then
		focused_once:set_key(id, true)
	end

	-- An address that couldn't be taken: why, under the bar.
	local problem_line = ui.show(function(): boolean
		return problem:get() ~= nil
	end, ui.row {
		px = "$space.3",
		pb = "$space.2",
		gap = "$space.1",
		items = "center",
		color = "$zeron.tone.danger.ink",
		live = "polite",
		ui.div { y = -0.5, aria_hidden = true, Icon { name = "danger-triangle", size = 12 } },
		ui.text {
			function(): string
				local why = problem:get()
				return if why then ui.t(why):get() else ""
			end,
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			wrap = true,
		},
	})

	-- Find in page: the words, lit in the page by its script (find_script),
	-- the current match in the accent and in view; Enter steps on, Shift
	-- Enter back, Escape closes (the page takes the keyboard back). The bar
	-- is a row of the tab under the address, as the editor's is, so it never
	-- covers a match.
	local query = ui.use_state("")
	local found = ui.use_state({ count = 0, current = 0 } :: { count: number, current: number })
	local find_input = ui.ref()
	local function find_open(): boolean
		return finding:get()[id] == true
	end
	local function find_theme(): string
		local t = page_theme()
		return js({ wash = "rgba(146,119,176,.35)", accent = t.accent or "#9277b0", ink = t.on_accent or "#ffffff" })
	end
	local function took(value: any)
		if type(value) == "table" and type(value.count) == "number" then
			found:set({ count = value.count, current = if type(value.current) == "number" then value.current else 0 })
		else
			found:set({ count = 0, current = 0 })
		end
	end
	local function search(text: string)
		if not has_page(web) then
			return
		end
		web:eval(`{FIND_SCRIPT}\n;window.__zeronFind.find({js_string(text)}, {find_theme()})`, function(value: any)
			took(value)
		end)
	end
	local function step(by: number)
		if not has_page(web) or found:peek().count == 0 then
			return
		end
		web:eval(`{FIND_SCRIPT}\n;window.__zeronFind.step({by})`, function(value: any)
			took(value)
		end)
	end
	local function close_find()
		if finding:peek()[id] then
			finding:set_key(id, nil)
		end
		if has_page(web) then
			web:run("window.__zeronFind && window.__zeronFind.clear();")
			web:focus()
		end
		found:set({ count = 0, current = 0 })
	end
	-- The words searched as they're typed (a beat after, as the editor's).
	local searched: string? = nil
	ui.effect(function()
		local text = query:get()
		if not find_open() then
			return
		end
		if text == searched then
			return
		end
		searched = text
		search(text)
	end)
	-- ⌘F again, with the bar open: its field, selected.
	local focus_seen = find_focus:peek()
	ui.effect(function()
		local n = find_focus:get()
		if n == focus_seen then
			return
		end
		focus_seen = n
		if finding:peek()[id] then
			local el = find_input:peek()
			if el then
				el:focus()
				el:select_all()
			end
		end
	end)
	local function tally(): string
		if query:get() == "" then
			return ""
		end
		local f = found:get()
		if f.count == 0 then
			return ui.t("panel.browser.find.no_results"):get()
		end
		return if f.current > 0
			then ui.t("panel.browser.find.tally", { current = tostring(f.current), count = tostring(f.count) }):get()
			else tostring(f.count)
	end
	local function none(): boolean
		return found:get().count == 0
	end
	-- Downloads: what a page hands over (a download link, a file it can't
	-- show) is saved where the person says, the save dialog suggesting its
	-- name (zgx saves only to a file the person picked: nothing lands in a
	-- folder unasked). Each is a line under the bar while it saves and
	-- after: its file, how far, then Show in Finder (or why it failed),
	-- and a × to put it away.
	local function list(): { Download }
		return downloads:get()[id] or {}
	end
	local function change(did: string, edit: ((Download) -> Download)?)
		local now = downloads:peek()[id] or {}
		local next: { Download } = {}
		for _, one in now do
			if one.id ~= did then
				table.insert(next, one)
			elseif edit then
				table.insert(next, edit(table.clone(one)))
			end
		end
		downloads:set_key(id, if #next > 0 then next else nil)
	end
	take_download = function(d: any)
		download_seq += 1
		local did = `{os.time()}-{download_seq}`
		local name = if type(d.name) == "string" and d.name ~= "" then d.name else ui.t("panel.browser.download.untitled"):get()
		local now = table.clone(downloads:peek()[id] or {})
		table.insert(now, { id = did, name = name, phase = "asking" })
		downloads:set_key(id, now)
		local ok, promise = pcall(function(): any
			return d:ask({
				on_progress = function(received: number, total: number?)
					change(did, function(one: Download): Download
						one.phase = "saving"
						one.received, one.total = received, total
						return one
					end)
				end,
				on_finish = function(done: boolean, err: string?)
					change(did, function(one: Download): Download
						one.phase = if done then "saved" else "failed"
						one.why = if done then nil else err
						return one
					end)
				end,
			})
		end)
		if not ok or not promise then
			change(did, nil)
			return
		end
		promise:after(function(file: any)
			if type(file) ~= "table" then
				-- (The dialog cancelled: nothing saved, nothing to show.)
				change(did, nil)
				return
			end
			change(did, function(one: Download): Download
				if one.phase == "asking" then
					one.phase = "saving"
				end
				one.path = file.path
				if type(file.name) == "string" and file.name ~= "" then
					one.name = file.name
				end
				return one
			end)
		end, function()
			change(did, nil)
		end)
	end
	local function size_of(bytes: number): string
		if bytes >= 1024 * 1024 then
			return string.format("%.1f MB", bytes / (1024 * 1024))
		end
		return string.format("%d KB", math.max(1, math.floor(bytes / 1024 + 0.5)))
	end
	local function says(one: Download): string
		if one.phase == "asking" then
			return ui.t("panel.browser.download.asking"):get()
		elseif one.phase == "saving" then
			if one.total and one.total > 0 then
				return ui.t("panel.browser.download.percent", { n = math.floor((one.received or 0) * 100 / one.total) }):get()
			elseif (one.received or 0) > 0 then
				return size_of(one.received :: number)
			end
			return ui.t("panel.browser.download.saving"):get()
		elseif one.phase == "saved" then
			return ui.t("panel.browser.download.saved"):get()
		end
		return if one.why and one.why ~= ""
			then ui.t("panel.browser.download.failed_why", { why = one.why }):get()
			else ui.t("panel.browser.download.failed"):get()
	end
	local download_rows = ui.each(ui.computed(list), function(one: Download, _i: number, now: State<any>): Node
		local function it(): Download
			return now:get() :: Download
		end
		local function phase(): string
			return it().phase
		end
		return ui.row {
			id = `panel-browser-download-{one.id}`,
			class = "browser-download",
			label = function(): string
				return `{it().name}, {says(it())}`
			end,
			w_full = true,
			h = 32,
			gap = "$space.2",
			items = "center",
			ui.div { w = "$zeron.icon_button.sm", shrink = 0, items = "center", justify = "center", aria_hidden = true, FileIcon { name = one.name, size = 16 } },
			FadeText { function(): string
				return it().name
			end, literal = true, shrink = 1, text_size = "$font.size.sm", color = "$text.primary" },
			ui.text {
				function(): string
					return says(it())
				end,
				literal = true,
				shrink = 0,
				tabular = true,
				text_size = "$font.size.sm",
				color = function(): string
					return if phase() == "failed" then "$zeron.tone.danger.ink" else "$text.tertiary"
				end,
			},
			ui.div { grow = 1 },
			ui.show(function(): boolean
				return phase() == "saving"
			end, ds.ProgressRing {
				value = function(): number?
					local x = it()
					return if x.total and x.total > 0 then (x.received or 0) / x.total else nil
				end,
				size = 14,
				tone = "neutral",
				label = one.name,
			}),
			ui.show(function(): boolean
				return phase() == "saved" and it().path ~= nil
			end, ds.Button {
				tone = "ghost",
				size = "sm",
				on_click = function()
					local path = it().path
					if path then
						pcall(ui.call, "image.reveal", { path = path })
					end
				end,
				ui.text { ui.t("panel.browser.download.show") },
			}),
			ui.show(function(): boolean
				return phase() ~= "asking" and phase() ~= "saving"
			end, IconButton {
				icon = "close",
				size = "sm",
				tone = "quiet",
				label = ui.t("panel.browser.download.dismiss", { name = one.name }),
				on_click = function()
					change(one.id, nil)
				end,
			}),
		}
	end, {
		key = function(one: Download): string
			return one.id
		end,
		direction = "column",
		w_full = true,
	})
	local downloads_bar = ui.show(function(): boolean
		return #list() > 0
	end, ui.column {
		id = `panel-browser-downloads-{id}`,
		role = "group",
		label = ui.t("panel.browser.download.label"),
		w_full = true,
		shrink = 0,
		px = "$space.2",
		py = "$space.1",
		border_sides = { 0, 0, 1, 0 },
		border_color = "$border.subtle",
		download_rows,
	})

	local find_bar = ui.show(find_open, ui.row {
		id = `panel-browser-find-{id}`,
		role = "toolbar",
		label = ui.t("panel.browser.find.label"),
		-- (The editor's find bar's metrics, to the point: ds/code_editor
		-- `find_bar`: the row's inset and gap, the glyph's slot, the field's
		-- size and widest, the tally's room in it, the small glyph buttons.)
		w_full = true,
		shrink = 0,
		p = "$space.1",
		gap = "$space.1",
		items = "center",
		border_sides = { 0, 0, 1, 0 },
		border_color = "$border.subtle",
		ui.div { w = "$zeron.icon_button.sm", shrink = 0, items = "center", justify = "center", color = "$text.tertiary", Icon { name = "search", size = 14 } },
		ds.TextField {
			input_ref = find_input,
			value = query,
			name = ui.t("panel.browser.find.label") :: any,
			placeholder = ui.t("panel.browser.find.placeholder") :: any,
			size = "sm",
			grow = 1,
			max_w = FIND_FIELD,
			select_on_focus = true,
			autofocus = true,
			trailing = ui.row {
				pr = "$space.1",
				FadeText {
					tally,
					literal = true,
					max_w = FIND_TALLY,
					tabular = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			},
			on_key = function(e: any): boolean?
				local k = string.lower(e.key)
				if k == "enter" then
					step(if e.shift then -1 else 1)
					return true
				end
				if k == "escape" then
					close_find()
					return true
				end
				return nil
			end,
		},
		ui.div { grow = 0.001 },
		ui.row {
			shrink = 0,
			gap = "$zeron.space.hair",
			items = "center",
			IconButton {
				icon = "arrow-up",
				label = ui.t("panel.browser.find.previous"),
				size = "sm",
				tooltip_side = "bottom",
				disabled = none,
				on_click = function()
					step(-1)
				end,
			},
			IconButton {
				icon = "arrow-down",
				label = ui.t("panel.browser.find.next"),
				size = "sm",
				tooltip_side = "bottom",
				disabled = none,
				on_click = function()
					step(1)
				end,
			},
			IconButton {
				icon = "close",
				label = ui.t("common.close"),
				size = "sm",
				tooltip_side = "bottom",
				on_click = close_find,
			},
		},
	})

	-- The dev servers running for the session (the engine's previews), for
	-- the address bar's suggestions: watched while the start page shows
	-- (screens/panel/previews shares the host's watch with the panel's
	-- home, which lists them as its Ports).
	local chat_id = ui.computed(function(): string?
		local s = read(props.session)
		return if type(s) == "string" then s elseif type(s) == "table" then s.id else nil
	end)
	local function watching(): boolean
		return not has_page(web) and chat_id:get() ~= nil
	end
	ui.effect(function()
		if not watching() then
			return
		end
		ui.on_cleanup(Previews.use(chat_id:get() :: string))
	end)
	local function services(): { any }
		return Previews.services(chat_id:get())
	end
	preview_list = services

	-- The address of the load that failed last (a file's, for its page).
	local failed_url: string? = nil

	-- The page, or the start page before there is one.
	local content = ui.presence(function(): string
		return if has_page(web) then "page" else "start"
	end, function(which: string): Node
		if which == "start" then
			return ui.column {
				id = `panel-browser-start-{id}`,
				w_full = true,
				grow = 1,
				min_h = 0,
				items = "center",
				justify = "center",
				p = "$space.4",
				gap = "$space.4",
				-- (The session's running ports are the panel's home's: its Ports.)
				ds.EmptyState {
					icon = "global",
					size = "sm",
					title = ui.t("panel.browser.start.browse_title") :: any,
					body = ui.t("panel.browser.start.body") :: any,
				},
			}
		end
		return api.view {
			id = `panel-browser-page-{id}`,
			label = ui.t("panel.browser.web_page"),
			web = web,
			w_full = true,
			grow = 1,
			min_h = 0,
			on_message = function(value: any, _origin: string, main_frame: boolean)
				heard:set({ value = value, main_frame = main_frame })
				if not main_frame or type(value) ~= "table" or value.kind ~= "zeron.annotation" then
					return
				end
				if value.nonce ~= nonces:peek()[id] then
					return
				end
				if value.event == "pick" and type(value.pick) == "table" then
					on_pick(value.pick)
				elseif value.event == "state" and type(value.state) == "string" then
					on_state(value.state)
				end
			end,
			on_navigate = function(e: any)
				-- Another page: its picker is gone with the last, so the mode
				-- ends (the shipping app's; its marks come back by restore).
				if type(e) == "table" and e.kind == "committed" and annotating:peek()[id] then
					on_state("cancelled")
				end
				if type(e) == "table" and e.kind == "failed" and type(e.url) == "string" and e.url ~= "" then
					failed_url = e.url
				end
				if type(e) == "table" and e.kind == "finished" then
					restore_marks()
					find_icon()
				end
			end,
			on_download = function(d: any)
				take_download(d)
			end,
			on_new_window = function(url: string)
				if props.open_url and api.allowed(url) then
					props.open_url(url)
				end
			end,
		}
	end, { w_full = true, grow = 1, min_h = 0 })

	-- A page that didn't load: what happened, over where it would be, and
	-- the way back.
	-- WebKit's words for no network at all read as offline (the shipping
	-- app's words otherwise, the error's own under them).
	local function offline(): boolean
		local e = web.error
		if type(e) ~= "string" then
			return false
		end
		local l = string.lower(e)
		return string.find(l, "offline", 1, true) ~= nil
			or string.find(l, "internet connection", 1, true) ~= nil
			or string.find(l, "not connected", 1, true) ~= nil
	end
	local trouble = ui.show(function(): boolean
		return has_page(web) and web.error ~= nil
	end, ui.column {
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		bg = "$background",
		ui.presence(function(): string
			if is_file_error(web.error) then
				return "file"
			end
			return if offline() then "offline" else "failed"
		end, function(which: string): Node
			if which == "file" then
				return ds.EmptyState {
					icon = "document",
					size = "sm",
					fill = true,
					title = ui.t("panel.browser.file_title") :: any,
					body = ui.t("panel.browser.file_body") :: any,
					action = {
						label = ui.t("panel.browser.open_in_browser") :: any,
						on_click = function()
							-- (The file's address: the page's may still be the one
							-- its link was on.)
							local u = failed_url or web.url
							if type(u) == "string" then
								api.open_externally(u)
							end
						end,
					},
				}
			end
			return ds.EmptyState {
				icon = if which == "offline" then "wifi-off" else "danger-triangle",
				size = "sm",
				fill = true,
				title = (if which == "offline" then ui.t("panel.browser.offline_title") else ui.t("panel.browser.failed_title")) :: any,
				body = if which == "offline"
					then ui.t("panel.browser.offline_body") :: any
					else function(): string
						return web.error or ""
					end,
				action = {
					label = ui.t("common.retry") :: any,
					on_click = function()
						web:reload()
					end,
				},
			}
		end, { w_full = true, grow = 1, min_h = 0 }),
	})

	return ui.column {
		id = `panel-browser-{id}`,
		ref = root_ref,
		relative = true,
		w_full = true,
		grow = 1,
		min_h = 0,
		bar,
		problem_line,
		find_bar,
		downloads_bar,
		ui.column {
			relative = true,
			w_full = true,
			grow = 1,
			min_h = 0,
			content,
			trouble,
		},
		-- The address bar's suggestions, last: over the page.
		suggestions.list(field_box, root_ref, FIELD_CORNER),
	}
end)

return M
