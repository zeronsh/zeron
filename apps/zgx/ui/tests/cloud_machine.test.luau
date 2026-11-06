-- A chat's Cloud machine on its composer. First what's said and offered
-- (screens/chat/cloud_machine, pure functions); then, on ui/chat_fixture.luau
-- with the edge answered by ui/cloud_stub.luau, what the composer does with it:
-- its state on the notice line, the one alert in the tray behind the pill, the
-- sizes it opens to, and what a pick asks of the host. Settings → Cloud's
-- default size is on ui/shell_fixture.luau.

local logic = require("../screens/chat/cloud_machine")

-- (All findable by one filter: `zgx test ui "Cloud machine:"`.)
local named_test = test
local function test(name: string, body: () -> ())
	named_test(`Cloud machine: {name}`, body)
end

local MACHINES = {
	{ id = "small", name = "Small", vcpu = 1, memory_gb = 2, disk_gb = 20, rate = 0.5 },
	{ id = "default", name = "Standard", vcpu = 2, memory_gb = 4, disk_gb = 50, rate = 1 },
	{ id = "large", name = "Large", vcpu = 4, memory_gb = 16, disk_gb = 111, rate = 2 },
	{ id = "xlarge", name = "Extra large", vcpu = 8, memory_gb = 32, disk_gb = 400, rate = 5.56 },
}
local NOW = 1800000000
local DAY = 86400

local function session(fields: { [string]: any }): any
	local s: { [string]: any } = { chat = "c1", device = "cloud-1", state = "sleeping", repo = "acme/atlas", machine = "default" }
	for k, v in fields do
		s[k] = v
	end
	return s
end

local function alert_of(s: any, opts: { [string]: any }?): any
	local o: any = opts or {}
	o.now = NOW
	return logic.alert(s, o)
end

local function row_named(p: any, id: string): any
	for _, r in p.rows do
		if r.id == id then
			return r
		end
	end
	return nil
end

-- ── What's said ────────────────────────────────────────────────────────────

test("the rate reads as the edge gives it, with no trailing places", function()
	expect(logic.rate(1)).to_be("1×")
	expect(logic.rate(0.5)).to_be("0.5×")
	expect(logic.rate(2)).to_be("2×")
	expect(logic.rate(5.56)).to_be("5.56×")
	expect(logic.rate(nil)).to_be("1×")
end)

test("what a machine holds rounds down, what a chat needs rounds up", function()
	expect(logic.holds_gb(110.9)).to_be(110)
	expect(logic.needs_gb(110.1)).to_be(111)
	expect(logic.specs(MACHINES[3])).to_be("4 vCPU · 16 GB memory · 111 GB disk")
end)

