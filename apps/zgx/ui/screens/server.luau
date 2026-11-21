-- The server an account lives on (docs/self-hosting.md): Zeron's, or one
-- someone runs themselves. One dialog, the same from setup's Sign in and
-- from Settings → Account: its title, a quiet line, two choices (Zeron, its
-- host under it; Self-hosted), the address under them, the guide to running
-- one, Cancel and the way on.
--
-- The address is looked up before anything changes: the way on spins, the
-- dialog holds still, and a server that won't do says why under the field,
-- the address as typed. One that will is used at once while signed out.
-- Signed in, an account is one server's: the chooser gives way to a
-- question naming the server found ("Switch to Acme?"), red, as signing
-- out is, and only its answer signs out and switches.
--
-- It's asked for from elsewhere too (`server.ask`): File → Connect to
-- Server…, the welcome's server, and an invite, a server's link
-- (`zeron://server?address=…`, src/links.rs; the page a self-hosted server
-- serves at its address hands it out). An invite opens the dialog on its
-- address, already looked up, and stops there: the way on names the
-- server found ("Connect to Acme") and only a click on it changes
-- anything. A link never moves an account on its own.
--
--   local server = require("@screens/server")
--   server.Dialog { open = state, on_changed = function(signed_out) … end }
--   server.ask()                    -- open it, as the account stands
--   server.ask("zeron.acme.com")    -- open it on an invite's address

local ds = require("@ds")
local auth = require("@account/auth")

local M = {}

export type Props = {
	open: State<boolean>,
	-- Heard once the server changed: `signed_out`, the account here was
	-- signed out for it (setup is the way back in).
	on_changed: ((signed_out: boolean) -> ())?,
}

-- Whether choosing can't happen here: a launch's environment named the
-- server (the host refuses a change; Settings and Sign in say so instead
-- of offering it).
function M.fixed(): boolean
	return auth.server:get().kind == "environment"
end

-- The server as a pill over Sign in's title names it (onboarding/signin's
-- `Server`): nil for Zeron's, which goes unsaid; its name and, unless the
-- name is the address already, the address; `fixed`, named by a launch's
-- environment. Tracked.
export type Identity = { name: string, host: string?, fixed: boolean }
function M.identity(): Identity?
	local s = auth.server:get()
	if s.kind == "zeron" then
		return nil
	end
	local named = s.name ~= nil and s.name ~= ""
	return { name = auth.server_label(s), host = if named then s.host else nil, fixed = s.kind == "environment" }
end

local function trim(s: string): string
	return (string.gsub(s, "^%s*(.-)%s*$", "%1"))
end

-- What was asked for from outside the dialog, for the one in the window to
-- take (setup's, or Settings → Account's, whichever is built; asked before
-- it is, it's taken as it's built): `seq` moving with each ask, and an
-- invite's address. Taken once each, never again on a reload.
export type Ask = { seq: number, address: string? }
local asked = ui.state("server_asked", { seq = 0 } :: Ask)
local taken = ui.state("server_asked_taken", 0)

-- Opens the dialog: as the account stands, or on an invite's `address`.
-- (Nothing happens while a launch's environment fixes the server.)
function M.ask(address: string?)
	asked:set({ seq = asked:peek().seq + 1, address = address })
end

-- What the field starts with: the server's host, as it would be typed, or
-- its whole address where the host alone wouldn't find it (http on the
-- local network); empty for Zeron's.
local function typed(server: auth.Server): string
	if server.kind ~= "self_hosted" then
		return ""
	end
	return if server.url == `https://{server.host}` then server.host else server.url
end

-- A line of trouble, as a field's error reads (ds/field), for what isn't
-- the address's: the switch itself refused.
local function trouble(says: () -> string): Node
	return ui.show(function(): boolean
		return says() ~= ""
	end, ui.row {
		gap = "$space.1",
		items = "center",
		color = "$zeron.tone.danger.ink",
		live = "polite",
		ui.div { y = -0.5, ds.Icon { name = "danger-triangle", size = 12 } },
		ui.text { says, text_size = "$font.size.sm", line_height = "$font.line.sm", wrap = true },
	})
end

