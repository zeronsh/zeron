--!strict
-- Allowing a plugin: who made it, and each thing it asks for in plain
-- words, the risky ones last and in the danger's ink. Its window is its
-- own (ui/consent.luau): no plugin and no template runs there, and only
-- that window can allow anything (`plugins.decide`), so nothing standing
-- in for Zeron's screens can say yes on your behalf.
--
-- The host publishes what to show as `consent`:
--   { item = PluginItem?, busy = boolean, error = string?,
--     device_name = string? }   (another of your devices it's on: allowed
--                                there, and named here)
-- and answers `plugins.decide { id, allow, hash }` (the hash it was shown:
-- a plugin that changed while you looked asks again) and
-- `plugins.sign { id }` ("It's mine": signs it as yours, after which it
-- asks only for what it needs; this device's key, so not offered for
-- another device's).

local ds = require("@ds")
local words = require("@screens/plugins/words")

local M = {}

local consent = ui.shared("consent", { busy = false } :: any)

local function item(): any
	return consent:get().item
end

-- One permission: its glyph, its line, and the line under it.
local function line(l: words.Line): Node
	return ui.row {
		w_full = true,
		gap = "$space.3",
		items = "start",
		ui.div {
			pt = "$zeron.space.hair",
			color = if l.risky then "$zeron.tone.danger.ink" else "$text.secondary",
			ds.Icon { name = l.icon :: any, size = 16 },
		},
		ui.column {
			grow = 1,
			min_w = 0,
			gap = "$zeron.space.hair",
			-- (Read as it's drawn: words.Line's words follow the locale.)
			ui.text {
				function(): string
					return l.title
				end,
				text_size = "$font.size.md",
				line_height = "$font.line.md",
				color = if l.risky then "$zeron.tone.danger.ink" else "$text.primary",
				wrap = true,
			},
			if l.detail
				then ui.text {
					function(): string
						return l.detail or ""
					end,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					color = "$text.secondary",
					wrap = true,
				}
				else nil,
		},
	}
end

-- Who made it, as the signature says.
local function author(it: any): Node
	local kind = it.author and it.author.kind
	if kind == "you" then
		return ds.Badge { ui.t("plugins.consent.made_by_you"), tone = "success", icon = "shield-check" }
	elseif kind == "verified" then
		return ds.Badge { ui.t("plugins.consent.made_by", { username = it.author.username }), tone = "info", icon = "shield-check" }
	end
	return ds.Badge { ui.t("plugins.consent.unknown_author"), tone = "warning", icon = "danger-triangle" }
end

-- (The host's reason as it gives it; else Zeron's words.)
local function why_unknown(it: any): string | Message
	local reason = it.author and it.author.reason
	if it.status == "tampered" then
		return ui.t("plugins.consent.tampered_why")
	end
	return reason or ui.t("plugins.consent.unknown_why")
end

-- What it asks, as of this plugin's state: all of it for a new one, what's
-- new for one asking again.
local function asked(it: any): { string }
	if it.ask and #it.ask > 0 then
		return it.ask
	end
	return it.permissions or {}
end

local function body(key: string): Node
	if key == "none" then
		return ds.EmptyState { icon = "check-circle", title = ui.t("plugins.consent.nothing"), size = "sm" }
	end
	local it = item()
	-- Another device's: its lines say where it runs.
	local elsewhere = consent:peek().device_name
	if type(elsewhere) ~= "string" or elsewhere == "" then
		elsewhere = nil
	end
	local lines = words.lines(if elsewhere then words.on_device(it, elsewhere) else it, asked(it))
	local again = it.allowed and #it.allowed > 0 and it.status == "pending"
	local list: { Node } = {}
	for _, l in lines do
		table.insert(list, line(l))
	end
	if #list == 0 then
		table.insert(list, ui.text { ui.t("plugins.consent.asks_nothing"), text_size = "$font.size.md", color = "$text.secondary", wrap = true })
	end
	local unknown = not (it.author and (it.author.kind == "you" or it.author.kind == "verified"))
	return ui.column {
		w_full = true,
		h_full = true,
		gap = "$space.5",
		ui.row {
			w_full = true,
			gap = "$space.3",
			items = "center",
			words.tile(it, 40),
			ui.column {
				grow = 1,
				min_w = 0,
				gap = "$space.1",
				ui.text {
					ui.t("plugins.consent.allow_title", { name = it.name }),
					heading = 1,
					text_size = "$font.size.lg",
					line_height = "$font.line.lg",
					weight = "$font.weight.semibold",
					color = "$text.primary",
					wrap = true,
				},
				ui.row {
					gap = "$space.2",
					items = "center",
					flex_wrap = true,
					author(it),
					ds.Badge { it.version, tabular = true },
					if elsewhere then ds.Badge { ui.t("plugins.consent.on_device", { device = elsewhere }), icon = "laptop" } else nil,
				},
			},
		},
		if unknown
			then ds.Callout {
				tone = "warning",
				title = if it.status == "tampered" then ui.t("plugins.consent.tampered") else ui.t("plugins.consent.unknown_author"),
				body = why_unknown(it),
			}
			else nil,
		ui.column {
			w_full = true,
			gap = "$space.3",
			ui.text {
				if again then ui.t("plugins.consent.asks_again") else ui.t("plugins.consent.asks"),
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = "$text.secondary",
			},
			table.unpack(list),
		},
		ui.div { grow = 1 },
		ui.column {
			w_full = true,
			gap = "$space.2",
			ui.show(function(): boolean
				return consent:get().error ~= nil
			end, ui.text {
				function(): string
					return consent:get().error or ""
				end,
				text_size = "$font.size.sm",
				color = "$zeron.tone.danger.ink",
				wrap = true,
			}),
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				if unknown and it.status ~= "tampered" and not elsewhere
					then ds.Button {
						tone = "ghost",
						size = "md",
						disabled = function(): boolean
							return consent:get().busy == true
						end,
						on_click = function()
							ui.invoke("plugins.sign", { id = it.id })
						end,
						ui.t("plugins.consent.mine"),
					}
					else nil,
				ui.div { grow = 1 },
				ds.Button {
					id = "consent-decline",
					tone = "secondary",
					size = "md",
					disabled = function(): boolean
						return consent:get().busy == true
					end,
					on_click = function()
						ui.invoke("plugins.decide", { id = it.id, allow = false, hash = it.hash })
					end,
					ui.t("plugins.consent.not_now"),
				},
				ds.Button {
					id = "consent-allow",
					tone = "primary",
					size = "md",
					disabled = function(): boolean
						return consent:get().busy == true
					end,
					on_click = function()
						ui.invoke("plugins.decide", { id = it.id, allow = true, hash = it.hash })
					end,
					ui.t("plugins.consent.allow"),
				},
			},
		},
	}
end

function M.View(): Node
	return ui.column {
		id = "consent",
		w_full = true,
		h_full = true,
		p = "$space.6",
		-- (Its window's title bar, with the window's buttons, above.)
		pt = "$space.7",
		-- One plugin's sheet, swapped for the next one's (or none): keyed
		-- by which plugin and what it asks, so it's built again only when
		-- there's something new to read. (The sheet is read once, as a
		-- whole: a decision is made on what was shown.)
		ui.presence(function(): string
			local it = item()
			if not it then
				return "none"
			end
			return `{consent:get().device_name or ""}/{it.id}@{it.hash}:{it.author and it.author.kind or ""}`
		end, body, { w_full = true, h_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
	}
end

return M
