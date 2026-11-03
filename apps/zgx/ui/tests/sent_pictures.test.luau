-- Pictures sent with a prompt (ui/sent_pictures_fixture.luau): in a chat on
-- another device each is read from there once (the engine's
-- ReadAttachmentChunk, kept as a file here) and opens in the lightbox from
-- that file; in a chat on this device each shows from its path, nothing
-- read; one that can't be read keeps its frame, nothing failing; and the
-- conversation made again (as a reload makes it) reads nothing again.

local function chat(remote: boolean): any
	local app = mount("sent_pictures_fixture.luau")
	app:resize(900, 700)
	app:set_state("test_sent_remote", remote)
	app:frames(20)
	expect(app:error()).to_be_nil()
	return app
end

local function reads(app: any): { string }
	local out = {}
	for line in (app:state("test_sent_reads") or ""):gmatch("[^\n]+") do
		table.insert(out, line)
	end
	return out
end

test("on another device, a sent picture is read from there once and opens from the file kept here", function()
	local app = chat(true)
	local got = reads(app)
	table.sort(got)
	expect(table.concat(got, ",")).to_be("/host/uploads/missing.png,/host/uploads/shot.png")
	app:click('"Open shot.png"')
	app:frames(60)
	local open = app:state("ds/lightbox.luau::open")
	expect(open).never.to_be_nil()
	expect(open.items[open.index].src).to_be("assets/composer/shell.png")
	expect(#reads(app)).to_be(2)
end)

test("a picture sent from here to another device shows from its file on the way, then from the chat's copy once it lands, before its reply begins too", function()
	local app = chat(true)
	app:set_state("test_sent_reads", "")
	app:set_state("test_sent_pending", "sending")
	app:frames(20)
	-- (This Mac's file: the chat's device never had it, so nothing is read.)
	expect(#reads(app)).to_be(0)
	app:click('"Open paste.png"')
	app:frames(60)
	local open = app:state("ds/lightbox.luau::open")
	expect(open).never.to_be_nil()
	expect(open.items[open.index].src).to_be("assets/composer/shell.png")
	app:key("Escape")
	app:frames(60)
	-- Arrived, its reply not begun (the chat's copy, still waiting): the
	-- same bubble, its picture now the upload, read from there, not looked
	-- for here.
	app:set_state("test_sent_pending", "working")
	app:frames(20)
	expect(app:error()).to_be_nil()
	expect(table.concat(reads(app), ",")).to_be("/host/uploads/paste.png")
	app:click('"Open paste.png"')
	app:frames(60)
	open = app:state("ds/lightbox.luau::open")
	expect(open).never.to_be_nil()
	expect(open.items[open.index].src).to_be("assets/composer/shell.png")
	app:key("Escape")
	app:frames(60)
	-- Its reply begun: the same picture, not read again.
	app:set_state("test_sent_pending", "landed")
	app:frames(20)
	expect(app:error()).to_be_nil()
	expect(table.concat(reads(app), ",")).to_be("/host/uploads/paste.png")
	expect(app:has('"Open paste.png"')).to_be(true)
end)

test("on this device, a sent picture shows from its path, nothing read", function()
	local app = chat(false)
	-- (The fixture starts on another device: what it read then aside.)
	app:set_state("test_sent_reads", "")
	app:frames(20)
	expect(#reads(app)).to_be(0)
	app:click('"Open shot.png"')
	app:frames(60)
	local open = app:state("ds/lightbox.luau::open")
	expect(open.items[open.index].src).to_be("assets/composer/shell.png")
end)

test("a sent picture that can't be read keeps its frame, nothing failing; made again, nothing is read again", function()
	local app = chat(true)
	expect(app:has('"Open gone.png"')).to_be(true)
	expect(app:error()).to_be_nil()
	expect(#reads(app)).to_be(2)
	app:set_state("test_sent_generation", 2)
	app:frames(20)
	expect(app:has('"Open shot.png"')).to_be(true)
	expect(#reads(app)).to_be(2)
end)

test("a sent picture whose file has gone draws nothing and says so, never a script error; opened, the lightbox says so too", function()
	local app = mount("sent_pictures_fixture.luau")
	app:resize(900, 700)
	app:set_state("test_sent_remote", false)
	app:set_state("test_sent_gone", true)
	app:frames(20)
	expect(app:error()).to_be_nil()
	expect(app:has('"Open deleted.png"')).to_be(true)
	app:click('"Open deleted.png"')
	app:frames(60)
	expect(app:error()).to_be_nil()
	expect(app:has('.lightbox-gone >> ~"deleted.png couldn\'t be shown"')).to_be(true)
	app:key("Escape")
	app:frames(60)
	expect(app:state("ds/lightbox.luau::open")).to_be_nil()
	expect(app:error()).to_be_nil()
end)
