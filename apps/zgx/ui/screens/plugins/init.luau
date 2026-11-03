--!strict
-- Plugins in the main window: the Plugins page in Settings, and what
-- plugins are shown of Zeron and may ask of it. Required once, as the
-- main window starts (app.luau). The host (src/plugins) loads the plugins
-- and your templates and keeps them to what they were allowed; this side
-- is the app's:
--
-- - **What plugins see** (read only), kept up to date from the shell's
--   route. Every plugin has the route, the appearance and the session
--   page's session id; "sessions.read", "profile.read" and "devices" grant
--   the rest:
--     zeron.route    { name = "home" | "session" | "settings", session?, page?, project? }
--     zeron.page_session  the session page's session id, or nil
--     zeron.profile_id    your id
--     zeron.session  { id, title, project?, device? } or nil
--     zeron.project  { id, name } or nil (never its path)
--     zeron.device   { id, name } where you're working
--     zeron.devices  { { id, name, here } } your devices
--     zeron.appearance  "light" | "dark"
-- - **What records are made from**, for the host alone (no plugin reads
--   these; each reads its own `plugin.<id>.records`, holding only what it
--   was allowed, src/plugins/records.rs):
--     plugins.profile      { id, name, username, avatar, email }
--     plugins.hide_emails  Settings → General's "hide email addresses"
-- - **What plugins ask**, through the host's actions (zeron.open,
--   zeron.compose.insert, a plugin's toast), arriving here as requests in
--   `plugins.request`: done in order, each once.
-- - **When a template or plugin fails**, Zeron's own takes its place: never
--   a placeholder. The host hears every script error and drops whichever
--   template or plugin it's on (src/plugins/recovery.rs); a screen that
--   fails to load or build inside the shell's boundary shows nothing for
--   that moment (`boundary.on_fail` below) while the host drops what stood
--   in for it, and the built-in arrives on the next frame. A quiet notice
--   says what happened, once.

local ds = require("@ds")
local settings = require("@app/screens/shell/settings")
local route = require("@app/screens/shell/route")
local boundary = require("@app/screens/shell/boundary")
local sessions = require("@account/sessions")
local auth = require("@account/auth")
local store = require("@app/screens/settings/store")
local PrivateText = require("@ds/private_text")
local state = require("@app/screens/plugins/state")
local page = require("@app/screens/plugins/page")
local chat = require("@app/screens/chat")
local ChatShared = require("@app/screens/chat/shared")
local Cargo = require("@ds/composer_cargo")

-- Each plugin's review comments still on a draft (`zeron.compose_comment`),
-- told back to it: its key → the session it's in and whether its chip is
-- under the pointer (`plugin.<id>.composed`), so the diff it was left on
-- keeps a mark on its lines and lights them as the chip is pointed at.
-- (Only plugins that have had one are told, and told again when they
-- have none left.)
local composed_told: { [string]: boolean } = {}
ui.effect(function()
	local all = ChatShared.plugin_comments()
	local over = Cargo.hovered:get()
	for plugin in all do
		composed_told[plugin] = true
	end
	for plugin in composed_told do
		local out: { [string]: any } = {}
		for key, c in all[plugin] or {} do
			out[key] = { session = c.session, hovered = c.id == over }
		end
		ui.shared(`plugin.{plugin}.composed`, {} :: any):set(out)
	end
end)

settings.add {
	id = "plugins",
	title = "Plugins",
	icon = "widget",
	view = page.View,
	group = "Customize",
	before = "archived",
}

-- Themes and Templates have their own pages, beside this one (added after
-- it, so they list right after Plugins).
require("@app/screens/themes")
require("@app/screens/templates")

-- Each plugin's own settings: a page of its own under "Plugins", while
-- it's on (screens/plugins/settings).
require("@app/screens/plugins/settings")

