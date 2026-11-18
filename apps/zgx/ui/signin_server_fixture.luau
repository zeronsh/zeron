-- Setup's Sign in, the pill naming its server and the dialog it opens
-- (screens/server), in setup's window, the host's account calls answered
-- here as it would: a server at an address with "acme" in it (Acme's), none
-- anywhere else ("not_zeron"). The wall is left out (a card's art is WebP,
-- which a headless host can't draw). Driven by
-- tests/signin_server.test.luau.
--
-- What was asked, in order, is `signin_server_calls`. `ui.command`s:
--   signin_server.waiting      waiting on the browser
--   signin_server.failed       the last sign-in didn't finish
--   signin_server.environment  a development build's ZERON_ZGX_EDGE named the server
--   signin_server.refuse_next  the next switch fails ("offline")
--   signin_server.ask          File → Connect to Server… (`server.ask()`)
--   signin_server.invite       an invite clicked, to zeron.acme.com
--   signin_server.signed_in    an account kept here (a switch asks first)

require("@ds").setup()
local auth = require("@account/auth")
local signin = require("@screens/onboarding/signin")
local server = require("@screens/server")

local calls = ui.state("signin_server_calls", {} :: { string })
local function log(entry: string)
	calls:push(entry)
end
local refuse = ui.state("signin_server_refuse", false)
local status = ui.state("signin_server_status", "ready")
local choosing = ui.state("signin_server_open", false)

local GUIDE = "https://github.com/zeronsh/zeron/blob/main/docs/self-hosting.md"
local ZERON = { kind = "zeron", host = "edge.zeron.sh", url = "https://edge.zeron.sh", zeron_host = "edge.zeron.sh", guide = GUIDE }
auth.account:set({ state = "signed_out", env = "production", server = ZERON } :: any)

local auths = auth :: any
auths.find_server = function(address: string, done: (any?, any?) -> ())
	log(`find_server:{address}`)
	local host = string.gsub(string.gsub(address, "^https://", ""), "/$", "")
	if string.find(host, "acme", 1, true) then
		done({ zeron = false, current = (auth.server:peek() :: any).host == host, host = host, url = `https://{host}`, workos_client_id = "client_acme", name = "Acme" }, nil)
	else
		done(nil, { error = "not_zeron", host = host, message = `{host} isn't a Zeron server.` })
	end
end
auths.forget_lookup = function() end
auths.use_server = function(found: any?, done: ((string?) -> ())?)
	log(`use_server:{if found then found.host else "zeron"}`)
	if refuse:peek() then
		refuse:set(false)
		if done then
			done("offline")
		end
		return
	end
	local s = if found
		then { kind = "self_hosted", name = found.name, host = found.host, url = found.url, zeron_host = ZERON.zeron_host, guide = GUIDE }
		else ZERON
	auth.account:set({ state = "signed_out", env = if found then "self_hosted" else "production", server = s } :: any)
	if done then
		done(nil)
	end
end
ui.command {
	id = "signin_server.waiting",
	title = "Waiting on the browser",
	run = function()
		status:set("waiting")
	end,
}
ui.command {
	id = "signin_server.failed",
	title = "The sign-in didn't finish",
	run = function()
		status:set("failed")
	end,
}
ui.command {
	id = "signin_server.environment",
	title = "ZERON_ZGX_EDGE named the server",
	run = function()
		auth.account:set({
			state = "signed_out",
			env = "custom",
			server = { kind = "environment", host = "localhost:8787", url = "http://localhost:8787", zeron_host = "localhost:8787", guide = GUIDE },
		} :: any)
	end,
}
ui.command {
	id = "signin_server.refuse_next",
	title = "The next switch fails",
	run = function()
		refuse:set(true)
	end,
}
ui.command {
	id = "signin_server.ask",
	title = "Connect to Server…",
	run = function()
		server.ask()
	end,
}
ui.command {
	id = "signin_server.invite",
	title = "An invite to zeron.acme.com",
	run = function()
		server.ask("zeron.acme.com")
	end,
}
ui.command {
	id = "signin_server.signed_in",
	title = "An account kept here",
	run = function()
		auth.account:set({ state = "signed_in", env = "production", server = auth.server:peek() } :: any)
	end,
}

local function no_wall(): signin.Wall
	return {
		build = function(): Node
			return ui.div {}
		end,
		intrudes = 0,
	}
end

return ui.div {
	w = 680,
	h = 500,
	signin.screen {
		status = status,
		walls = { one = no_wall },
		server = server.identity,
		on_server = ui.set(choosing, true),
		overlay = server.Dialog {
			open = choosing,
			on_changed = function(signed_out: boolean)
				log(`changed:{signed_out}`)
				if status:peek() == "failed" then
					status:set("ready")
				end
			end,
		},
	},
}
