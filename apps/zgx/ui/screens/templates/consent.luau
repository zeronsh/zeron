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
-- (from the store, `store = { id, publisher, verified, verified_alternative }`,
-- and `update_of`, `replaces_edits`, `new_owner`, `also_installed`; `key` nil
-- for one a deploy token published;
-- a package as the engine lists it waiting: `pending, id, name,
-- description, version, zeron_version, key, files, ignored, hash, source,
-- from = { kind, url?, ref? } (where it came from: a file, a link),
-- author = { kind = "you" | "verified" | "unknown", username?, reason? }`)
-- and answers `templates.decide { pending, allow, hash }` (the hash it was
-- shown: a package that changed meanwhile isn't installed).
--
-- A changed package never gets here, nor an unsigned one but the store's: the
-- engine refuses it on import. So the signature line always says it checks
-- out; who signed it is the question. One a deploy token published has none:
-- what vouches for it is the store's record, which it says instead.
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

-- Allow is armed a moment after a sheet shows (the host holds one sooner:
-- src/plugins/template_consent.rs), so a click or an Enter meant for the main
-- window, as this one comes up under it, allows nothing.
M.ARM_MS = require("@screens/plugins/arm").MS
local armed = ui.state("template_consent_armed", false)

local function item(): any
	return consent:get().item
end

local function bare_version(it: any): boolean
	return type(it.version) == "string" and string.match(it.version, "^%d[%d.]*$") ~= nil
end

-- A username as its badge takes it: with "…" where the widest letters would
-- run past the window's side (39 at most; it's whole in the red box). Cut
-- here, by what each letter may take: zgx's own "…" goes to the last word's
-- end, which is the whole of it.
local NAME_W = 230
local function clipped(username: string): string
	local w = 0
	for i = 1, #username do
		w += if string.find(string.sub(username, i, i), "[mwMW]") then 10 else 7
		if w > NAME_W then
			return string.sub(username, 1, i - 1) .. "…"
		end
	end
	return username
end

-- Who signed it, as Zeron's directory knows the key.
-- Who made it, as drawn: the store's publisher for one of the store's (what
-- the engine is told was shown), else who signed it.
local function maker(it: any): string
	local store = if type(it.store) == "table" then it.store else nil
	if store and type(store.publisher) == "string" and store.publisher ~= "" then
		return store.publisher
	end
	return if it.author and type(it.author.username) == "string" then it.author.username else ""
end

local function signer(it: any): Node
	local kind = it.author and it.author.kind
	if kind == "you" then
		return ds.Badge { ui.t("templates.consent.made_by_you"), tone = "success", icon = "shield-check" }
	elseif kind == "verified" then
		return ds.Badge { ui.t("templates.consent.made_by", { username = clipped(maker(it)) }), class = "template-consent-maker", tone = "info", icon = "shield-check" }
	end
	return ds.Badge { ui.t("plugins.words.unverified"), tone = "warning", icon = "danger-triangle" }
end

-- Whom the warning names (read now: in a function, it follows the locale).
local function whom(it: any): string
	local kind = it.author and it.author.kind
	if kind == "verified" then
		return ui.t("templates.consent.whom_verified", { username = maker(it) }):get()
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

-- The store's record of it, if that's where it's from.
local function store_of(it: any): any?
	return if type(it.store) == "table" then it.store else nil
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