-- What plugins see, written as it changes (each a steady table, so a
-- plugin reading one hears only real changes).
local out = {
	route = ui.shared("zeron.route", { name = "home" } :: any),
	session = ui.shared("zeron.session", nil :: any),
	project = ui.shared("zeron.project", nil :: any),
	device = ui.shared("zeron.device", nil :: any),
	devices = ui.shared("zeron.devices", {} :: { any }),
	appearance = ui.shared("zeron.appearance", "dark"),
	page_session = ui.shared("zeron.page_session", nil :: string?),
	profile_id = ui.shared("zeron.profile_id", nil :: string?),
	profile = ui.shared("plugins.profile", {} :: any),
	hide_emails = ui.shared("plugins.hide_emails", true),
}

local function put(s: any, value: any)
	s:set(value)
end

ui.effect(function()
	local c = route.current:get()
	put(out.route, { name = c.name, session = c.session or ui.null, page = c.page or ui.null, project = c.project or ui.null })
	local id = if c.name == "session" and type(c.session) == "string" then c.session else nil
	if out.page_session:peek() ~= id then
		put(out.page_session, id)
	end
end)
-- You, as far as Zeron knows you yet: the account's, then Settings'
-- profile and username once they've loaded.
ui.effect(function()
	local user: any = auth.user:get()
	local me: any = store.me:get() or {}
	local id = if user then user.id else nil
	put(out.profile, {
		id = id or ui.null,
		name = me.name or (if user then user.first_name else nil) or ui.null,
		username = store.username:get() or ui.null,
		email = me.email or (if user then user.email else nil) or ui.null,
	})
	if out.profile_id:peek() ~= id then
		put(out.profile_id, id)
	end
end)
ui.effect(function()
	local hidden = PrivateText.hidden:get()
	if out.hide_emails:peek() ~= hidden then
		put(out.hide_emails, hidden)
	end
end)
ui.effect(function()
	local s = route.session:get()
	put(out.session, if s then { id = s.id, title = s.title or "", project = s.project or ui.null, device = s.device or ui.null } else nil)
end)
ui.effect(function()
	local p = route.project:get()
	put(out.project, if p then { id = p.id, name = p.name or "" } else nil)
end)
ui.effect(function()
	local now = sessions.now:get()
	local id = route.device:get()
	local list = {}
	local here: any = nil
	for _, d in now.devices or {} do
		table.insert(list, { id = d.id, name = d.name, here = d.here })
		if d.id == id then
			here = { id = d.id, name = d.name }
		end
	end
	put(out.devices, list)
	put(out.device, here)
end)
ui.effect(function()
	ui.theme.current:get()
	local kind = ui.theme.kind()
	put(out.appearance, if kind == "light" or kind == "high-contrast-light" then "light" else "dark")
end)

