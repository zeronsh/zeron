-- A reply's pictures (ds/reply_picture through ds/markdown, read by
-- screens/chat/reply_images; ui/reply_images_fixture.luau): each draws at
-- its own shape within the reading column and 420 tall; a click opens it in the
-- lightbox among its reply's others; one that can't be read says so (why,
-- on hover); a URL,
-- a home path are never read (the links they were stay), a GIF is read as
-- any picture; and the
-- reply made again (as a reload makes it) reads nothing again.

local function open_reply(): any
	local app = mount("reply_images_fixture.luau")
	app:resize(800, 1400)
	app:frames(20)
	expect(app:error()).to_be_nil()
	return app
end

local function reads(app: any): { string }
	local out = {}
	for line in (app:state("test_reply_reads") or ""):gmatch("[^\n]+") do
		table.insert(out, line)
	end
	return out
end

local function box(app: any, label: string): (number, number)
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(`"{label}"`, 1, true) then
			local w, h = line:match(" (%d+)x(%d+)")
			return tonumber(w) :: number, tonumber(h) :: number
		end
	end
	return 0, 0
end

test("a reply's pictures read once each, from the chat's folder; a URL and a home path are never read", function()
	local app = open_reply()
	local got = reads(app)
	table.sort(got)
	expect(table.concat(got, ",")).to_be("/work/assets/composer/shell.png,/work/assets/composer/transcript.png,/work/assets/pictures/blink.gif,/work/captures/settings.png,/work/pruned.png")
	expect(app:has('"A URL"')).to_be(true)
	expect(app:has('"Home"')).to_be(true)
	-- (A GIF: a picture like any, which plays.)
	expect(app:has('"Open A GIF"')).to_be(true)
end)

test("a picture draws at its own shape, within the column and 420 tall, and opens in the lightbox among its reply's", function()
	local app = open_reply()
	-- 868 by 617 in a 568-point column: as wide as the column, its shape kept.
	local w, h = box(app, "Open The shell")
	expect(w).to_be(568)
	expect(math.abs(h - 617 * 568 / 868) < 1.5).to_be(true)
	app:click('"Open The transcript"')
	app:frames(60)
	-- (Among the shell, the transcript and the GIF: the ones shown.)
	expect(app:has('"2 of 3"')).to_be(true)
	expect(app:state("ds/lightbox.luau::lifted")).to_be("reply:r1:./assets/composer/transcript.png")
	app:key("ArrowLeft")
	app:frames(30)
	expect(app:has('"1 of 3"')).to_be(true)
	app:key("Escape")
	app:frames(60)
	expect(app:state("ds/lightbox.luau::open")).to_be_nil()
	-- Focus back on the picture it closed into (the one shown last).
	local focused = ""
	for line in app:tree():gmatch("[^\n]+") do
		if line:find("focused", 1, true) then
			focused = line
		end
	end
	expect(focused).to_contain('"Open The shell"')
end)

test("a picture that can't be read says so (why, on hover)", function()
	local app = open_reply()
	expect(app:has('"Settings couldn\'t be shown"')).to_be(true)
	expect(app:has('text "No such file in the chat\'s folder"')).to_be(false)
	expect(app:has('"Open Settings"')).to_be(false)
end)

test("the reply made again (as a reload makes it) reads nothing again", function()
	local app = open_reply()
	expect(#reads(app)).to_be(5)
	app:set_state("test_reply_generation", 2)
	app:frames(20)
	expect(app:has('"Open The shell"')).to_be(true)
	expect(#reads(app)).to_be(5)
end)

test("a reply picture whose kept file was pruned says so, never a script error", function()
	local app = open_reply()
	expect(app:error()).to_be_nil()
	expect(app:has('"Pruned couldn\'t be shown"')).to_be(true)
	expect(app:has('"Open Pruned"')).to_be(false)
end)
