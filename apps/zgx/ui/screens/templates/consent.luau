--!strict
-- Installing someone else's template: what it is, who signed it, that its
-- signature checks out, and plainly what it can do: a template is Zeron's
-- own code, so it runs with everything Zeron can. Its window is its own
-- (ui/template_consent.luau): no plugin and no template runs there, and
-- only that window can install anything (`templates.decide`), so nothing
-- standing in for Zeron's screens can say yes on your behalf.
--
-- The host publishes what to show as `template_consent`:
--   { item = Pending?, busy = boolean, error = string? }
-- (a package as the engine lists it waiting: `pending, id, name,
-- description, version, zeron_version, key, files, ignored, hash, source,
-- from = { kind, url?, ref? } (where it came from: a file, a link),
-- author = { kind = "you" | "verified" | "unknown", username?, reason? }`)
-- and answers `templates.decide { pending, allow, hash }` (the hash it was
-- shown: a package that changed meanwhile isn't installed).
--
-- An unsigned or changed package never gets here: the engine refuses it on
-- import. So the signature line always says it checks out; who signed it is
-- the question.
--
-- Keeping a template in preview (`item.keep`, the template's id: one an
-- agent made, changed or switched to, or signed by "you" and never kept)
-- shows the same sheet, as a keep: `templates.decide { keep, allow, hash }`,
-- `hash` the digest of its files as shown (src/plugins/preview.rs). Kept,
-- it runs with Zeron's powers until its files change.

local ds = require("@ds")
local words = require("@screens/plugins/words")

local M = {}

local consent = ui.shared("template_consent", { busy = true } :: any)

local function item(): any
	return consent:get().item
end

-- Who signed it, as Zeron's directory knows the key.
local function signer(it: any): Node
	local kind = it.author and it.author.kind
	if kind == "you" then
		return ds.Badge { ui.t("templates.consent.made_by_you"), tone = "success", icon = "shield-check" }
	elseif kind == "verified" then
		return ds.Badge { ui.t("templates.consent.made_by", { username = it.author.username }), tone = "info", icon = "shield-check" }
	end
	return ds.Badge { ui.t("plugins.words.unverified"), tone = "warning", icon = "danger-triangle" }
end

-- Whom the warning names (read now: in a function, it follows the locale).
local function whom(it: any): string
	local kind = it.author and it.author.kind
	if kind == "verified" then
		return ui.t("templates.consent.whom_verified", { username = it.author.username }):get()
	elseif kind == "you" then
		return ui.t("templates.consent.whom_you"):get()
	end
	return ui.t("templates.consent.whom_unknown"):get()
end

-- The signing key, short: enough to tell two apart.
local function short(key: string?): string
	local k = key or ""
	return if #k > 12 then `{string.sub(k, 1, 12)}…` else k
end

-- A line of facts: its glyph, its words, and the words under them.
local function fact(icon: string, title: string | Message, detail: (string | Message)?, tone: string?): Node
	return ui.row {
		w_full = true,
		gap = "$space.3",
		items = "start",
		ui.div {
			pt = "$zeron.space.hair",
			color = if tone then tone else "$text.secondary",
			ds.Icon { name = icon :: any, size = 16 },
		},
		ui.column {
			grow = 1,
			min_w = 0,
			gap = "$zeron.space.hair",
			ui.text { title, text_size = "$font.size.md", line_height = "$font.line.md", color = "$text.primary", wrap = true },
			if detail
				then ui.text { detail, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.secondary", wrap = true }
				else nil,
		},
	}
end

local function body(key: string): Node
	if key == "loading" then
		return ds.EmptyState { icon = "shield-check", title = ui.t("templates.consent.checking"), size = "sm" }
	elseif key == "none" then
		return ds.EmptyState {
			icon = "check-circle",
			title = ui.t("templates.consent.nothing"),
			body = ui.t("templates.consent.nothing_body"),
			size = "sm",
		}
	end
	local it = item()
	local unknown = not (it.author and (it.author.kind == "you" or it.author.kind == "verified"))
	local ignored: { string } = it.ignored or {}
	local made_for = if it.zeron_version and it.zeron_version ~= "" then it.zeron_version else nil
	local files = tonumber(it.files) or 0
	local keep = it.keep ~= nil
	local answer = function(allow: boolean)
		if keep then
			ui.invoke("templates.decide", { keep = it.keep, allow = allow, hash = it.hash })
		else
			ui.invoke("templates.decide", { pending = it.pending, allow = allow, hash = it.hash })
		end
	end
	return ui.column {
		w_full = true,
		h_full = true,
		gap = "$space.5",
		ui.row {
			w_full = true,
			gap = "$space.3",
			items = "center",
			ds.Tile { label = it.name, hue = words.hue(it.id or it.name), size = 40 },
			ui.column {
				grow = 1,
				min_w = 0,
				gap = "$space.1",
				ui.text {
					if keep
						then ui.t("templates.consent.keep_title", { name = it.name })
						else ui.t("templates.consent.install_title", { name = it.name }),
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
					signer(it),
					if it.version and it.version ~= "" then ds.Badge { it.version, tabular = true } else nil,
				},
			},
		},
		if keep
			then ds.Callout {
				tone = "danger",
				title = ui.t("templates.consent.keep_trust"),
				body = ui.t("templates.consent.keep_body"),
			}
			else ds.Callout {
				tone = "danger",
				title = ui.t("templates.consent.trust", {
					whom = function(): string
						return whom(it)
					end,
				}),
				body = ui.t("templates.consent.trust_body"),
			},
		if unknown and not keep
			then ds.Callout {
				tone = "warning",
				title = ui.t("plugins.words.unverified"),
				-- (The host's reason as it gives it; else Zeron's words, with
				-- where it came from.)
				body = (it.author and it.author.reason) or (function(): any
					local where = words.where(it.from)
					return if where
						then ui.t("templates.consent.unverified_body_from", { where = where })
						else ui.t("templates.consent.unverified_body")
				end)(),
			}
			else nil,
		ui.column {
			id = "template-consent-facts",
			w_full = true,
			gap = "$space.3",
			if keep
				then fact("file-code", ui.t("templates.consent.keep_files", { n = files }), ui.t("templates.consent.keep_files_detail"))
				else fact(
					"shield-check",
					ui.t("templates.consent.signature"),
					ui.t("templates.consent.signature_detail", { key = short(it.key) }),
					"$zeron.tone.success.ink"
				),
			if keep then nil else fact(
				"file-code",
				if made_for
					then ui.t("templates.consent.files_made_for", { n = files, version = made_for })
					else ui.t("templates.consent.files", { n = files }),
				if it.description and it.description ~= "" then it.description else nil
			),
			if #ignored > 0
				then fact(
					"lock-keyhole-unlocked",
					ui.t("templates.consent.left_out", { n = #ignored }),
					ui.t("templates.consent.left_out_detail", { files = table.concat(ignored, ", ") })
				)
				else nil,
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
				ui.div { grow = 1 },
				ds.Button {
					id = "template-consent-decline",
					tone = "secondary",
					size = "md",
					disabled = function(): boolean
						return consent:get().busy == true
					end,
					on_click = function()
						answer(false)
					end,
					ui.t("templates.consent.not_now"),
				},
				ds.Button {
					id = "template-consent-allow",
					tone = "primary",
					size = "md",
					disabled = function(): boolean
						return consent:get().busy == true
					end,
					on_click = function()
						answer(true)
					end,
					if keep then ui.t("templates.consent.keep") else ui.t("templates.consent.install"),
				},
			},
		},
	}
end

function M.View(): Node
	return ui.column {
		id = "template-consent",
		w_full = true,
		h_full = true,
		p = "$space.6",
		-- (Its window's title bar, with the window's buttons, above.)
		pt = "$space.7",
		-- One package's sheet, swapped for the next one's (or none): keyed by
		-- which package, as what, and who signed it, so it's built again only
		-- when there's something new to read. (The sheet is read once, as a
		-- whole: a decision is made on what was shown.)
		ui.presence(function(): string
			local s = consent:get()
			local it = s.item
			if not it then
				return if s.busy then "loading" else "none"
			end
			local a = it.author or {}
			return `{it.keep or it.pending}@{it.hash}:{a.kind or ""}:{a.username or ""}:{a.reason or ""}`
		end, body, { w_full = true, h_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
	}
end

return M