test("each state has its own line, and an alert's state leaves the line to it", function()
	local function text(fields: any): string?
		local n = logic.line(session(fields))
		return if n then n.text else nil
	end
	expect(text({ state = "sleeping" })).to_be("Cloud machine asleep — it wakes when you send")
	expect((logic.line(session({ state = "sleeping" })) :: any).action).to_be("Wake")
	expect(text({ state = "starting" })).to_be("Waking its Cloud machine…")
	expect((logic.line(session({ state = "starting" })) :: any).spinner).to_be(true)
	expect(text({ state = "provisioning" })).to_be("Starting a Cloud machine…")
	expect(text({ state = "stopping" })).to_be("Its Cloud machine is going to sleep…")
	-- (Up, it has nothing to say: a notice is an alert, in the pill for as long
	-- as the chat's open.)
	expect(text({ state = "ready" })).to_be(nil)
	expect(text({ state = "error", error = "No room" })).to_be(nil)
	expect(text({ state = "data_expired" })).to_be(nil)
	expect(text({ wake_blocked = { reason = "disk_too_small", needed = 150 } })).to_be(nil)
end)

test("a stage reads in words; one it can't word isn't shown as an id", function()
	local function text(stage: string): string
		return (logic.line(session({ state = "starting", stage = stage })) :: any).text
	end
	expect(text("clone")).to_be("Waking its Cloud machine — cloning the repository…")
	expect(text("restoring your files")).to_be("Waking its Cloud machine — restoring your files…")
	expect(text("x_internal_step")).to_be("Waking its Cloud machine…")
end)

test("an old edge's session raises nothing", function()
	local s = { chat = "c1", device = "cloud-1", state = "ready" }
	expect(alert_of(s)).to_be(nil)
	expect(logic.line(s)).to_be(nil)
end)

test("an unknown reason a machine can't wake isn't called disk_too_small", function()
	expect(alert_of(session({ wake_blocked = { reason = "something_new" } }))).to_be(nil)
end)

-- ── One alert at a time ────────────────────────────────────────────────────

test("alerts never compete: the worst is the one raised", function()
	local warn = { used = 41, capacity = 50, level = "warn" }
	local critical = { used = 49, capacity = 50, level = "critical" }
	local soon = NOW + 3 * DAY
	local blocked = { reason = "disk_too_small", needed = 150 }
	local function kind(fields: any, opts: any): string?
		local a = alert_of(session(fields), opts)
		return if a then a.kind else nil
	end
	expect(kind({ state = "data_expired", wake_blocked = blocked, disk = critical })).to_be("data_expired")
	expect(kind({ wake_blocked = blocked, disk = critical, expires = soon })).to_be("wake_blocked")
	expect(kind({ state = "error", disk = critical })).to_be("failed")
	expect(kind({ disk = critical, expires = soon }, { failure = "failed" })).to_be("wake_failed")
	expect(kind({ disk = critical, expires = soon })).to_be("disk")
	expect(kind({ disk = warn, expires = soon })).to_be("expiring")
	expect(kind({ state = "ready", disk = warn })).to_be("disk")
	expect(kind({ state = "ready", disk = { used = 5, capacity = 50, level = "ok" } })).to_be(nil)
end)

test("the wake-blocked alert says what the chat needs", function()
	local a = alert_of(session({ wake_blocked = { reason = "disk_too_small", needed = 149.2 } }))
	expect(a.title).to_be("Machine can't wake")
	expect(a.summary).to_be("It needs 150 GB of disk")
	expect(a.body).to_contain("it needs 150 GB")
	expect(a.picks).to_be(true)
	expect(a.dismissable).to_be(nil)
end)

test("a failure says the real reason", function()
	local a = alert_of(session({ state = "error", error = "The provider is out of machines." }))
	expect(a.body).to_be("The provider is out of machines.")
	expect(a.action).to_be("retry")
	expect(alert_of(session({ state = "error" })).body).to_be("Something went wrong on its machine.")
end)

test("a refused wake says why, in words", function()
	local function body(word: string): string
		return alert_of(session({}), { failure = word }).body
	end
	expect(body("disk_too_small")).to_be("That size can't hold this chat's files. Pick a bigger one.")
	expect(body("offline")).to_be("You're offline. Try again once you're back.")
	expect(body("You're out of Cloud credits. Add some in Settings → Billing.")).to_contain("out of Cloud credits")
	expect(body("failed")).to_be("The machine didn't wake. Try again in a moment.")
end)

test("the disk warning says how full it is", function()
	local a = alert_of(session({ state = "ready", disk = { used = 41.5, capacity = 50, level = "warn" } }))
	expect(a.title).to_be("Disk 83% full")
	expect(a.summary).to_be("42 of 50 GB on this chat's machine")
	expect(a.tone).to_be("warning")
	expect(a.dismissable).to_be(true)
	expect(a.picks).to_be(true)
	expect(alert_of(session({ state = "ready", disk = { used = 49, capacity = 50, level = "critical" } })).tone).to_be("danger")
end)

test("files going is said only when it's soon, and only while it sleeps", function()
	expect(logic.expiry_level(session({ expires = NOW + 30 * DAY }), NOW)).to_be(nil)
	expect(logic.expiry_level(session({ expires = NOW + 7 * DAY }), NOW)).to_be("soon")
	expect(logic.expiry_level(session({ expires = NOW + 3600 }), NOW)).to_be("very_soon")
	expect(logic.expiry_level(session({ state = "ready", expires = NOW + 3600 }), NOW)).to_be(nil)
	expect(logic.expiry_level(session({}), NOW)).to_be(nil)
	local a = alert_of(session({ expires = NOW + 3 * DAY }))
	expect(a.summary).to_be("Its files are deleted in 3 days")
	expect(a.tone).to_be("neutral")
	expect(alert_of(session({ expires = NOW + 3600 })).summary).to_be("Its files are deleted in 1 day")
end)

test("files gone says the conversation is kept, and where a fresh machine comes from", function()
	local a = alert_of(session({ state = "data_expired" }))
	expect(a.body).to_contain("The conversation is kept")
	expect(a.body).to_contain("acme/atlas, on the same branch")
	expect(a.action).to_be("fresh")
end)

-- ── The sizes ──────────────────────────────────────────────────────────────

test("every size is listed; those too small are disabled and say what they hold", function()
	local s = session({ wake_blocked = { reason = "disk_too_small", needed = 150 } })
	local p = logic.picker(s, MACHINES, alert_of(s))
	expect(#p.rows).to_be(4)
	expect(p.none_fit).to_be(false)
	local large = row_named(p, "large")
	expect(large.disabled).to_be(true)
	expect(large.reason).to_be("holds 111 GB")
	expect(large.specs).to_be("4 vCPU · 16 GB memory · 111 GB disk")
	expect(large.rate).to_be("2× credits")
	expect(row_named(p, "small").reason).to_be("holds 20 GB")
	-- The one in use says so, not what it holds.
	expect(row_named(p, "default").disabled).to_be(true)
	expect(row_named(p, "default").current).to_be(true)
	expect(row_named(p, "default").reason).to_be("Current")
	local xlarge = row_named(p, "xlarge")
	expect(xlarge.disabled).to_be(false)
	expect(xlarge.reason).to_be(nil)
	expect(xlarge.rate).to_be("5.56× credits")
end)

test("a size holding exactly what's needed will do", function()
	local s = session({ wake_blocked = { reason = "disk_too_small", needed = 111 } })
	expect(row_named(logic.picker(s, MACHINES, alert_of(s)), "large").disabled).to_be(false)
end)

test("nothing big enough says so and offers a new chat, its files left alone", function()
	local s = session({ wake_blocked = { reason = "disk_too_small", needed = 900 } })
	local p = logic.picker(s, MACHINES, alert_of(s))
	expect(p.none_fit).to_be(true)
	for _, r in p.rows do
		expect(r.disabled).to_be(true)
	end
	-- (It doesn't go on asking for a pick there's no way to make.)
	expect(p.body).to_be("This chat's machine can't wake: it needs 900 GB of disk.")
	expect(p.note).to_contain("No machine holds the 900 GB")
	expect(p.note).to_contain("this chat and its files stay as they are")
	expect(p.action).to_be("new_chat")
end)

test("a disk warning offers only what's bigger than it has", function()
	local s = session({ state = "ready", disk = { used = 41, capacity = 50, level = "warn" } })
	local p = logic.picker(s, MACHINES, alert_of(s))
	expect(row_named(p, "small").reason).to_be("holds 20 GB")
	expect(row_named(p, "default").reason).to_be("Current")
	expect(row_named(p, "large").disabled).to_be(false)
	expect(row_named(p, "xlarge").disabled).to_be(false)
	expect(p.note).to_contain("restarts the machine")
end)

test("the biggest machine filling up has nothing to pick, and says what to do", function()
	local s = session({ state = "ready", machine = "xlarge", disk = { used = 340, capacity = 400, level = "warn" } })
	local p = logic.picker(s, MACHINES, alert_of(s))
	expect(p.none_fit).to_be(true)
	expect(p.action).to_be(nil)
	expect(p.note).to_contain("biggest machine")
end)

test("a machine in a turn waits for it to end before it restarts", function()
	local s = session({ state = "ready", disk = { used = 41, capacity = 50, level = "warn" } })
	local p = logic.picker(s, MACHINES, alert_of(s), { running = true })
	expect(row_named(p, "xlarge").disabled).to_be(true)
	expect(row_named(p, "xlarge").reason).to_be("after this turn")
	-- (Nothing's lost by waiting, so it isn't "nothing fits".)
	expect(p.none_fit).to_be(false)
end)

test("the default's options are every size, with specs and rate, none disabled", function()
	local options = logic.default_options(MACHINES)
	expect(#options).to_be(4)
	expect(options[4].value).to_be("xlarge")
	expect(options[4].label).to_be("Extra large")
	expect(options[4].description).to_be("8 vCPU · 32 GB memory · 400 GB disk · 5.56× credits")
	expect((options[4] :: any).disabled).to_be(nil)
end)

-- ── As the deck takes it ───────────────────────────────────────────────────

local function deck(fields: any, opts: any?, picks: any?): any
	local s = session(fields)
	local a = alert_of(s)
	return logic.deck_alert(a, if a.picks then logic.picker(s, MACHINES, a, picks) else nil, opts)
end

test("the alert reaches the deck with every size under it, those too small disabled", function()
	local d = deck({ wake_blocked = { reason = "disk_too_small", needed = 150 } })
	expect(d.tone).to_be("warning")
	expect(d.title).to_be("Machine can't wake")
	expect(d.detail).to_be("It needs 150 GB of disk")
	expect(#d.more).to_be(4)
	expect(d.dismissable).to_be(false)
	local large, xlarge = d.more[3], d.more[4]
	expect(large.title).to_be("Large")
	expect(large.disabled).to_be(true)
	expect(large.action).to_be(nil)
	-- The cells say only what it is; the trailing slot says why it's out of reach.
	expect(table.concat(large.cells, "|")).to_be("4 vCPU|16 GB memory|111 GB disk|2× credits")
	expect(large.trail).to_be("holds 111 GB")
	expect(xlarge.disabled).to_be(false)
	expect(xlarge.action).to_be("Wake here")
	expect(xlarge.trail).to_be(nil)
	expect(table.concat(xlarge.cells, "|")).to_be("8 vCPU|32 GB memory|400 GB disk|5.56× credits")
	local current = d.more[2]
	expect(current.current).to_be(true)
	expect(current.trail).to_be("Current")
	expect(d.grid).to_be(logic.GRID)
	expect(logic.size_of(xlarge.id)).to_be("xlarge")
	expect(logic.size_of(d.id)).to_be(nil)
end)

test("what's already said under it isn't said on its row too", function()
	local d = deck({ state = "error", error = "The provider is out of machines." })
	expect(d.tone).to_be("danger")
	expect(d.detail).to_be(nil)
	expect(d.body).to_be("The provider is out of machines.")
	expect(d.action).to_be("Try again")
	expect(d.more).to_be(nil)
end)

test("files going reads in the deck's quiet tone, and can be let go", function()
	local d = deck({ expires = NOW + 3 * DAY })
	expect(d.tone).to_be("info")
	expect(d.dismissable).to_be(true)
end)

test("a pick on its way takes nothing more, and a refused one says why", function()
	local blocked = { wake_blocked = { reason = "disk_too_small", needed = 150 } }
	local busy = deck(blocked, { busy = true })
	expect(busy.spinner).to_be(true)
	expect(busy.more[4].action).to_be(nil)
	local refused = deck(blocked, { failure = "disk_too_small" })
	expect(refused.note).to_be("That size can't hold this chat's files. Pick a bigger one.")
	expect(refused.more[4].action).to_be("Wake here")
end)

test("nothing big enough: its way on is the new chat", function()
	local s = session({ wake_blocked = { reason = "disk_too_small", needed = 900 } })
	local a = alert_of(s)
	local p = logic.picker(s, MACHINES, a)
	local d = logic.deck_alert(a, p)
	expect(d.action).to_be("Start a new chat")
	expect((logic.way_on(a, p))).to_be("new_chat")
	expect(d.body).to_be("This chat's machine can't wake: it needs 900 GB of disk.")
	expect(d.note).to_contain("No machine holds the 900 GB")
end)

-- ── Dismissal ──────────────────────────────────────────────────────────────

test("put away stays away until the level worsens", function()
	local map = logic.dismiss({}, "c1", "disk", "warn")
	expect(logic.dismissed(map, "c1", "disk", "warn")).to_be(true)
	expect(logic.dismissed(map, "c1", "disk", "critical")).to_be(false)
	expect(logic.dismissed(map, "c2", "disk", "warn")).to_be(false)
	expect(logic.dismissed(map, "c1", "expiring", "soon")).to_be(false)
end)

test("a level falling moves what's remembered with it", function()
	local function disk(level: string): any
		return session({ state = "ready", disk = { used = 1, capacity = 50, level = level } })
	end
	local put = logic.dismiss({}, "c1", "disk", "critical")
	-- Still critical: nothing moved, the same map.
	expect(logic.settle(put, disk("critical"), NOW) == put).to_be(true)
	-- Down to warn: warn stays away, critical is news again.
	local lower = logic.settle(put, disk("warn"), NOW)
	expect(logic.dismissed(lower, "c1", "disk", "warn")).to_be(true)
	expect(logic.dismissed(lower, "c1", "disk", "critical")).to_be(false)
	-- Clear: forgotten, so filling up again warns again.
	local clear = logic.settle(lower, disk("ok"), NOW)
	expect(clear.c1).to_be(nil)
	expect(logic.dismissed(clear, "c1", "disk", "warn")).to_be(false)
end)

test("an alert put away isn't raised, and the next one down is", function()
	local s = session({ disk = { used = 41, capacity = 50, level = "warn" }, expires = NOW + 3 * DAY })
	local map = logic.dismiss({}, "c1", "expiring", "soon")
	local a = alert_of(s, {
		dismissed = function(kind: string, level: string): boolean
			return logic.dismissed(map, "c1", kind, level)
		end,
	})
	expect(a.kind).to_be("disk")
end)

-- ── On the composer ────────────────────────────────────────────────────────

local function cloud_calls(app: any): { string }
	return (app:state("cloud_stub_calls") :: any) or {}
end

local function asked(app: any, entry: string): boolean
	for _, c in cloud_calls(app) do
		if c == entry then
			return true
		end
	end
	return false
end

local function asked_like(app: any, prefix: string): number
	local n = 0
	for _, c in cloud_calls(app) do
		if string.sub(c, 1, #prefix) == prefix then
			n += 1
		end
	end
	return n
end

local function showing(app: any, sel: string): boolean
	if not app:has(sel) then
		return false
	end
	local first = string.match(app:tree(sel), "[^\n]+") or ""
	return string.find(first, " hidden", 1, true) == nil
end

local LIST = 'role:list "Alerts"'
local function in_deck(what: string): string
	return `{LIST} >> {what}`
end

local function open_deck(app: any)
	app:click('role:button "Open Alerts"')
	app:wait("#never-shown", { timeout_ms = 320 })
	app:frames(10)
end

-- The chat fixture's s2, on a Cloud machine, as the edge says `case`; the deck
-- opened as the person would, if `open`.
local function on_cloud(case: string, open: boolean?): any
	local app = mount("chat_fixture.luau")
	app:resize(960, 760)
	app:command("go_s2")
	app:frames(30)
	app:command("cloud_stub.offer")
	app:command(`cloud_stub.session.{case}`)
	app:command("session_on_cloud")
	-- (Past the reads' gap: the session as just set.)
	app:wait("#never-shown", { timeout_ms = 300 })
	app:frames(30)
	if open then
		open_deck(app)
	end
	return app
end

local function to_case(app: any, case: string)
	app:wait("#never-shown", { timeout_ms = 300 })
	app:command(`cloud_stub.session.{case}`)
	app:frames(30)
end

-- A size's row in the deck.
local function size(name: string): string
	return in_deck(`role:listitem "{name}"`)
end

test("nothing opens itself: each alert waits in the pill for the person", function()
	for _, case in { "sleeping", "waking", "failed", "blocked", "disk_warn", "expiring", "expired" } do
		local app = on_cloud(case)
		expect(app:has('role:button "Open Alerts"')).to_be(true)
		expect(app:state("chat_tray")).to_be(nil)
		expect(showing(app, LIST)).to_be(false)
		expect(app:error()).to_be(nil)
	end
end)

-- The glyphs: the tab's, and any an alert's row wears of its own.
local function glyphs_in(app: any, sel: string, indent: string): number
	local n = 0
	for line in app:tree(sel):gmatch("[^\n]+") do
		if line:match(`^{indent}@%d+ img`) and not line:find(" hidden", 1, true) then
			n += 1
		end
	end
	return n
end

test("each Cloud alert shows one glyph, the tab's, folded or opened to its sizes", function()
	for _, case in { "blocked", "blocked_none", "disk_warn", "disk_critical", "expiring", "failed", "expired" } do
		local app = on_cloud(case, true)
		expect(glyphs_in(app, LIST, "    ")).to_be(0)
		expect(glyphs_in(app, 'role:tab "Fold Alerts"', "  ")).to_be(1)
		expect(app:error()).to_be(nil)
	end
end)

test("with its notice beside it, each says its own tone", function()
	local app = on_cloud("disk_critical_asleep", true)
	expect(showing(app, in_deck('~"Cloud machine asleep"'))).to_be(true)
	expect(showing(app, in_deck('~"Disk 96% full"'))).to_be(true)
	expect(glyphs_in(app, LIST, "    ")).to_be(2)
end)

test("the alerts are the composer's own", function()
	local app = on_cloud("blocked")
	expect(app:has('#chat-composer >> role:button "Open Alerts"')).to_be(true)
	open_deck(app)
	expect(app:has(`#chat-composer >> {LIST}`)).to_be(true)
end)

test("a sleeping chat on Cloud says so, never that it can't reach the host", function()
	local app = on_cloud("sleeping", true)
	app:type('role:textbox "Message"', "Ship it")
	app:frames(10)
	expect(showing(app, in_deck('~"Cloud machine asleep"'))).to_be(true)
	expect(app:has('~"Can\'t reach"')).to_be(false)
	expect(app:error()).to_be(nil)
end)

test("the notice's Wake wakes it, and it reads as waking", function()
	local app = on_cloud("sleeping", true)
	app:click(in_deck('role:button "Wake"'))
	app:frames(30)
	expect(asked(app, "cloud.wake:s2:-")).to_be(true)
	expect(showing(app, in_deck('~"Waking its Cloud machine…"'))).to_be(true)
	expect(app:has(in_deck('~"Cloud machine asleep"'))).to_be(false)
end)

test("each of its states reads in the deck as it changes", function()
	local app = on_cloud("waking", true)
	expect(showing(app, in_deck('~"Waking its Cloud machine…"'))).to_be(true)
	to_case(app, "staged")
	expect(showing(app, in_deck('~"cloning the repository"'))).to_be(true)
	to_case(app, "provisioning")
	expect(showing(app, in_deck('~"Starting a Cloud machine…"'))).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("a machine that's up says nothing, and the deck goes", function()
	local app = on_cloud("sleeping", true)
	to_case(app, "ready")
	expect(app:has('role:button "Open Alerts"')).to_be(false)
	expect(app:has('role:tab "Fold Alerts"')).to_be(false)
	expect(app:state("chat_tray")).to_be(nil)
	-- Nor, with a draft, that it can't be reached.
	app:type('role:textbox "Message"', "Ship it")
	app:frames(10)
	expect(app:has('role:button "Open Alerts"')).to_be(false)
end)

test("a chat with no session row keeps the old notice", function()
	local app = on_cloud("gone")
	app:type('role:textbox "Message"', "Ship it")
	app:frames(10)
	open_deck(app)
	expect(showing(app, in_deck('~"Can\'t reach Cloud machine"'))).to_be(true)
end)

test("failed shows the real reason, with another go", function()
	local app = on_cloud("failed", true)
	expect(showing(app, in_deck('~"The provider ran out of machines in this region."'))).to_be(true)
	app:click(in_deck('role:button "Try again"'))
	app:frames(30)
	expect(asked(app, "cloud.wake:s2:-")).to_be(true)
end)

test("wake-blocked opens to what it needs and every size", function()
	local app = on_cloud("blocked", true)
	expect(showing(app, in_deck('~"Machine can\'t wake"'))).to_be(true)
	expect(showing(app, in_deck('~"it needs 150 GB of disk"'))).to_be(true)
	-- All four, the ones too small disabled, saying what they hold.
	local large = app:tree(size("Large"))
	expect(string.match(large, "[^\n]+")).to_contain("disabled")
	expect(app:has(`{size("Large")} >> ~"holds 111 GB"`)).to_be(true)
	expect(app:has(`{size("Large")} >> ~"2× credits"`)).to_be(true)
	expect(app:has(`{size("Large")} >> role:button`)).to_be(false)
	expect(app:has(`{size("Small")} >> ~"holds 20 GB"`)).to_be(true)
	-- The one in use reads as current, and isn't dimmed like the rest.
	expect(app:has(`{size("Standard")} >> ~"Current"`)).to_be(true)
	expect(app:has(`{size("Standard")} >> ~"holds"`)).to_be(false)
	local xlarge = app:tree(size("Extra large"))
	expect(string.find(string.match(xlarge, "[^\n]+") or "", " disabled", 1, true)).to_be(nil)
	for _, cell in { "8 vCPU", "32 GB memory", "400 GB disk" } do
		expect(app:has(`{size("Extra large")} >> ~"{cell}"`)).to_be(true)
	end
	expect(app:has(`{size("Extra large")} >> ~"5.56× credits"`)).to_be(true)
	-- It can't be let go: it's in the way until it's dealt with.
	expect(app:has(in_deck('role:button ~"Dismiss"'))).to_be(false)
	expect(app:error()).to_be(nil)
end)

-- Each size row's cells and trailing words, by their edges, as the tree lays
-- them out, and the row's own box. (Its name is its label: the tree doesn't list
-- it again. What it lines up with is the first cell, which only stays put while
-- the name's column does.)
local function grid_rows(app: any): { any }
	local rows: { any } = {}
	local at: any = nil
	for line in app:tree(LIST):gmatch("[^\n]+") do
		local name, x, w = line:match('listitem "([^"]+)" (%d+),%d+ (%d+)x')
		if name and (name == "Small" or name == "Standard" or name == "Large" or name == "Extra large") then
			at = { name = name, right = tonumber(x) + tonumber(w), texts = {} }
			table.insert(rows, at)
		elseif name then
			at = nil
		elseif at then
			local tx, tw = line:match(' text "[^"]*" (%d+),%d+ (%d+)x')
			if tx and not line:find(" hidden", 1, true) then
				table.insert(at.texts, { left = tonumber(tx), right = tonumber(tx) + tonumber(tw) })
			end
		end
	end
	return rows
end

test("the sizes are a grid: every column on one line, at every width, nothing past its row", function()
	for _, case in { "blocked", "disk_warn" } do
		local app = on_cloud(case, true)
		for _, width in { 960, 760, 640, 560, 480, 420 } do
			app:resize(width, 760)
			app:wait("#never-shown", { timeout_ms = 120 })
			app:frames(20)
			local rows = grid_rows(app)
			expect(#rows).to_be(4)
			-- As many cells in each row, each ending on its column's line. (A row
			-- with words in its trailing slot has those after them; one with a
			-- pick, a button rather than a text.)
			local cells = math.huge
			for _, r in rows do
				cells = math.min(cells, #r.texts)
			end
			for _, r in rows do
				for c = 1, cells do
					expect(r.texts[c].right).to_be(rows[1].texts[c].right)
				end
				-- Nothing runs past the row, whatever's in its trailing slot.
				for _, t in r.texts do
					expect(t.right <= r.right).to_be(true)
				end
			end
		end
		expect(app:error()).to_be(nil)
	end
end)

test("short of room the columns go in order, from every row at once", function()
	local app = on_cloud("blocked", true)
	local function shown_cells(width: number): number
		app:resize(width, 760)
		app:wait("#never-shown", { timeout_ms = 120 })
		app:frames(20)
		-- (Extra large: cells alone, its slot's a button.)
		for _, r in grid_rows(app) do
			if r.name == "Extra large" then
				return #r.texts
			end
		end
		return -1
	end
	local wide, narrower = shown_cells(960), shown_cells(560)
	expect(wide).to_be(4)
	expect(narrower < wide).to_be(true)
	-- The disk's the last of them to go.
	shown_cells(480)
	expect(app:has(`{size("Extra large")} >> ~"400 GB disk"`)).to_be(true)
end)

test("the trailing slot holds why at the same place as a pick", function()
	local app = on_cloud("disk_warn", true)
	local why = string.match(app:tree(`{size("Small")} >> ~"holds 20 GB"`), ' (%d+),%d+ (%d+)x')
	expect(why ~= nil).to_be(true)
	local function right_of(sel: string): number
		local x, w = string.match(app:tree(sel), ' (%d+),%d+ (%d+)x')
		return tonumber(x) + tonumber(w)
	end
	local reason = right_of(`{size("Small")} >> ~"holds 20 GB"`)
	local current = right_of(`{size("Standard")} >> ~"Current"`)
	local pick = right_of(`{size("Large")} >> role:button "Move here" >> ~"Move here"`)
	expect(reason).to_be(current)
	expect(math.abs(reason - pick) <= 1).to_be(true)
end)

test("picking a size that fits wakes the chat on it; it's asleep already, so no sleep first", function()
	local app = on_cloud("blocked", true)
	app:click(`{size("Extra large")} >> role:button "Wake here"`)
	app:frames(30)
	expect(asked(app, "cloud.wake:s2:xlarge")).to_be(true)
	expect(asked_like(app, "cloud.sleep:")).to_be(0)
	-- It's waking now: the alert's gone, the notice says so.
	expect(app:has(in_deck('~"Machine can\'t wake"'))).to_be(false)
	expect(showing(app, in_deck('~"Waking its Cloud machine…"'))).to_be(true)
end)

test("a pick the edge refuses as too small says so and stays up", function()
	local app = on_cloud("blocked", true)
	app:command("cloud_stub.refuse_wake")
	app:click(`{size("Extra large")} >> role:button "Wake here"`)
	app:frames(30)
	expect(asked(app, "cloud.wake:s2:xlarge")).to_be(true)
	expect(showing(app, in_deck('~"That size can\'t hold this chat\'s files."'))).to_be(true)
	expect(showing(app, `{size("Extra large")} >> role:button "Wake here"`)).to_be(true)
end)

test("nothing big enough offers a new chat in its project, and wakes nothing", function()
	local app = on_cloud("blocked_none", true)
	expect(showing(app, in_deck('~"No machine holds the 900 GB"'))).to_be(true)
	expect(app:has(`{size("Extra large")} >> ~"holds 400 GB"`)).to_be(true)
	expect(app:has(in_deck('role:button "Wake here"'))).to_be(false)
	-- (No size to wake it on: it doesn't pretend otherwise.)
	expect(app:has(in_deck('~"Pick a size"'))).to_be(false)
	app:click(in_deck('role:button "Start a new chat"'))
	app:frames(30)
	expect(asked_like(app, "cloud.wake:")).to_be(0)
	expect(asked_like(app, "cloud.retry:")).to_be(0)
	expect(asked_like(app, "cloud.sleep:")).to_be(0)
	expect(app:state("shell_new_chat_project")).to_be("atlas")
end)

test("the disk warning is put away until it worsens, not message by message", function()
	local app = on_cloud("disk_warn", true)
	expect(showing(app, in_deck('~"Disk 83% full"'))).to_be(true)
	expect(showing(app, in_deck('~"42 of 50 GB on this chat\'s machine"'))).to_be(true)
	-- It takes no modal: the composer's there to write in.
	expect(app:has("role:dialog")).to_be(false)
	app:type('role:textbox "Message"', "Still here")
	app:click(in_deck('role:button "Dismiss Disk 83% full"'))
	app:frames(30)
	expect(app:has('role:button "Open Alerts"')).to_be(false)
	-- Messages go by, and the same level reads again: still away.
	to_case(app, "disk_warn")
	expect(app:has('role:button "Open Alerts"')).to_be(false)
	-- It worsens: back, in the pill.
	to_case(app, "disk_critical")
	expect(app:has('role:button "Open Alerts"')).to_be(true)
	expect(app:state("chat_tray")).to_be(nil)
	open_deck(app)
	expect(showing(app, in_deck('~"Disk 96% full"'))).to_be(true)
	-- All clear and full again: back too.
	app:click(in_deck('role:button "Dismiss Disk 96% full"'))
	app:frames(30)
	to_case(app, "disk_ok")
	to_case(app, "disk_warn")
	expect(app:has('role:button "Open Alerts"')).to_be(true)
end)

test("the disk warning opens to the same sizes; a machine that's up sleeps, then wakes on it", function()
	local app = on_cloud("disk_warn", true)
	expect(app:has(`{size("Standard")} >> ~"Current"`)).to_be(true)
	app:click(`{size("Large")} >> role:button "Move here"`)
	app:frames(30)
	-- In that order: the edge refuses a new size on a machine that's up.
	local calls = table.concat(cloud_calls(app), ",")
	local slept = string.find(calls, "cloud.sleep:s2", 1, true)
	local woke = string.find(calls, "cloud.wake:s2:large", 1, true)
	expect(slept ~= nil).to_be(true)
	expect(woke ~= nil).to_be(true)
	expect((slept :: number) < (woke :: number)).to_be(true)
	expect(showing(app, in_deck('~"Waking its Cloud machine…"'))).to_be(true)
end)

test("a machine that won't go to sleep says so, and the size stays as it was", function()
	local app = on_cloud("disk_warn", true)
	app:command("cloud_stub.fail_next")
	app:click(`{size("Large")} >> role:button "Move here"`)
	app:frames(30)
	expect(asked(app, "cloud.sleep:s2")).to_be(true)
	expect(asked_like(app, "cloud.wake:")).to_be(0)
	expect(showing(app, in_deck('~"Machine didn\'t wake"'))).to_be(true)
end)

test("files going soon is one quiet word; later on, only that it's asleep", function()
	local app = on_cloud("expires_later", true)
	expect(showing(app, in_deck('~"Cloud machine asleep"'))).to_be(true)
	expect(app:has(in_deck('~"Its files are deleted"'))).to_be(false)
	to_case(app, "expiring")
	-- (On its row, and said in full under it.)
	expect(showing(app, in_deck('role:listitem ~"Its files are deleted in 3 days"'))).to_be(true)
	-- (Not two alerts for one machine.)
	expect(app:has(in_deck('~"Cloud machine asleep"'))).to_be(false)
	app:click(in_deck('role:button "Wake it now"'))
	app:frames(30)
	expect(asked(app, "cloud.wake:s2:-")).to_be(true)
end)

test("files going, put away, leaves it saying it's asleep", function()
	local app = on_cloud("expiring", true)
	app:click(in_deck('role:button "Dismiss Machine asleep"'))
	app:frames(30)
	expect(showing(app, in_deck('~"Cloud machine asleep"'))).to_be(true)
	expect(app:has(in_deck('~"Its files are deleted"'))).to_be(false)
end)

test("files gone explains itself and offers a fresh machine", function()
	local app = on_cloud("expired", true)
	expect(showing(app, in_deck('role:listitem ~"The conversation is kept"'))).to_be(true)
	expect(showing(app, in_deck('~"acme/atlas, on the same branch"'))).to_be(true)
	-- (Its files are already gone: nothing to ask first. And not a wake: an
	-- expired machine only comes back through the fresh route, on the size new
	-- chats start on.)
	app:click(in_deck('role:button "Start a fresh machine"'))
	app:frames(30)
	expect(asked(app, "cloud.retry:s2:default")).to_be(true)
	expect(asked_like(app, "cloud.wake:")).to_be(0)
	expect(app:has("role:dialog")).to_be(false)
	expect(showing(app, in_deck('~"Starting a Cloud machine…"'))).to_be(true)
end)

test("an old edge shows state, and nothing of sizes", function()
	local app = on_cloud("sleeping", true)
	app:command("cloud_stub.old_edge")
	app:frames(30)
	expect(showing(app, in_deck('~"Cloud machine asleep"'))).to_be(true)
	expect(app:has(`{LIST} >> role:listitem "Standard"`)).to_be(false)
	expect(app:error()).to_be(nil)
end)

test("nothing about a machine shows on a chat that isn't on Cloud", function()
	local app = mount("chat_fixture.luau")
	app:resize(960, 760)
	app:command("go_s2")
	app:frames(30)
	app:command("cloud_stub.offer")
	app:frames(30)
	expect(app:has('role:button "Open Alerts"')).to_be(false)
	expect(asked_like(app, "cloud.sessions")).to_be(0)
end)

test("at rest, it reads the sessions rarely and draws nothing for them", function()
	local app = on_cloud("sleeping")
	local before = asked_like(app, "cloud.sessions")
	app:wait("#never-shown", { timeout_ms = 3400 })
	-- (Asleep isn't settling: no read in the close follow's 3 s.)
	expect(asked_like(app, "cloud.sessions")).to_be(before)
end)

test("while waking, it follows closely", function()
	local app = on_cloud("waking")
	local before = asked_like(app, "cloud.sessions")
	app:wait("#never-shown", { timeout_ms = 3400 })
	expect(asked_like(app, "cloud.sessions") > before).to_be(true)
end)

-- ── Settings → Cloud ───────────────────────────────────────────────────────

local PAGE = "#settings-page"
local NAV = "#settings-nav"

-- (A radio's line says "checked" or "unchecked": one contains the other.)
local function chosen_size(app: any, name: string): boolean
	local line = string.match(app:tree(`{PAGE} >> role:radiogroup "Default machine" >> role:radio "{name}"`), "[^\n]+") or ""
	return string.find(line, " unchecked", 1, true) == nil and string.find(line, " checked", 1, true) ~= nil
end

local function settings(old_edge: boolean?): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:command("cloud_stub.offer")
	if old_edge then
		app:command("cloud_stub.old_edge")
	end
	app:command("shell.settings")
	app:frames(20)
	app:click(`{NAV} >> role:tab "Cloud"`)
	app:frames(30)
	return app
end

test("Settings → Cloud lists every size with its rate, none held back", function()
	local app = settings()
	local group = `{PAGE} >> role:radiogroup "Default machine"`
	expect(app:has(group)).to_be(true)
	for _, name in { "Small", "Standard", "Large", "Extra large" } do
		local radio = app:tree(`{group} >> role:radio "{name}"`)
		expect(string.find(string.match(radio, "[^\n]+") or "", " disabled", 1, true)).to_be(nil)
	end
	expect(app:tree(group)).to_contain("8 vCPU · 32 GB memory · 400 GB disk · 5.56× credits")
	expect(app:tree(group)).to_contain("1 vCPU · 2 GB memory · 20 GB disk · 0.5× credits")
	expect(chosen_size(app, "Standard")).to_be(true)
	expect(chosen_size(app, "Extra large")).to_be(false)
end)

test("picking a size asks the edge, and sticks", function()
	local app = settings()
	local group = `{PAGE} >> role:radiogroup "Default machine"`
	app:click(`{group} >> role:radio "Extra large"`)
	app:frames(30)
	expect(asked(app, "cloud.set_machine:xlarge")).to_be(true)
	expect(chosen_size(app, "Extra large")).to_be(true)
	expect(chosen_size(app, "Standard")).to_be(false)
end)

test("a size the edge refuses goes back, and says so", function()
	local app = settings()
	local group = `{PAGE} >> role:radiogroup "Default machine"`
	app:command("cloud_stub.refuse_machine")
	app:click(`{group} >> role:radio "Large"`)
	app:frames(30)
	expect(chosen_size(app, "Standard")).to_be(true)
	expect(chosen_size(app, "Large")).to_be(false)
	expect(app:has(`{PAGE} >> ~"Couldn't change the default machine."`)).to_be(true)
end)

test("an old edge offers no choice, and no section", function()
	local app = settings(true)
	expect(showing(app, `{PAGE} >> role:radiogroup "Default machine"`)).to_be(false)
	expect(app:has(`{PAGE} >> ~"credits used"`)).to_be(true)
end)
