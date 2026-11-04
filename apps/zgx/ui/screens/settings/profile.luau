-- Settings → Account (id `profile`): who you are to your team, the gallery's Profile
-- (demos/example_profile) on your account: the fields at the left, and
-- your member card beside them as the live preview (your name on it as you
-- type it), its address and the ways to share it under it. Someone with no
-- card yet sees the card's outline in the same place, and the way to one,
-- which is a username. Too narrow for two columns, the card stands over
-- the fields.
--
-- Your name is the account's (`PATCH /auth/me`; the card's name follows),
-- your title and your photo the edge's (`PATCH /auth/me {title}`,
-- `PUT /auth/me/photo`: a square you place in a picture of yours, shown
-- everywhere your face is). Deleting the account is the last row, behind
-- typing your address.
-- The username is chosen once. Under who you are, your account: whom
-- you're signed in as and in which organization, the computers your
-- sessions sync across, and signing out (out of plugins' reach: it ends the
-- engine's session and opens setup again). Settings' old Account page is
-- this one now: `settings/account` lands here, on the account's rows.

local host = require("@app/screens/settings/host")
local ds = require("@ds")
local api = require("@account/api")
local auth = require("@account/auth")
local sessions = require("@account/sessions")
local CardActions = require("@ds/card_actions")
local store = require("@app/screens/settings/store")
local search = require("@app/screens/settings/search")
local visits = require("@app/screens/settings/visits")

local M = {}

M.ID = "profile"
-- (Its id stays `profile`: the sidebar foot's you opens and selects it.)
M.TITLE = "Account"

search.index(M.ID, ui.t("settings.profile.title"), ui.t("settings.profile.terms"), {
	{ key = "photo", title = ui.t("settings.profile.photo.label"), terms = ui.t("settings.profile.photo.terms") },
	{ key = "name", title = ui.t("settings.profile.name.label"), terms = ui.t("settings.profile.name.terms") },
	{ key = "title", title = ui.t("settings.profile.job_title.label"), terms = ui.t("settings.profile.job_title.terms") },
	{ key = "username", title = ui.t("settings.profile.username.label"), terms = ui.t("settings.profile.username.terms") },
	{ key = "email", title = ui.t("settings.profile.email.label"), terms = ui.t("settings.profile.email.terms") },
	{ key = "card", title = ui.t("settings.profile.card.label"), terms = ui.t("settings.profile.card.terms") },
	{ key = "devices", title = ui.t("settings.profile.devices.label"), terms = ui.t("settings.profile.devices.terms") },
	{ key = "signing_keys", title = ui.t("settings.profile.keys.label"), terms = ui.t("settings.profile.keys.terms") },
	{ key = "sign_out", title = ui.t("settings.profile.sign_out.label"), terms = ui.t("settings.profile.sign_out.terms") },
	{ key = "delete_account", title = ui.t("settings.profile.delete.label"), terms = ui.t("settings.profile.delete.terms") },
})

local toast = require("@ds/toast")
local signing_keys = require("@app/screens/settings/signing_keys")
local function toast_failed(title: string, body: string)
	toast.show { title = title, body = body, tone = "danger" }
end

-- Signing out, once asked (the row holds still while it goes).
local leaving = ui.state("settings_signing_out", false)
local confirming = ui.state("settings_sign_out_confirm", false)
local function sign_out()
	if leaving:peek() then
		return
	end
	leaving:set(true)
	auth.sign_out(function()
		store.forget()
		leaving:set(false)
		-- (The shell may ask too, as it sees this window signed out: the host opens
		-- setup once a window.)
		host.invoke("app.onboard")
	end)
end

-- The card beside the fields: a card's 300 by 420 at this, its column as
-- wide as it.
local ZOOM = 0.6
local W, H = 300, 420
local CARD_W = W * ZOOM

-- The name being typed, and whether it's been touched since it was loaded
-- (until then it follows the account's).
local draft = ui.state("settings_name_draft", "")
local touched = ui.state("settings_name_touched", false)
local saving = ui.state("settings_name_saving", false)
local shared = ui.state("settings_card_shared", "ready")

local function name_of(me: store.Me?, u: auth.User?): string
	if me and me.name and me.name ~= "" then
		return me.name
	end
	return if u and u.first_name then u.first_name else ""
end
local function saved_name(): string
	return name_of(store.me:get(), auth.user:get())
end

-- Untouched, the field follows the account's name; typed in, it's yours
-- until saved.
ui.effect(function()
	local n = saved_name()
	if not touched:peek() and draft:peek() ~= n then
		draft:set(n)
	end
end)
ui.effect(function()
	local d = draft:get()
	if d ~= name_of(store.me:peek(), auth.user:peek()) and not touched:peek() then
		touched:set(true)
	end
end)

local function name_error(): string?
	if not touched:get() then
		return nil
	end
	local n = draft:get()
	if not string.match(n, "%S") then
		return ui.t("settings.profile.name.empty"):get()
	elseif #n > 80 then
		return ui.t("settings.profile.name.too_long"):get()
	end
	return nil
end

-- Your title, typed the same way: it follows the account's until touched.
local title_draft = ui.state("settings_title_draft", "")
local title_touched = ui.state("settings_title_touched", false)
local function saved_title(): string
	local me = store.me:get()
	return if me and me.title then me.title else ""
end
ui.effect(function()
	local t = saved_title()
	if not title_touched:peek() and title_draft:peek() ~= t then
		title_draft:set(t)
	end
end)
ui.effect(function()
	local d = title_draft:get()
	local me = store.me:peek()
	if d ~= (if me and me.title then me.title else "") and not title_touched:peek() then
		title_touched:set(true)
	end
end)
local function title_error(): string?
	return if #(string.match(title_draft:get(), "^%s*(.-)%s*$") or "") > 80 then ui.t("settings.profile.job_title.too_long"):get() else nil
end

local function trimmed(s: string): string
	return string.match(s, "^%s*(.-)%s*$") or ""
end
local function name_changed(): boolean
	return trimmed(draft:get()) ~= saved_name()
end
local function title_changed(): boolean
	return trimmed(title_draft:get()) ~= saved_title()
end
local function changed(): boolean
	return name_changed() or title_changed()
end

local function save()
	if saving:peek() or name_error() or title_error() or not changed() then
		return
	end
	saving:set(true)
	-- Each that changed; saved once all have answered.
	local waiting = 0
	local function one(was_touched: State<boolean>): (ok: boolean) -> ()
		waiting += 1
		return function(ok: boolean)
			waiting -= 1
			if ok then
				was_touched:set(false)
			end
			if waiting == 0 then
				saving:set(false)
			end
		end
	end
	local name_done = if name_changed() then one(touched) else nil
	local title_done = if title_changed() then one(title_touched) else nil
	if name_done then
		store.rename(trimmed(draft:peek()), name_done)
	end
	if title_done then
		store.set_title(trimmed(title_draft:peek()), title_done)
	end
end

-- Your photo: a picture of yours, the square of it you place (dragged,
-- zoomed) in a circle `VIEW` wide, cut by the host and kept by the edge.
-- Where the square is: its centre in the picture (0 to 1 each way), and how
-- far in (1: the picture's short side fills the circle).
local VIEW = 240
local cropping = ui.state("settings_photo_cropping", false)
local crop_path = ui.state("settings_photo_path", "")
local crop_px = ui.state("settings_photo_px", { w = 1, h = 1 })
local zoom = ui.state("settings_photo_zoom", 1)
local centre = ui.state("settings_photo_centre", { x = 0.5, y = 0.5 })
local uploading = ui.state("settings_photo_uploading", false)

-- Points one of the picture's pixels takes in the circle (tracked).
local function scale(): number
	local px = crop_px:get()
	return VIEW * zoom:get() / math.max(1, math.min(px.w, px.h))
end
-- A centre moved back to where the picture still fills the circle.
local function placed(x: number, y: number): (number, number)
	local px = crop_px:peek()
	local k = VIEW * zoom:peek() / math.max(1, math.min(px.w, px.h))
	local hx = VIEW / (2 * px.w * k)
	local hy = VIEW / (2 * px.h * k)
	return math.clamp(x, hx, 1 - hx), math.clamp(y, hy, 1 - hy)
end
local function settle()
	local c = centre:peek()
	local x, y = placed(c.x, c.y)
	centre:set({ x = x, y = y })
end
-- The square as the host cuts it: its corner and side, in pixels.
function M.crop(): { x: number, y: number, side: number }
	local px, c = crop_px:peek(), centre:peek()
	local side = math.max(1, math.min(px.w, px.h)) / zoom:peek()
	return { x = c.x * px.w - side / 2, y = c.y * px.h - side / 2, side = side }
end

local function choose_photo()
	host.open_files({
		title = ui.t("settings.profile.photo.choose"):get(),
		filters = { { ui.t("settings.profile.photo.images"):get(), { "png", "jpg", "jpeg" } } },
	}):after(function(files: { File })
		local f = files[1]
		if not f then
			return
		end
		host.call("account.photo_size", { path = f.path }):after(function(size: any)
			ui.batch(function()
				crop_path:set(f.path)
				crop_px:set({ w = size.w, h = size.h })
				zoom:set(1)
				centre:set({ x = 0.5, y = 0.5 })
				cropping:set(true)
			end)
		end, function()
			toast_failed(ui.t("settings.profile.photo.open_failed"):get(), ui.t("settings.profile.photo.open_failed_body"):get())
		end)
	end)
end

local function use_photo()
	if uploading:peek() then
		return
	end
	uploading:set(true)
	store.set_photo(crop_path:peek(), M.crop(), function(ok: boolean)
		uploading:set(false)
		if ok then
			cropping:set(false)
		end
	end)
end

-- The circle: the picture under it, dragged to place and zoomed with the
-- slider (or the wheel).
local function cropper(): Node
	local from: { x: number, y: number, cx: number, cy: number }? = nil
	return ui.column {
		w_full = true,
		items = "center",
		gap = "$space.4",
		ui.div {
			id = "photo-crop",
			w = VIEW,
			h = VIEW,
			shrink = 0,
			relative = true,
			rounded = "$radius.full",
			overflow = "hidden",
			bg = "$zeron.secondary.bg",
			cursor = "grab",
			role = "img",
			label = ui.t("settings.profile.photo.preview"),
			on_press = function(e: PointerEvent)
				local c = centre:peek()
				from = { x = e.x, y = e.y, cx = c.x, cy = c.y }
			end,
			on_pointer_move = function(e: PointerEvent)
				local f = from
				if not f then
					return
				end
				local px = crop_px:peek()
				local k = VIEW * zoom:peek() / math.max(1, math.min(px.w, px.h))
				local x, y = placed(f.cx - (e.x - f.x) / (px.w * k), f.cy - (e.y - f.y) / (px.h * k))
				centre:set({ x = x, y = y })
			end,
			on_release = function()
				from = nil
			end,
			on_wheel = function(e: any): boolean
				zoom:set(math.clamp(zoom:peek() - (e.dy or 0) * 0.005, 1, 4))
				settle()
				return true
			end,
			ui.image {
				src = function(): string
					return crop_path:get()
				end,
				absolute = true,
				left = function(): number
					return VIEW / 2 - centre:get().x * crop_px:get().w * scale()
				end,
				top = function(): number
					return VIEW / 2 - centre:get().y * crop_px:get().h * scale()
				end,
				w = function(): number
					return crop_px:get().w * scale()
				end,
				h = function(): number
					return crop_px:get().h * scale()
				end,
				aria_hidden = true,
			},
		},
		ds.Slider { value = zoom, min = 1, max = 4, step = 0.01, label = ui.t("settings.profile.photo.zoom"), w = VIEW, on_change = settle },
	}
end

-- Deleting the account: the dialog's word to type is your address.
local deleting = ui.state("settings_deleting_account", false)
local delete_busy = ui.state("settings_delete_busy", false)
-- Waiting for you to sign in again in the browser: only just after
-- signing in can the account be deleted (no agent on this Mac can).
local delete_signing_in = ui.state("settings_delete_signing_in", false)
local function delete_account(email: string)
	if delete_busy:peek() then
		return
	end
	delete_busy:set(true)
	store.delete_account(email, function(word: string?)
		delete_signing_in:set(false)
		if word == "cancelled" then
			-- (Closed while signing in: you stopped it; nothing to say.)
			delete_busy:set(false)
			return
		end
		if word then
			delete_busy:set(false)
			deleting:set(false)
			toast_failed(ui.t("settings.profile.delete.failed"):get(), store.says(word))
			return
		end
		-- Gone: signed out here too, and setup opens again.
		auth.sign_out(function()
			store.forget()
			delete_busy:set(false)
			deleting:set(false)
			host.invoke("app.onboard")
		end)
	end, function()
		delete_signing_in:set(true)
	end)
end
-- Closed while waiting for that sign-in (Cancel, Escape): stop waiting.
ui.effect(function()
	if not deleting:get() and delete_signing_in:peek() then
		store.cancel_delete()
	end
end)

-- Your photo's address now ("" none): the profile's, else the one kept.
local function photo(): string
	local me = store.me:get()
	if me and me.avatar then
		return me.avatar
	end
	return store.photo()
end
-- Whether it's one you chose (the edge's), not your sign-in's.
local function own_photo(): boolean
	return string.find(photo(), "/auth/avatars/", 1, true) ~= nil
end

-- What the card says as you type: the first word of the name.
local function card_name(): string
	local first = string.match(draft:get(), "^%s*(%S+)")
	if first then
		return first
	end
	local c = store.card:get()
	return if c then c.first_name else "—"
end

-- The card as it will be, its address, and the ways to share it in a
-- quiet pill of glyphs as wide as the card.
local function preview(): Node
	return ui.column {
		w = CARD_W,
		shrink = 0,
		gap = "$space.3",
		items = "center",
		ui.div {
			zoom = ZOOM,
			shrink = 0,
			-- (Its face is fixed as it's built: built again only if the card
			-- itself is another, keyed by its number.)
			ui.presence(function(): number
				local c = store.card:get()
				return if c then c.number else 0
			end, function(_: number): Node
				local c = store.card:peek()
				if not c then
					return ui.div {}
				end
				local f = api.member(c) :: any
				f.name = card_name
				return ds.MemberCard(f)
			end),
		},
		ui.text {
			function()
				local u = store.username:get()
				return if u then api.address(u) else ""
			end,
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			color = "$text.tertiary",
		},
		CardActions {
			state = shared,
			form = "glyphs",
			w = CARD_W,
			on_share = function()
				local u = store.username:peek()
				if u then
					local text = ui.t("settings.profile.card.share_text"):get()
					local function esc(s: string): string
						return (string.gsub(s, "[^%w%-%._~]", function(ch: string): string
							return string.format("%%%02X", string.byte(ch))
						end))
					end
					host.invoke("open_url", `https://x.com/intent/post?text={esc(text)}&url={esc(api.link(u))}`)
				end
			end,
			on_copy = function()
				local u = store.username:peek()
				if u then
					ui.clipboard.write(api.link(u))
				end
			end,
		},
	}
end

-- Claiming a username, in a dialog: the name typed, the address it makes,
-- whether it's free as it's typed, and Claim.
local claiming = ui.state("settings_claiming", false)
local wanted = ui.state("settings_claim_name", "")
local claim_status = ui.state("settings_claim_status", "idle")
local claim_error = ui.state("settings_claim_error", nil :: string?)
local checked_for = ""
local function check_now()
	local u = api.normalise(wanted:peek())
	claim_error:set(nil)
	if api.problem(u) then
		claim_status:set("idle")
		return
	end
	claim_status:set("checking")
	checked_for = u
	api.check(u):after(function(c: api.Check)
		if checked_for == u then
			claim_status:set(if c.available then "free" else "taken")
			if not c.available then
				claim_error:set(api.says(c.reason))
			end
		end
	end, function()
		if checked_for == u then
			claim_status:set("idle")
		end
	end)
end
local function claim()
	local u = api.normalise(wanted:peek())
	if api.problem(u) then
		claim_error:set(ui.t("settings.profile.claim.invalid"):get())
		return
	end
	claim_status:set("claiming")
	api.claim(u):after(function(card: api.Card)
		store.username:set(u)
		store.card:set(card)
		claim_status:set("idle")
		claiming:set(false)
	end, function(err: any)
		claim_status:set("idle")
		claim_error:set(api.says(err.message))
	end)
end
local function claim_dialog(): Node
	return ds.Dialog {
		open = claiming,
		title = ui.t("settings.profile.claim.title"),
		body = ui.t("settings.profile.claim.body"),
		size = "md",
		confirm = { label = ui.t("settings.profile.claim.confirm"), on_click = claim, keep_open = true },
		ui.column {
			w_full = true,
			gap = "$space.2",
			ds.TextField {
				value = wanted,
				name = ui.t("settings.profile.username.label"),
				placeholder = ui.t("settings.profile.claim.placeholder"),
				w_full = true,
				autofocus = true,
				error = function()
					return claim_error:get()
				end,
				on_commit = function()
					claim()
				end,
			},
			ds.AddressLine {
				prefix = "zeron.sh/c/",
				value = function()
					return api.normalise(wanted:get())
				end,
				status = function()
					return claim_status:get()
				end,
			},
		},
	}
end
-- Whether what's typed is free, asked 300 ms after the typing stops, and
-- only while the dialog is open: never on a launch or a reload alone. (The
-- timer is this run's: a keystroke's run ends the one before.)
ui.effect(function()
	wanted:get()
	if claiming:get() then
		ui.after(300, check_now)
	end
end)

-- No card yet: its outline in the same place, and the way to one.
local function missing(): Node
	return ui.column {
		w = CARD_W,
		shrink = 0,
		gap = "$space.3",
		items = "center",
		ui.div {
			w = CARD_W,
			h = H * ZOOM,
			shrink = 0,
			p = "$space.3",
			rounded = "$zeron.card.radius",
			border = 1,
			border_dash = { 4, 4 },
			border_color = "$border.strong",
			ui.text { "zeron", font = "$font.sans", weight = "$font.weight.bold", text_size = "$font.size.xs", color = "$text.disabled" },
		},
		ui.text { ui.t("settings.profile.card.none"), text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.tertiary" },
		ds.Button {
			ui.t("settings.profile.claim.open"),
			tone = "secondary",
			size = "sm",
			on_click = function()
				claim_error:set(nil)
				claiming:set(true)
			end,
		},
	}
end

-- The card's column: loading (its outline, quiet), the card, or the way to
-- one.
local function aside(): Node
	return search.anchor(
		"card",
		-- (The card's own width, wherever it stands: beside the fields, or
		-- centred over them.)
		ui.column { w = CARD_W, shrink = 0, ui.presence(function(): string
			local s = store.state("card")
			if s ~= "ready" and store.card:get() == nil then
				return "waiting"
			end
			return if store.card:get() then "card" else "none"
		end, function(k: string): Node
			if k == "card" then
				return preview()
			elseif k == "none" then
				return missing()
			end
			return ui.div { w = CARD_W, h = H * ZOOM, rounded = "$zeron.card.radius", bg = "$zeron.secondary.bg" }
		end, { enter = { opacity = { 0, 1 }, duration = 160 }, exit = { opacity = 0, duration = 80 }, mode = "wait" }) },
		true
	)
end

local function row(key: string, props: any): Node
	return search.anchor(key, ds.Settings.Row(props))
end

-- Signed out: the page as it will be, with nothing to fill in. The card
-- yours will be, made out to you, where your card stands once you're in;
-- what an account gives, as the rows the page will hold; and the one way
-- in, where Save changes stands (setup, as signing out leaves you).
local PREVIEW_SEED = "zeron-account-preview"
local function today(): string
	local d = os.date("*t") :: any
	return api.since(string.format("%04d-%02d-%02d", d.year, d.month, d.day))
end
local function signed_out(narrow: () -> boolean): Node
	local function gives(icon: string, label: Message, description: Message): Node
		return ds.Settings.Row { icon = icon :: any, label = label, description = description }
	end
	return ds.Settings.Page {
		title = ui.t("settings.profile.title"),
		description = ui.t("settings.profile.signed_out.description"),
		aside = ui.column {
			w = CARD_W,
			shrink = 0,
			gap = "$space.3",
			items = "center",
			ui.div {
				zoom = ZOOM,
				shrink = 0,
				ds.MemberCard { number = 0, name = "You", since = today(), seed = PREVIEW_SEED, arrive = true },
			},
			ui.text {
				ui.t("settings.profile.signed_out.yours"),
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = "$text.tertiary",
			},
		},
		aside_w = CARD_W,
		aside_top = narrow,
		ds.Settings.Section {
			ds.Settings.Group {
				gives("user-rounded", ui.t("settings.profile.signed_out.name"), ui.t("settings.profile.signed_out.name_description")),
				gives("tag", ui.t("settings.profile.signed_out.card"), ui.t("settings.profile.signed_out.card_description")),
				gives("smartphone", ui.t("settings.profile.signed_out.sessions"), ui.t("settings.profile.signed_out.sessions_description")),
			},
			ui.row {
				w_full = true,
				justify = "end",
				pt = "$space.2",
				ds.Button {
					ui.t("settings.profile.signed_out.sign_in"),
					on_click = function()
						host.invoke("app.onboard")
					end,
				},
			},
		},
	}
end

local device_icon = require("@app/screens/settings/devices").icon

-- The page's width, kept so a rebuild knows it before it's measured.
local width = ui.state("settings_profile_w", 720)

function M.view(): Node
	visits.once("profile", function()
		store.load_me()
		store.load_card()
		if auth.signed_in:peek() then
			signing_keys.load(true)
		end
		if store.org:peek() == nil then
			store.load_org()
		end
	end)
	local room = ui.ref()
	ui.effect(function()
		local el = room:get()
		local w = room.width
		if el and w and w > 0 and math.abs(w - width:peek()) > 1 then
			width:set(w)
		end
	end)
	-- Two columns only while the fields keep a comfortable width beside
	-- the card (about 416 of a 620 page, the card's 180 and the gap);
	-- narrower, the card stands over them.
	local TWO_COLUMNS = 620
	local function narrow(): boolean
		return width:get() < TWO_COLUMNS
	end
	-- Where you're signed in: your devices, not Cloud sessions' machines.
	local devices = ui.computed(function(): { sessions.Device }
		return sessions.computers(sessions.now:get().devices)
	end)
	return ui.column {
		ref = room,
		w_full = true,
		items = "center",
		-- (The card moves between beside the fields and over them as the
		-- width crosses two columns' worth, in place: the page is built
		-- once, so a field being typed in keeps its focus.)
		-- Signed out: nothing to fill in, only the way in.
		ui.show(function(): boolean
			return not auth.signed_in:get()
		end, signed_out(narrow)),
		ui.show(auth.signed_in, ds.Settings.Page {
				title = ui.t("settings.profile.title"),
				description = function()
					local o = store.org:get()
					return if o
						then ui.t("settings.profile.description_org", { org = o.name }):get()
						else ui.t("settings.profile.description_team"):get()
				end,
				aside = aside(),
				aside_w = CARD_W,
				aside_top = narrow,
				ds.Settings.Section {
					ds.Settings.Group {
						row("photo", {
							label = ui.t("settings.profile.photo.label"),
							-- (Its buttons stay at the end beside it; a line too
							-- long fades instead.)
							truncate = true,
							description = function(): string
								return if own_photo()
									then ui.t("settings.profile.photo.own"):get()
									else ui.t("settings.profile.photo.from_sign_in"):get()
							end,
							-- (Yours: another account signed in is another face, the
							-- one swap; its seed is fixed as it's made.)
							lead = ui.presence(function(): string
								local u = auth.user:get()
								return if u then u.id else "you"
							end, function(seed: string): Node
								return ds.Avatar {
									seed = seed,
									size = 40,
									label = ui.t("settings.profile.photo.avatar"),
									src = function(): string
										return store.photo_file(photo())
									end,
								}
							end),
							control = ui.show(auth.signed_in, ui.row {
								gap = "$space.2",
								items = "center",
								ui.show(own_photo, ds.Button {
									ui.t("common.remove"),
									tone = "ghost",
									size = "sm",
									on_click = function()
										store.set_photo(nil, nil)
									end,
								}),
								ds.Button { ui.t("settings.profile.photo.change"), tone = "secondary", size = "sm", on_click = choose_photo },
							}),
						}),
						row("name", {
							label = ui.t("settings.profile.name.label"),
							description = ui.t("settings.profile.name.description"),
							stacked = true,
							control = ds.TextField {
								value = draft,
								name = ui.t("settings.profile.name.label"),
								w_full = true,
								error = name_error,
								on_commit = function()
									save()
								end,
							},
						}),
						row("title", {
							label = ui.t("settings.profile.job_title.label"),
							description = ui.t("settings.profile.job_title.description"),
							stacked = true,
							control = ds.TextField {
								value = title_draft,
								name = ui.t("settings.profile.job_title.label"),
								placeholder = ui.t("settings.profile.job_title.placeholder"),
								w_full = true,
								error = title_error,
								disabled = function(): boolean
									return not auth.signed_in:get()
								end,
								on_commit = function()
									save()
								end,
							},
						}),
						row("username", {
							label = ui.t("settings.profile.username.label"),
							description = function()
								local u = store.username:get()
								if u then
									return api.address(u)
								end
								return if store.state("card") == "ready" then ui.t("settings.profile.username.not_chosen"):get() else "…"
							end,
						}),
					},
					ui.row {
						w_full = true,
						justify = "end",
						pt = "$space.2",
						ds.Button {
							ui.t("settings.profile.save"),
							busy = saving,
							disabled = function()
								-- (Not while saving: busy shows the spinner, undimmed.)
								return not saving:get() and (name_error() ~= nil or title_error() ~= nil or not changed())
							end,
							on_click = save,
						},
					},
				},
				-- Your account: whom you're signed in as, where your sessions
				-- sync.
				search.anchor(
					"email",
					ds.Settings.Section {
						title = ui.t("settings.profile.email.label"),
						ds.Settings.Group {
							ds.Settings.Row {
								label = function(): string
									local me = store.me:get()
									local u = auth.user:get()
									return (me and me.email) or (u and u.email) or ui.t("settings.profile.email.not_signed_in"):get()
								end,
								private_label = true,
								description = function(): string
									if not auth.signed_in:get() then
										return ui.t("settings.profile.email.local"):get()
									end
									local o = store.org:get()
									return if o
										then ui.t("settings.profile.email.org", { org = o.name }):get()
										else ui.t("settings.profile.email.sync"):get()
								end,
								icon = "user-rounded",
								truncate = true,
							},
						},
					}
				),
				search.anchor(
					"devices",
					ds.Settings.Section {
						title = ui.t("settings.profile.devices.label"),
						ds.Settings.Each(devices, function(_d: sessions.Device, _i: number, d: State<sessions.Device>): Node
							return ds.Settings.Row {
								label = function()
									return d:get().name
								end,
								description = function()
									local p = d:get().platform
									return if p == "macos" then "macOS" elseif p == "linux" then "Linux" elseif p == "ios" then "iOS" else p
								end,
								icon = device_icon(d:peek().platform),
								badge = if d:peek().here then ds.Badge { ui.t("settings.profile.devices.this") } else nil,
							}
						end, {
							key = function(d: sessions.Device): string
								return d.id
							end,
						}),
					}
				),
				-- The keys your devices sign what you make with.
				search.anchor("signing_keys", signing_keys.section()),
				-- Signing out, last: out of plugins' reach.
				ui.column {
					hook = false,
					w_full = true,
					search.anchor(
						"sign_out",
						ds.Settings.Section {
							ds.Settings.Group {
								-- (One child: the Group rules between its children, and Delete
								-- account brings its own.)
								ui.column {
									w_full = true,
									ds.Settings.Row {
										label = function(): string
											return if leaving:get()
												then ui.t("settings.profile.sign_out.busy"):get()
												else ui.t("settings.profile.sign_out.label"):get()
										end,
										description = ui.t("settings.profile.sign_out.description"),
										-- (Signed out, the page is the way in instead.)
										danger = true,
										on_click = function()
											if not leaving:peek() then
												confirming:set(true)
											end
										end,
									},
									search.anchor("delete_account", ui.column {
										w_full = true,
										ds.Settings.Rule(),
										ds.Settings.Row {
											label = function(): string
												return if delete_busy:get()
													then ui.t("settings.profile.delete.busy"):get()
													else ui.t("settings.profile.delete.label"):get()
											end,
											description = ui.t("settings.profile.delete.description"),
											danger = true,
											on_click = function()
												if not delete_busy:peek() then
													deleting:set(true)
												end
											end,
										},
									}),
								},
							},
						}
					),
				},
			}),
		claim_dialog(),
		ds.Dialog {
			open = confirming,
			title = ui.t("settings.profile.sign_out.confirm_title"),
			body = ui.t("settings.profile.sign_out.confirm_body"),
			tone = "danger",
			confirm = { label = ui.t("settings.profile.sign_out.label"), on_click = sign_out },
		},
		ds.Dialog {
			open = cropping,
			title = ui.t("settings.profile.crop.title"),
			body = ui.t("settings.profile.crop.body"),
			size = "sm",
			confirm = { label = ui.t("settings.profile.crop.confirm"), keep_open = true, on_click = use_photo },
			cropper(),
		},
		-- (The word to type is your address: a new dialog for another
		-- account, the one swap.)
		ui.presence(function(): string
			local me = store.me:get()
			local u = auth.user:get()
			return (me and me.email) or (u and u.email) or ""
		end, function(email: string): Node
			local o = store.org:peek()
			return ds.Dialog {
				open = deleting,
				title = ui.t("settings.profile.delete.confirm_title"),
				body = if o
					then ui.t("settings.profile.delete.confirm_body_org", { org = o.name })
					else ui.t("settings.profile.delete.confirm_body"),
				tone = "danger",
				size = "md",
				confirm_text = email,
				confirm = {
					label = ui.t("settings.profile.delete.label"),
					keep_open = true,
					busy = function(): boolean
						return delete_busy:get()
					end,
					on_click = function()
						delete_account(email)
					end,
				},
				-- Signing in again, in the browser, before it goes.
				ui.show(
					delete_signing_in,
					ui.column {
						w_full = true,
						gap = "$space.1",
						ui.text {
							ui.t("settings.profile.delete.step_up.title"),
							text_size = "$font.size.md",
							line_height = 20,
							weight = "$font.weight.medium",
							color = "$text.primary",
							wrap = true,
						},
						ui.text {
							ui.t("settings.profile.delete.step_up.body"),
							text_size = "$font.size.sm",
							line_height = "$font.line.sm",
							color = "$text.secondary",
							wrap = true,
						},
					}
				),
			}
		end),
	}
end

return M
