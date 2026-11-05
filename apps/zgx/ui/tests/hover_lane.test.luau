-- A message's hover lane (its time and Copy, ds/transcript's `strip`) as
-- the list scrolls under a still pointer: it goes with the pointer, one lane
-- at a time, right under its message. (A lane shown mid-scroll and let go
-- before the scroll stopped once stayed drawn where it last stood, hanging
-- in the space under a reply: zgx's partial repaint, fixed in zgx c43b4e5;
-- zgx-asks/stale-paint-mounted-mid-scroll has the pixel repro.)

local TEXT = "Yes. I called Zeron’s `show_image` tool three times to display `components.png`, `workbench.png`, and `zgx-effects.png` from the project.\n\n"

local function settle(app: any)
	app:frames(30)
end

-- Real time passing (timers run on the clock; frames alone pass none).
local function wait(app: any, ms: number)
	app:wait("#never-shown", { timeout_ms = ms })
end

-- How far under the text above it the shown lane's time stands (nil: no
-- lane shows).
local function lane_gap(app: any, stamp: string): number?
	local bottom: number? = nil
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local y, h = line:match(" %-?%d+,(%-?%d+) %d+x(%d+)")
		if y then
			if line:find(`"{stamp}`, 1, true) then
				return if bottom then (tonumber(y) :: number) - bottom else nil
			elseif line:find("text ", 1, true) then
				bottom = (tonumber(y) :: number) + (tonumber(h) :: number)
			end
		end
	end
	return nil
end

-- The reply "a<n>"'s time ("a7" → "Sep 8, 4:17 PM").
local function time_of(id: string): string
	return `Sep 8, 4:{10 + (tonumber(string.sub(id, 2)) :: number)} PM`
end

-- The lanes shown: at most one, and it is the hovered message's.
local function one_lane(app: any)
	local now = app:state("transcript_hover") :: string
	local shown = 0
	for i = 1, 30 do
		if app:has('"' .. time_of(`a{i}`) .. '"') then
			shown += 1
			if `a{i}` ~= now then
				error(`the lane of a{i} showed with the pointer over {now}`)
			end
		end
	end
	expect(shown <= 1).to_be(true)
end

test("hover lane: it goes with the pointer as the list scrolls under it, one at a time", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s1")
	settle(app)
	app:hover('~"Part 30 works like this"')
	settle(app)
	expect(app:state("transcript_hover")).to_be("a30")
	-- Scrolled up under the still pointer a step a frame: the lane follows
	-- whatever message is under it, never two at once.
	local seen: { [string]: boolean } = {}
	for _ = 1, 12 do
		app:scroll("#transcript", -60)
		app:frames(2)
		wait(app, 20)
		settle(app)
		seen[app:state("transcript_hover") :: string] = true
		one_lane(app)
	end
	-- It left the message it started on, and showed others on the way.
	local now = app:state("transcript_hover") :: string
	expect(now).never.to_be("a30")
	local others = 0
	for id in seen do
		if id ~= "" and id ~= "a30" then
			others += 1
		end
	end
	expect(others >= 1).to_be(true)
end)

test("hover lane: a pointer crossing into a message mid-scroll shows its lane at once", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s1")
	settle(app)
	-- (Opened, it settles at its end first.)
	wait(app, 300)
	settle(app)
	app:scroll("#transcript", -200)
	app:frames(3)
	app:hover('~"Part 29 works like this"')
	settle(app)
	expect(app:state("transcript_hover")).to_be("a29")
	expect(app:count('"' .. time_of("a29") .. '"')).to_be(1)
	one_lane(app)
end)

test("hover lane: under a reply written into its runway, hovered and scrolled as it lands", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s1")
	settle(app)
	app:type('role:textbox "Message"', "did u call any show image tool")
	app:key("Enter")
	wait(app, 700)
	settle(app)
	-- Its reply, in blocks as the host sends one, written a few words at a
	-- time under the pointer while the list is nudged both ways.
	local ms = table.clone(app:state("chat.messages"))
	ms[#ms] = table.clone(ms[#ms])
	ms[#ms].waiting = nil
	table.insert(ms, { id = "rr", role = "assistant", text = "", model = "Opus", streaming = true, started = 0, blocks = { { id = "t0", kind = "text", text = "" } } })
	app:set_state("chat.messages", ms)
	app:frames(5)
	local n = 0
	for k = 12, #TEXT, 12 do
		n += 1
		local part = string.sub(TEXT, 1, k)
		app:set_state("chat.draft", { id = "rr", text = part, part = "t0", part_text = part })
		app:frames(2)
		if n % 3 == 0 then
			app:scroll("#transcript", if n % 2 == 0 then -120 else 120)
		end
	end
	local done = table.clone(ms)
	done[#done] = { id = "rr", role = "assistant", text = TEXT, model = "Opus", at = "Nov 2, 3:36 AM", copy = TEXT, blocks = { { id = "t0", kind = "text", text = TEXT } } }
	app:set_state("chat.messages", done)
	app:set_state("chat.draft", { id = "", text = "" })
	for k = 1, 8 do
		app:scroll("#transcript", if k % 2 == 0 then -200 else 220)
		app:frames(2)
		app:hover('~"Yes. I called"')
		app:frames(2)
	end
	wait(app, 150)
	settle(app)
	app:hover('~"Yes. I called"')
	wait(app, 150)
	settle(app)
	-- Its lane right under its last line (the 12 the lane's top pad and
	-- the block gap less its pull up give), not down its runway.
	local gap = lane_gap(app, "Nov 2, 3:36 AM")
	expect(gap ~= nil and (gap :: number) <= 16).to_be(true)
end)
