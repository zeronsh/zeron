--!strict
-- What you do as yourself in the store, asked in Zeron's own window
-- (ui/store_consent.luau) once a template that isn't Zeron's has run: the
-- main window is drawn by it, so its buttons aren't proof you pressed them.
-- The host says what's asked, as the engine told it:
--   `store_consent`: { item = { act = "publish" | "unyank" | "token", kind?,
--     id?, name, version?, scope?, expires? }, busy, secret? }
-- and answers `store.decide { allow }`. A new token is made here and shown
-- here (`secret`), once; the main window never has it.

local ds = require("@ds")
local locale = require("@ds/locale")
local words = require("@screens/plugins/words")
local token_block = require("@screens/plugins/token_block")

local M = {}

local shown = ui.shared("store_consent", { busy = true } :: any)

local function item(): any
	local now = shown:get()
	return if type(now.item) == "table" then now.item else {}
end

-- What's asked is a thing's own words, drawn as they are.
local function plain(v: any, most: number): string
	return if type(v) == "string" then string.sub(v, 1, most) else ""
end

local function title(it: any): string | Message
	if it.act == "publish" then
		return ui.t("plugins.store.act.publish.title", { name = plain(it.name, 64), version = plain(it.version, 32) })
	elseif it.act == "unyank" then
		return ui.t("plugins.store.act.unyank.title", { name = plain(it.name, 64), version = plain(it.version, 32) })
	end
	return ui.t("plugins.store.act.token.title")
end

local function scope_words(scope: any): string
	if type(scope) ~= "table" or scope.all == true then
		return ui.t("plugins.store.key.scope.all"):get()
	end
	local ids: { string } = {}
	for _, id in (if type(scope.plugins) == "table" then scope.plugins else {}) :: { any } do
		table.insert(ids, plain(id, 40))
	end
	return table.concat(ids, ui.t("plugins.words.list_sep"):get())
end

local function facts(it: any): Node
	local items: { any } = {}
	local function add(key: string, value: () -> string)
		table.insert(items, { key = ui.t(key), value = value })
	end
	if it.act == "token" then
		add("plugins.store.key.name", function(): string
			return plain(it.name, 64)
		end)
		add("plugins.store.key.scope.label", function(): string
			return scope_words(it.scope)
		end)
		add("plugins.store.key.expiry_label", function(): string
			return if type(it.expires) == "number" then locale.date(it.expires / 1000) else ui.t("plugins.store.key.expiry.never"):get()
		end)
	else
		add("plugins.store.act.id", function(): string
			return plain(it.id, 40)
		end)
		add("plugins.store.act.version", function(): string
			return plain(it.version, 32)
		end)
	end
	return ds.Settings.Group {
		ui.column { id = "store-consent-facts", w_full = true, px = "$space.4", py = "$space.3", ds.KeyValue { items = items, key_width = 96 } },
	}
end

local function answer(allow: boolean)
	ui.invoke("store.decide", { allow = allow })
end

local function asking(): Node
	local it = item()
	local act = if it.act == "publish" or it.act == "unyank" then it.act else "token"
	local function busy(): boolean
		return shown:get().busy == true
	end
	return ui.column {
		w_full = true,
		h_full = true,
		gap = "$space.5",
		ui.row {
			w_full = true,
			gap = "$space.3",
			items = "center",
			ds.Tile { label = plain(it.name, 64), hue = words.hue(plain(it.id or it.name, 64)), size = 40 },
			ui.text {
				title(it),
				heading = 1,
				grow = 1,
				min_w = 0,
				text_size = "$font.size.lg",
				line_height = "$font.line.lg",
				weight = "$font.weight.semibold",
				color = "$text.primary",
				wrap = true,
			},
		},
		ui.text {
			ui.t(`plugins.store.act.{act}.body`),
			w_full = true,
			wrap = true,
			text_size = "$font.size.md",
			line_height = 20,
			color = "$text.secondary",
		},
		facts(it),
		ds.Callout { tone = "info", title = ui.t("plugins.store.act.why_title"), body = ui.t("plugins.store.act.why_body") },
		ui.div { grow = 1 },
		ui.show(function(): boolean
			return busy() and act == "token"
		end, ui.text {
			id = "store-consent-signing-in",
			ui.t("plugins.store.key.signing_in"),
			w_full = true,
			wrap = true,
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			color = "$text.tertiary",
		}),
		ui.row {
			w_full = true,
			gap = "$space.2",
			items = "center",
			ui.div { grow = 1 },
			ds.Button {
				id = "store-consent-decline",
				tone = "secondary",
				size = "md",
				on_click = function()
					answer(false)
				end,
				ui.t("plugins.store.act.not_now"),
			},
			ds.Button {
				id = "store-consent-allow",
				tone = "primary",
				size = "md",
				busy = busy,
				disabled = busy,
				on_click = function()
					if not shown:peek().busy then
						answer(true)
					end
				end,
				ui.t(`plugins.store.act.{act}.allow`),
			},
		},
	}
end

-- The new token, until it's put away.
local function made(): Node
	local copied = ui.use_state(false)
	return ui.column {
		id = "store-consent-token",
		w_full = true,
		h_full = true,
		gap = "$space.5",
		ui.text {
			ui.t("plugins.store.key.secret_title"),
			heading = 1,
			w_full = true,
			wrap = true,
			text_size = "$font.size.lg",
			line_height = "$font.line.lg",
			weight = "$font.weight.semibold",
			color = "$text.primary",
		},
		ui.text {
			ui.t("plugins.store.key.secret_body"),
			w_full = true,
			wrap = true,
			text_size = "$font.size.md",
			line_height = 20,
			color = "$text.secondary",
		},
		token_block.Block {
			secret = function(): string?
				local s = shown:get().secret
				return if type(s) == "string" and s ~= "" then s else nil
			end,
			copy = function(token: string): any
				return ui.call("secret.copy", { text = token })
			end,
			copied = copied,
		},
		ui.div { grow = 1 },
		ui.row {
			w_full = true,
			items = "center",
			ui.div { grow = 1 },
			ds.Button {
				id = "store-consent-done",
				tone = "primary",
				size = "md",
				on_click = function()
					ui.invoke("close", nil)
				end,
				ui.t("plugins.store.key.done"),
			},
		},
	}
end

function M.View(): Node
	return ui.column {
		id = "store-consent",
		w_full = true,
		h_full = true,
		p = "$space.6",
		-- (Its window's title bar, with the window's buttons, above.)
		pt = "$space.7",
		ui.presence(function(): string
			local now = shown:get()
			if type(now.secret) == "string" and now.secret ~= "" then
				return "made"
			end
			-- (What's asked, whole: a sheet is of one thing from start to end.)
			local it = now.item
			return if type(it) == "table" then `ask:{it.act}:{plain(it.id, 40)}:{plain(it.version, 32)}:{plain(it.name, 64)}` else "none"
		end, function(key: string): Node
			return if key == "made" then made() elseif key == "none" then ui.div {} else asking()
		end, { w_full = true, h_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
	}
end

return M