M.Dialog = ui.component(function(props: Props): Node
	local open = props.open
	-- "zeron" | "self_hosted"
	local choice = ui.use_state("zeron")
	local address = ui.use_state("")
	-- "looking" (the address, up: Cancel gives it up) or "using" (the server
	-- changing: the dialog holds still until it has); "" while neither.
	local busy = ui.use_state("")
	-- Why it didn't go: under the field, or (for Zeron's) under the choices.
	local why = ui.use_state("")
	local input = ui.ref()

	-- Signed in: the question, and what it would switch to.
	local asking = ui.use_state(false)
	local switching = ui.use_state(false)
	local switch_why = ui.use_state("")
	-- (A record, kept whole: `found` nil is Zeron's.)
	local target = ui.use_state(nil :: { found: auth.Found?, label: string }?)

	-- An invite's address, for the dialog to open on ("" for none), and
	-- the server it found while signed out: shown, waiting on the click
	-- that uses it (the way on reads "Connect to Acme").
	local invite = ui.use_state("")
	local ready = ui.use_state(nil :: { found: auth.Found?, label: string }?)
	-- The address the last lookup was for: `why` and `ready` are its answer,
	-- and stand until the field reads otherwise.
	local looked = ui.use_state("")

	local function focus_now()
		local el = input:peek()
		if el then
			el:focus()
		end
	end
	-- (Once what changed it has been drawn: an address just enabled.)
	local function focus_field()
		ui.after(0, focus_now)
	end

	-- Whether the account here would be signed out by a switch (one kept,
	-- maybe still being read).
	local function signed_in(): boolean
		local s = auth.state:peek()
		return s == "signed_in" or s == "restoring"
	end

	local function switched(signed_out: boolean)
		if props.on_changed then
			props.on_changed(signed_out)
		end
	end

	-- Uses `found` (nil: Zeron's) while signed out; signed in, asks first.
	local function go_to(found: auth.Found?, label: string)
		if signed_in() then
			target:set({ found = found, label = label })
			busy:set("")
			open:set(false)
			asking:set(true)
			return
		end
		busy:set("using")
		auth.use_server(found, function(failure: string?)
			busy:set("")
			if failure then
				why:set(auth.server_says({ error = failure, host = label, message = "" }))
				return
			end
			open:set(false)
			switched(false)
		end)
	end

	-- Looks `typed_address` up and goes on to what it found. An invite's
	-- (`invited`) stops short while signed out: the server found is shown
	-- on the way on, and used only when that's clicked. (Signed in, the
	-- question that comes next is the click.)
	local function look_up(typed_address: string, invited: boolean)
		busy:set("looking")
		looked:set(typed_address)
		auth.find_server(typed_address, function(found: auth.Found?, not_found: auth.NotFound?)
			if not found then
				busy:set("")
				why:set(auth.server_says(not_found :: auth.NotFound))
				focus_field()
				return
			end
			-- (An invite to Zeron's own shows it as its choice.)
			if invited and found.zeron then
				choice:set("zeron")
			end
			-- Where it is already: nothing to change. (An invite to it stays
			-- open, showing it chosen, rather than flashing shut.)
			if found.current then
				busy:set("")
				if not invited then
					open:set(false)
					return
				end
				focus_field()
				return
			end
			-- Zeron's own, typed: the same as choosing it.
			local to = if found.zeron then nil else found
			local label = if found.zeron
				then auth.server_label(nil)
				elseif found.name and found.name ~= "" then found.name
				else found.host
			if invited and not signed_in() then
				busy:set("")
				ready:set({ found = to, label = label })
				-- (On the address, Enter the way on.)
				focus_field()
				return
			end
			go_to(to, label)
		end)
	end

	-- What the dialog opens on: an invite's address, looked up at once, or
	-- the server as the account stands.
	local function prefill()
		local invited = invite:peek()
		invite:set("")
		auth.forget_lookup()
		busy:set("")
		why:set("")
		ready:set(nil)
		looked:set("")
		if invited == "" then
			local s = auth.server:peek()
			choice:set(if s.kind == "self_hosted" then "self_hosted" else "zeron")
			address:set(typed(s))
			return
		end
		choice:set("self_hosted")
		address:set(invited)
		look_up(invited, true)
	end

	-- Opened: as the account stands, or on an invite. Closed: nothing typed
	-- or found is kept, and a lookup on its way is heard by no one.
	ui.effect(function()
		if open:get() then
			prefill()
		else
			auth.forget_lookup()
			busy:set("")
			ready:set(nil)
			why:set("")
		end
	end)
	-- Another address typed: the last answer, and a server an invite found,
	-- no longer stand. (Not the one just looked up: its answer, however soon
	-- it came, is for it.)
	ui.effect(function()
		if trim(address:get()) == looked:peek() then
			return
		end
		looked:set("")
		why:set("")
		ready:set(nil)
	end)
	-- Asked for from elsewhere (`M.ask`): taken once. An invite while the
	-- dialog is already open fills it in again.
	ui.effect(function()
		local a = asked:get()
		if a.seq == 0 or a.seq == taken:peek() then
			return
		end
		taken:set(a.seq)
		if auth.server:peek().kind == "environment" then
			return
		end
		local invited = trim(a.address or "")
		if open:peek() then
			if invited ~= "" then
				invite:set(invited)
				prefill()
			end
			return
		end
		invite:set(invited)
		open:set(true)
	end)
	ui.effect(function()
		if not asking:get() then
			switching:set(false)
			switch_why:set("")
		end
	end)

	local function connect()
		if busy:peek() ~= "" then
			return
		end
		why:set("")
		-- An invite's server, found: the way on is to it.
		local r = ready:peek()
		if r then
			go_to(r.found, r.label)
			return
		end
		if choice:peek() == "zeron" then
			if auth.server:peek().kind == "zeron" then
				open:set(false)
				return
			end
			go_to(nil, auth.server_label(nil))
			return
		end
		local typed_address = trim(address:peek())
		if typed_address == "" then
			why:set(auth.server_says({ error = "empty", host = "", message = "" }))
			focus_field()
			return
		end
		look_up(typed_address, false)
	end

	local function switch()
		local t = target:peek()
		if switching:peek() or not t then
			return
		end
		switching:set(true)
		switch_why:set("")
		auth.use_server(t.found, function(failure: string?)
			switching:set(false)
			if failure then
				switch_why:set(auth.server_says({ error = failure, host = t.label, message = "" }))
				return
			end
			asking:set(false)
			switched(true)
		end)
	end

	local function self_hosted(): boolean
		return choice:get() == "self_hosted"
	end

	local function target_label(): string
		local t = target:get()
		return if t then t.label else ""
	end

	-- (Both are modals, drawn over the window: the box holding them is out of
	-- the flow, so it neither takes room nor a gap where it's put.)
	return ui.div {
		absolute = true,
		ds.Dialog {
			open = open,
			title = ui.t("account.server.title"),
			body = ui.t("account.server.body"),
			size = "md",
			hold = function(): boolean
				return busy:get() == "using"
			end,
			confirm = {
				label = function(): string
					local r = ready:get()
					if r then
						return ui.t("account.server.connect_to", { server = r.label }):get()
					end
					return if self_hosted() then ui.t("account.server.connect"):get() else ui.t("account.server.use_zeron"):get()
				end,
				keep_open = true,
				busy = function(): boolean
					return busy:get() ~= ""
				end,
				disabled = function(): boolean
					return self_hosted() and string.match(address:get(), "%S") == nil
				end,
				on_click = connect,
			},
			build = function(): Node
				local server = auth.server:peek()
				return ui.column {
					id = "server-dialog",
					w_full = true,
					gap = "$space.4",
					-- Where focus starts: the modal puts it on the first control
					-- (Zeron's choice) as this is built; Self-hosted's address
					-- takes it after, once it can be typed in (an invite's
					-- lookup over), Enter the way on.
					on_mount = function()
						ui.after(0, function()
							if choice:peek() == "self_hosted" and busy:peek() == "" then
								focus_now()
							end
						end)
					end,
					ds.RadioGroup {
						value = choice,
						label = ui.t("account.server.label"),
						options = {
							{ value = "zeron", label = ui.t("account.server.zeron"), description = server.zeron_host },
							{ value = "self_hosted", label = ui.t("account.server.self_hosted"), description = ui.t("account.server.self_hosted_description") },
						},
						on_change = function(v: string)
							-- (Chosen again while an address was looked up: its
							-- answer is for a choice no longer made.)
							if busy:peek() == "looking" then
								auth.forget_lookup()
								busy:set("")
							end
							why:set("")
							ready:set(nil)
							if v == "self_hosted" then
								focus_field()
							end
						end,
					},
					-- (Under Self-hosted, its address: kept in place, quiet, while
					-- Zeron is chosen, so the dialog never jumps as the choice
					-- moves.)
					ds.TextField {
						value = address,
						label = ui.t("account.server.address"),
						placeholder = ui.t("account.server.placeholder"),
						inputmode = "url",
						size = "md",
						w_full = true,
						select_on_focus = true,
						input_ref = input,
						hint = ui.t("account.server.separate"),
						error = function(): string?
							local w = why:get()
							return if w ~= "" and self_hosted() then w else nil
						end,
						disabled = function(): boolean
							return busy:get() ~= "" or not self_hosted()
						end,
						on_key = function(e: any): boolean?
							if e.key == "Enter" and not (e.shift or e.cmd or e.alt or e.ctrl) then
								connect()
								return true
							end
							return nil
						end,
					},
					trouble(function(): string
						return if self_hosted() then "" else why:get()
					end),
					ui.row {
						ds.Link {
							ui.t("account.server.guide"),
							tone = "quiet",
							size = "sm",
							external = true,
							href = server.guide,
							on_click = function()
								ui.invoke("open_url", server.guide)
							end,
						},
					},
				}
			end,
		},
		-- Signed in: the question before anything is signed out.
		ds.Dialog {
			open = asking,
			title = function(): string
				return ui.t("account.server.switch_title", { server = target_label() }):get()
			end,
			body = function(): string
				return ui.t("account.server.switch_body", { server = target_label() }):get()
			end,
			tone = "danger",
			hold = function(): boolean
				return switching:get()
			end,
			confirm = {
				label = ui.t("account.server.switch"),
				keep_open = true,
				busy = function(): boolean
					return switching:get()
				end,
				on_click = switch,
			},
			trouble(function(): string
				return switch_why:get()
			end),
		},
	}
end)

return M