-- Why it wasn't done: the engine's reason in Zeron's words where it has them,
-- else as it says it.
local OWN: { [string]: string } = { taken_down = "taken_down", not_in_store = "not_in_store" }
local function why(error: any): string
	local said = if type(error) == "string" then error else ""
	local own = OWN[string.match(said, "^([%l_]+): ") or string.match(said, "^([%l_]+)$") or ""]
	return if own then ui.t(`templates.consent.{own}`):get() else words.failure(said)
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
	local store = store_of(it)
	local update = it.update_of ~= nil
	local alternative: string? = if store
			and store.verified == false
			and type(store.verified_alternative) == "table"
			and type(store.verified_alternative.version) == "string"
		then store.verified_alternative.version
		else nil
	local answer = function(allow: boolean)
		if keep then
			ui.invoke("templates.decide", { keep = it.keep, allow = allow, hash = it.hash })
		else
			ui.invoke("templates.decide", { pending = it.pending, allow = allow, hash = it.hash })
		end
	end
	local changes: { Node } = {}
	local function change(id: string, words_of: Message)
		table.insert(changes, ui.div { id = id, w_full = true, fact("danger-triangle", words_of, nil, "$zeron.tone.warning.ink") })
	end
	-- (The verified version taken instead, which may be older than what's here.)
	local replaced: any = if it.instead == true and type(it.from) == "table" and type(it.from.replaced) == "table" then it.from.replaced else nil
	if replaced and not keep then
		local was = if type(replaced.version) == "string" then replaced.version else ""
		local said = if was == ""
			then ui.t("templates.consent.instead_unknown", { to = it.version or "" })
			elseif replaced.older == true then ui.t("templates.consent.instead_older", { from = was, to = it.version or "" })
			else ui.t("templates.consent.instead", { from = was, to = it.version or "" })
		table.insert(changes, ui.div { id = "template-consent-instead", w_full = true, fact("info-circle", said, nil, "$zeron.tone.info.ink") })
	end
	if store and store.verified ~= true and not keep and not alternative then
		change("template-consent-unverified", ui.t("plugins.store.not_verified_yet", { version = it.version or "" }))
	end
	local changed: any = if not keep and type(it.store_changed) == "table" then it.store_changed else nil
	if changed then
		change(
			"template-consent-changed",
			if changed.unverified == true
				then ui.t("templates.consent.now_unverified")
				elseif changed.verified == true then ui.t("templates.consent.now_verified")
				elseif changed.publisher == true then ui.t("templates.consent.now_publisher")
				else ui.t("templates.consent.store_changed")
		)
	end
	if (it.new_owner == true or (changed and changed.publisher == true)) and not keep then
		change("template-consent-owner", ui.t("templates.consent.new_owner", { username = if store and type(store.publisher) == "string" then store.publisher else "" }))
	end
	if it.replaces_edits == true and not keep then
		change("template-consent-edits", ui.t("templates.store.update_replaces_edits"))
	end
	return ui.column {
		w_full = true,
		h_full = true,
		gap = "$space.5",
		-- What's decided on, all of it: it keeps its height and scrolls where the
		-- window can't hold it; the answer stays under it.
		ui.scroll {
			id = "template-consent-scroll",
			w_full = true,
			grow = 1,
			basis = 0,
			min_h = 0,
			fade = { 12, 12 },
			fade_overflow = true,
			ui.column {
				id = "template-consent-said",
				w_full = true,
				shrink = 0,
				gap = "$space.5",
				ui.row {
					w_full = true,
					gap = "$space.3",
					-- (The tile at the title's first line, however many it takes.)
					items = "start",
					ds.Tile { label = it.name, hue = words.hue(it.id or it.name), size = 40 },
					ui.column {
						grow = 1,
						min_w = 0,
						gap = "$space.1",
						ui.text {
							if keep
								then ui.t("templates.consent.keep_title", { name = it.name })
								elseif update then ui.t("templates.consent.update_title", { name = it.name })
								else ui.t("templates.consent.install_title", { name = it.name }),
							heading = 1,
							text_size = "$font.size.lg",
							line_height = "$font.line.lg",
							weight = "$font.weight.semibold",
							color = "$text.primary",
							wrap = true,
						},
						-- (Each whole: no name long enough to push the store's word on
						-- it out of the window.)
						ui.row {
							gap = "$space.2",
							items = "center",
							flex_wrap = true,
							signer(it),
							-- (The store's word on this version, as of now.)
							if store and store.verified == true
								then ds.Badge { ui.t("plugins.store.verified"), class = "template-consent-verified", tone = "success" }
								elseif store then ds.Badge { ui.t("plugins.store.review.waiting"), class = "template-consent-verified", tone = "warning" }
								else nil,
							-- (A version of numbers alone: one with words of its own
							-- is said in the facts, never beside the marks.)
							if bare_version(it) then ds.Badge { it.version, tabular = true, class = "template-consent-version", max_w = 120, tooltip = it.version } else nil,
						},
					},
				},
				-- What changed as this showed, whose it is now, and what of yours
				-- goes: the first facts, under the head, so no name is long
				-- enough to push them out of view.
				if #changes > 0 then ui.column { id = "template-consent-changes", w_full = true, gap = "$space.3", table.unpack(changes) } else nil,
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
				-- Not verified, and the version that is, to take instead. (With none
				-- to take, it's said in the first of the lines above.)
				if alternative and not keep
					then ui.div {
						id = "template-consent-unverified",
						w_full = true,
						ds.Callout {
							tone = "warning",
							title = ui.t("plugins.store.not_verified_yet", { version = it.version or "" }),
							action = {
								label = ui.t("plugins.store.install_instead", { version = alternative }),
								on_click = function()
									if armed:peek() and consent:peek().busy ~= true then
										ui.invoke("templates.install_verified", nil)
									end
								end,
							},
						},
					}
					else nil,
				if unknown and not keep and not store
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
						elseif type(it.key) == "string" and it.key ~= "" then fact(
							"shield-check",
							ui.t("templates.consent.signature"),
							ui.t("templates.consent.signature_detail", { key = short(it.key) }),
							"$zeron.tone.success.ink"
						)
						-- (No signature: a deploy token published it, and it's exactly
						-- what the store's record says, or the engine refused it.)
						elseif store then fact("shield-check", ui.t("templates.consent.store_record"), ui.t("templates.consent.store_record_detail"))
						else nil,
					-- Where it came from.
					if keep
						then nil
						elseif store
						then fact("global", ui.t("templates.consent.from_store"), ui.t("templates.consent.from_store_detail", { username = if type(store.publisher) == "string" then store.publisher else "" }))
						else fact("file-code", if type(it.from) == "table" and (it.from.kind == "link" or it.from.kind == "release") then ui.t("templates.consent.from_link") else ui.t("templates.consent.from_file")),
					if type(it.also_installed) == "table" and #it.also_installed > 0 and not keep
						then fact("gallery", ui.t("templates.store.other_title"), ui.t("templates.store.other_body"))
						else nil,
					if keep then nil else fact(
						"file-code",
						if made_for
							then ui.t("templates.consent.files_made_for", { n = files, version = made_for })
							else ui.t("templates.consent.files", { n = files })
					),
					if type(it.version) == "string" and it.version ~= "" and not bare_version(it)
						then fact("info-circle", ui.t("templates.consent.version", { version = it.version }))
						else nil,
					if #ignored > 0
						then fact(
							"lock-keyhole-unlocked",
							ui.t("templates.consent.left_out", { n = tonumber(it.ignored_n) or #ignored }),
							ui.t("templates.consent.left_out_detail", { files = table.concat(ignored, ", ") .. (if (tonumber(it.ignored_n) or 0) > #ignored then "…" else "") })
						)
						else nil,
				},
				-- The package's own words, under a line saying so: never among the
				-- facts, which are Zeron's.
				if type(it.description) == "string" and it.description ~= "" and not keep
					then ui.column {
						id = "template-consent-own-words",
						w_full = true,
						gap = "$zeron.space.hair",
						ui.text { ui.t("templates.consent.own_words"), w_full = true, wrap = true, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.tertiary" },
						ui.text {
							function(): string
								return it.description
							end,
							w_full = true,
							wrap = true,
							text_size = "$font.size.sm",
							line_height = "$font.line.sm",
							color = "$text.secondary",
						},
					}
					else nil,
			},
		},
		ui.column {
			w_full = true,
			gap = "$space.2",
			ui.show(function(): boolean
				return consent:get().error ~= nil
			end, ui.text {
				function(): string
					return why(consent:get().error)
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
						return consent:get().busy == true or not armed:get()
					end,
					on_click = function()
						if armed:peek() then
							answer(true)
						end
					end,
					if keep then ui.t("templates.consent.keep") elseif update then ui.t("plugins.store.update") else ui.t("templates.consent.install"),
				},
			},
		},
	}
end

-- What the store says of it (a word of it changed is a sheet to read again).
local function said_by_store(it: any): string
	local store = store_of(it)
	return if store then `{it.store_changed ~= nil}:{store.verified == true}:{store.publisher or ""}` else ""
end

-- Which sheet shows: one package's, as what, and by whom.
local function shown_key(): string
	local s = consent:get()
	local it = s.item
	if not it then
		return if s.busy then "loading" else "none"
	end
	local a = it.author or {}
	return `{it.keep or it.pending}@{it.hash}:{a.kind or ""}:{a.username or ""}:{a.reason or ""}:{said_by_store(it)}`
end

-- (A sheet's who, looked up as it shows, is the same sheet: armed as it was.)
local function sheet_key(): string
	local it = consent:get().item
	return if it then `{it.keep or it.pending}@{it.hash}:{said_by_store(it)}` else ""
end

function M.View(): Node
	local sheet = ui.computed(sheet_key)
	local arming = 0
	ui.effect(function()
		local key = sheet:get()
		arming += 1
		local mine = arming
		armed:set(false)
		if key ~= "" then
			ui.after(M.ARM_MS, function()
				if mine == arming then
					armed:set(true)
				end
			end)
		end
	end)
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
		ui.presence(shown_key, body, { w_full = true, h_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
	}
end

return M
