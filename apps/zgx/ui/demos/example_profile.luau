-- Settings: a profile. Who you are to your team, with your member card
-- beside the fields as the live preview of it: the fields at the left, and
-- at the right the card as it will be, your name on it changing as you
-- type it (one composed object: the card, and the ways to share it in a
-- quiet pill of glyphs under it, as wide as the card; no name or number
-- beside it that the card already says). Live: it tilts and shines under
-- the pointer. Where the
-- window is too narrow for two columns, the card stands centred over the
-- fields instead. Someone with no card yet sees the card's outline in the
-- same place, and the way to one.
--
-- The groups that aren't about how you appear (your preferences, where
-- you're signed in, the way out) stay in the fields' column under the
-- identity group: one column of groups keeps the page's rhythm, and the
-- card's column stays the object's own.

local ds = require("@ds")
local kit = require("@demos/example_kit")

local name = ui.state("ex_pr_name", "Mara Okafor")
local title = ui.state("ex_pr_title", "Staff engineer")
local zone = ui.state("ex_pr_zone", "lisbon")
local digest = ui.state("ex_pr_digest", true)
local shown = ui.state("ex_pr_state", "ready")
local deleting = ui.state("ex_pr_deleting", false)
-- The card's own state: shared, copied, or neither (the control sets it).
local card_state = ui.state("ex_pr_card", "ready")
-- The window's width (the demo's switch): settings' own, or one too narrow
-- for the list of pages, which goes, and for two columns.
local width = ui.state("ex_pr_width", "full")
local function narrow(): boolean
	return width:get() == "narrow"
end
local FULL, NARROW = 900, 560

-- Your card, as the directory has it: the next number on the day the
-- account was made (there were 1,373 members on Oct 28, 2026), and the
-- badges given since (none yet). Its name is the display name's, as typed.
local YOU = {
	number = 1378,
	-- The username: the card's address (zeron.sh/c/mara), chosen once.
	username = "mara",
	since = "Oct 30, 2026",
	layers = {
		{ src = "assets/cards/blossom/sky.webp", depth = 0 },
		{ src = "assets/cards/blossom/far.webp", depth = 1 },
		{ src = "assets/cards/blossom/near.webp", depth = 2 },
		{ src = "assets/cards/blossom/detail.webp", depth = 3 },
	},
	holo = "glyph",
	badges = {} :: { ds.CardBadge },
}

-- The card beside the fields: a card's 300 by 420 at this, the column it
-- stands in as wide as it.
local ZOOM = 0.6
local W, H = 300, 420
local CARD_W = W * ZOOM

-- A card's outline where a card would be, for someone without one yet: a
-- dashed hairline on the card's own shape, the mark faint in its corner.
local function outline(): Node
	return ui.div {
		w = CARD_W,
		h = H * ZOOM,
		shrink = 0,
		p = "$space.3",
		rounded = "$zeron.card.radius",
		border = 1,
		border_dash = { 4, 4 },
		border_color = "$border.strong",
		ui.text { "zeron", font = "$font.sans", weight = "$font.weight.bold", text_size = "$font.size.xs", color = "$text.disabled" },
	}
end