-- What plugins asked for, done here in order. The host numbers each;
-- those already done are skipped (a reload of the scripts keeps `done`).
local requests = ui.shared("plugins.request", {} :: { any })
local done = ui.state("plugins_request_done", 0)
local function run(r: any)
	if r.kind == "open" then
		local to = r.value
		if to == "home" then
			route.new_chat(nil)
		elseif type(to) == "table" and type(to.session) == "string" then
			route.open_session(to.session)
		elseif type(to) == "table" and type(to.settings) == "string" and to.settings ~= "account" then
			route.settings(to.settings)
		end
	elseif r.kind == "compose" and type(r.value) == "string" then
		chat.store.insert(string.sub(r.value, 1, 20000))
	elseif r.kind == "compose_comment" and type(r.value) == "table" and type(r.title) == "string" then
		-- A plugin's review comment on a file or a diff (the host checked
		-- "compose"): a comment chip on the session's draft, as the files
		-- pane's, sent as production's review comments block. Its key
		-- comes back to it in `plugin.<id>.composed` while it's carried.
		local c = r.value
		local line = tonumber(c.line)
		if type(c.file) == "string" and c.file ~= "" and line and type(c.note) == "string" then
			ChatShared.attach_comment {
				file = c.file,
				line = math.floor(line),
				to = if tonumber(c.to) then math.floor(tonumber(c.to) :: number) else nil,
				side = if c.side == "L" or c.side == "R" then c.side else nil,
				snippet = if type(c.snippet) == "string" then string.sub(c.snippet, 1, 20000) else nil,
				body = string.sub(c.note, 1, 20000),
				plugin = r.title,
				key = if type(c.key) == "string" then c.key else nil,
				session = if type(c.session) == "string" then c.session else nil,
			}
		end
	elseif r.kind == "panel_tab" and type(r.value) == "table" then
		-- A plugin's tab in a chat's panel (the host checked "panel.tabs").
		local ok, Panel = pcall(require, "@app/screens/panel")
		if ok and type(Panel) == "table" and Panel.open_plugin_tab then
			Panel.open_plugin_tab(r.value.session, r.value)
		end
	elseif r.kind == "link" and type(r.value) == "string" then
		-- A plugin's link (the host checked "links.open" and that it's a web
		-- link): where Zeron opens links.
		require("@ds/links").open(r.value)
	elseif r.kind == "toast" then
		ds.toast.show { title = r.title, body = r.value }
	elseif r.kind == "waiting" then
		ds.toast.show {
			title = ui.t("plugins.init.waiting", { name = r.title }),
			tone = "info",
			action = {
				label = ui.t("plugins.init.review"),
				run = function()
					state.review(r.value)
				end,
			},
		}
	elseif r.kind == "template_waiting" then
		-- Someone else's template, brought in (an agent's import_template):
		-- its consent window, the host's, installs it.
		ds.toast.show {
			title = ui.t("plugins.init.waiting", { name = r.title }),
			body = ui.t("plugins.init.template_waiting_body"),
			tone = "info",
			action = {
				label = ui.t("plugins.init.review"),
				run = function()
					ui.invoke("templates.review", r.value)
				end,
			},
		}
	elseif r.kind == "plugin_failed" then
		ds.toast.show {
			title = ui.t("plugins.init.failed", { name = r.title }),
			body = ui.t("plugins.init.failed_body"),
			tone = "warning",
			action = {
				label = ui.t("plugins.init.show_folder"),
				run = function()
					state.open_folder(r.value.id)
				end,
			},
		}
	elseif r.kind == "template_failed" then
		ds.toast.show {
			title = ui.t("plugins.init.template_failed", { name = r.title }),
			body = ui.t("plugins.init.template_failed_body"),
			tone = "warning",
		}
	elseif r.kind == "template_broken" then
		ds.toast.show {
			title = ui.t("plugins.init.template_broken", { name = r.title }),
			body = ui.t("plugins.init.template_broken_body"),
			tone = "warning",
		}
	end
end
ui.effect(function()
	local list = requests:get()
	local last = done:peek()
	for _, r in list do
		if r.seq > last then
			last = r.seq
			local ok, why = pcall(run, r)
			if not ok then
				print(`zeron-zgx: a plugin's request failed: {why}`)
			end
		end
	end
	if last ~= done:peek() then
		done:set(last)
	end
end)

-- A screen that failed while a template or a plugin stands in for it:
-- nothing in its place for this moment (never the "didn't load" one), and
-- the host told, which puts Zeron's own back.
boundary.on_fail(function(module: string, why: string): Node?
	-- Ours when a plugin's copy failed (only theirs live in `.loaded`), or
	-- when the error names a file a plugin or the template stands in for,
	-- wherever the screen that failed (a template's chat/home fails as the
	-- chat page builds it).
	local ours = string.find(why, "/.loaded/", 1, true) ~= nil
	for _, replaced in state.now:peek().replaced or {} do
		local stem = string.gsub(replaced, "^@app/", "")
		if replaced == module or string.find(why, stem .. ".luau:", 1, true) or string.find(why, stem .. "/init.luau:", 1, true) then
			ours = true
		end
	end
	if not ours then
		return nil
	end
	ui.invoke("templates.failed", { module = module, error = why })
	return ui.div {}
end)

return {}