return {
	id = "ex_profile",
	title = "Profile",
	about = "Settings → Profile: your photo, name and title as your team sees them, with your member card beside the fields as the live preview (your name on it as you type), the ways to share it under it, your preferences, the devices you're signed in on, and an account deletion that makes you type your email. A save that says how it went; a profile with no card yet.",
	demo = function()
		local function name_error(): string?
			return if string.match(name:get(), "%S") then nil else "Your teammates need something to call you."
		end
		local function has_card(): boolean
			return shown:get() ~= "nocard"
		end
		-- The card as it will be, your name on it as you type it; under it,
		-- the ways to share it, glyphs alone, the card's width.
		local function preview(): Node
			return ui.column {
				w = CARD_W,
				shrink = 0,
				gap = "$space.3",
				items = "center",
				ui.div {
					zoom = ZOOM,
					shrink = 0,
					ds.MemberCard {
						number = YOU.number,
						name = function(): string
							local n = name:get()
							return if string.match(n, "%S") then n else "—"
						end,
						since = YOU.since,
						layers = YOU.layers,
						holo = YOU.holo,
						badges = if #YOU.badges > 0 then YOU.badges else nil,
					},
				},
				-- Its address, the username: where the card lives, what the
				-- ways under it share.
				ui.text { `zeron.sh/c/{YOU.username}`, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.tertiary" },
				ds.CardActions { state = card_state, form = "glyphs", w = CARD_W },
			}
		end
		-- No card yet (no username chosen, or no account): its outline in the
		-- same place, and the way to one, which is a username.
		local function missing(): Node
			return ui.column {
				w = CARD_W,
				shrink = 0,
				gap = "$space.3",
				items = "center",
				outline(),
				ui.text { "No card yet", text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.tertiary" },
				ds.Button { "Claim a username", tone = "secondary", size = "sm" },
			}
		end
		local function aside(): Node
			return ui.presence(has_card, function(with_card: boolean): Node
				return if with_card then preview() else missing()
			end, { enter = { opacity = { 0, 1 }, duration = 160 }, exit = { opacity = 0, duration = 80 }, mode = "wait" })
		end
		return kit.stage {
			states = {
				{ value = "ready", label = "Ready" },
				{ value = "saving", label = "Saving" },
				{ value = "failed", label = "Failed" },
				{ value = "nocard", label = "No card" },
			},
			state = shown,
			also = ds.Segmented {
				value = width,
				options = { { value = "full", label = "Full window" }, { value = "narrow", label = "Narrow" } },
				label = "Width",
			},
			w = function(): number
				return if narrow() then NARROW else FULL
			end,
			h = 640,
			overlays = {
				ds.Dialog {
					open = deleting,
					title = "Delete your account?",
					body = "You'll leave Halcyon Labs and be signed out everywhere. This can't be undone.",
					tone = "danger",
					size = "md",
					confirm_text = "mara@halcyon.dev",
					confirm = { label = "Delete account" },
				},
			},
			kit.settings {
				page = "profile",
				-- (Built again as the width switches: the aside moves.)
				ui.presence(narrow, function(is_narrow: boolean): Node
					return ds.Settings.Page {
						title = "Profile",
						description = "How you appear to everyone in Halcyon Labs.",
						aside = aside(),
						aside_w = CARD_W,
						aside_top = is_narrow,
						ui.show(function()
							return shown:get() == "failed"
						end, ds.Callout { tone = "danger", title = "Couldn't save your profile", body = "Your changes are still here. Check your connection and try again.", action = { label = "Try again", on_click = function() shown:set("ready") end } }),
						-- Who you are: what the card beside it previews.
						ds.Settings.Section {
							ds.Settings.Group {
								ds.Settings.Row {
									label = "Photo",
									description = "Shows beside your chats and comments.",
									lead = ds.Avatar { seed = "mara", size = 40 },
									control = ds.Button { "Change", tone = "secondary", size = "sm" },
								},
								ds.Settings.Row {
									label = "Name",
									description = "What your teammates call you. It's on your card.",
									stacked = true,
									control = ds.TextField { value = name, name = "Name", w_full = true, error = name_error },
								},
								ds.Settings.Row {
									label = "Title",
									description = "Under your name, wherever it shows.",
									stacked = true,
									control = ds.TextField { value = title, name = "Title", placeholder = "What you do", w_full = true },
								},
								-- The username: chosen once, so no control; what it is,
								-- or that it isn't yet (the card's column offers it).
								ds.Settings.Row {
									label = "Username",
									description = function()
										return if has_card() then `zeron.sh/c/{YOU.username}` else "Not chosen yet. Your card needs one."
									end,
								},
								ds.Settings.Row {
									label = "Email",
									description = "mara@halcyon.dev",
									badge = ds.Badge { "Verified", tone = "success", icon = "check-circle" },
									control = ds.Button { "Change", tone = "secondary", size = "sm" },
								},
							},
							ui.row {
								w_full = true,
								justify = "end",
								pt = "$space.2",
								gap = "$space.2",
								ds.Button {
									function()
										return if shown:get() == "saving" then "Saving…" else "Save changes"
									end,
									disabled = function()
										return shown:get() == "saving" or name_error() ~= nil
									end,
								},
							},
						},
						ds.Settings.Section {
							title = "Preferences",
							ds.Settings.Group {
								ds.Settings.Row {
									label = "Time zone",
									description = "Used for quiet hours and your weekly digest.",
									control = ds.Select {
										value = zone,
										name = "Time zone",
										w = 160,
										options = {
											{ value = "lisbon", label = "Lisbon" },
											{ value = "london", label = "London" },
											{ value = "berlin", label = "Berlin" },
											{ value = "nyc", label = "New York" },
										},
									},
								},
								ds.Settings.Row { label = "Weekly digest", description = "What your agents did, on Monday mornings.", control = ds.Switch { value = digest, label = "Weekly digest" } },
							},
						},
						ds.Settings.Section {
							title = "Signed in on",
							ds.Settings.Group {
								ds.Settings.Row { label = "Mara's MacBook Pro", description = "Lisbon · Active now", icon = "laptop", badge = ds.Badge { "This device" } },
								ds.Settings.Row { label = "build-box-02", description = "Frankfurt · 3 hours ago", icon = "remote-server", control = ds.Button { "Sign out", tone = "ghost", size = "sm" } },
							},
						},
						ds.Settings.Section {
							ds.Settings.Group {
								ds.Settings.Row { label = "Delete account", description = "Your chats stay with your team. This can't be undone.", danger = true, on_click = function() deleting:set(true) end },
							},
						},
					}
				end, { w_full = true, enter = { opacity = { 0, 1 }, duration = 160 }, exit = { opacity = 0, duration = 80 }, mode = "wait" }),
			},
		}
	end,
}
