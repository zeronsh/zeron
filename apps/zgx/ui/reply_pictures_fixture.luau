-- Every state of a reply's pictures in one transcript, for
-- tests/reply_pictures.test.luau and for captures: one picture after a line
-- of text, a run of them, a very wide, a very tall and a tiny one, one
-- still coming, one that can't be read, ones never read (a URL, a home
-- path), a GIF (it plays: src/gif_tests.rs), and the pictures an agent shows (`show_image`, a generated
-- image: image parts). Reads come from a stub: `assets/…` reads at once
-- (or after `fixture_delay` ms), `slow/…` never lands, anything else isn't
-- in the chat's folder. `fixture_width` narrows the page; the commands
-- switch the theme and stream a reply that shows a picture.
--
--   ./target/release/zeron-zgx fixture=reply_pictures_fixture.luau --inspect

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local T = require("@ds/transcript")
local Lightbox = require("@ds/lightbox")
local ReplyImages = require("@app/screens/chat/reply_images")

local reads = ui.state("fixture_reads", "")
local delay = ui.state("fixture_delay", 0)
local width = ui.state("fixture_width", 820)
-- Whether the chat is on this Mac (a picture's file is here: Show in Finder).
local here = ui.state("fixture_here", true)

-- A read: `assets/…` lands (at once, or after the delay), `slow/…` never
-- does, anything else isn't there.
local function stub(prefix: string): (target: any, path: string) -> any
	return function(_target: any, path: string): any
		reads:set(reads:peek() .. path .. "\n")
		local rel = string.match(path, "^" .. prefix .. "(.*)$") or path
		return {
			after = function(_self: any, ok: (any) -> (), err: (any) -> ())
				if string.sub(rel, 1, 5) == "slow/" then
					return
				end
				local function land()
					if string.sub(rel, 1, 7) == "assets/" then
						ok({ src = rel })
					else
						err("No such file in the chat's folder")
					end
				end
				if delay:peek() > 0 then
					ui.after(delay:peek(), land)
				else
					land()
				end
			end,
		}
	end
end

local function session(): any
	return { id = "chat-1", cwd = "/work", device = if here:get() then nil else "far", here = "mac" }
end
local pictures = ReplyImages.for_session(session, stub("/work/"))
local sent = ReplyImages.for_attachments(session, stub(""))

local function reply(id: string, blocks: { any }): any
	local text = {}
	for _, b in blocks do
		if b.kind == "text" then
			table.insert(text, b.text)
		end
	end
	return {
		id = id,
		role = "assistant",
		text = table.concat(text, "\n\n"),
		model = "Opus",
		at = "Nov 2, 4:10 PM",
		blocks = blocks,
	}
end

local function words(id: string, text: string): any
	return reply(id, { { id = "t", kind = "text", text = text } })
end

local MESSAGES = {
	{ id = "u1", role = "user", text = "Show me the new shell." },
	words("single", "Here it is after the change:\n![The shell](assets/composer/shell.png)\nThe sidebar keeps its width now."),
	{ id = "u2", role = "user", text = "Before and after, side by side?" },
	words(
		"row",
		"Three captures, in order:\n\n![The shell](assets/composer/shell.png)\n![The transcript](./assets/composer/transcript.png)\n![Pixels](assets/lightbox/pixels.png)\n\nThe second is the one to look at."
	),
	{ id = "u3", role = "user", text = "And the odd shapes?" },
	words(
		"shapes",
		"A very wide chart:\n\n![Frame times](assets/pictures/wide.png)\n\nA very tall page:\n\n![The whole page](assets/lightbox/tall.png)\n\nAn icon:\n\n![The icon](assets/pictures/tiny.png)"
	),
	{ id = "u4", role = "user", text = "What if a picture is slow, or missing?" },
	words(
		"states",
		"Still coming:\n\n![The build log](slow/log.png)\n\nNot in the folder:\n\n![Settings](captures/settings.png)\n\nNever read: ![a URL](https://example.com/x.png) and ![home](~/x.png). A GIF, playing:\n\n![The spinner](assets/pictures/blink.gif)"
	),
	{ id = "u4b", role = "user", text = "And one among the words?" },
	words(
		"inline",
		"The menu bar ![The menu](assets/lightbox/pixels.png) sits above it, and these ![Left](assets/composer/transcript.png) ![Right](assets/pictures/tiny.png) close the line."
	),
	{ id = "u5", role = "user", text = "Generate two icon ideas." },
	reply("parts", {
		{ id = "t1", kind = "text", text = "Two directions:" },
		{
			id = "i1",
			kind = "images",
			images = {
				{ id = "i1", path = "assets/lightbox/pixels.png", name = "icon-a.png" },
				{ id = "i2", path = "assets/composer/transcript.png", name = "icon-b.png" },
			},
		},
		{ id = "t2", kind = "text", text = "The first reads better small." },
	}),
	{ id = "u6", role = "user", text = "Show me the screenshot." },
	reply("part", {
		{ id = "i3", kind = "images", images = { { id = "i3", path = "assets/composer/shell.png", name = "shell.png" } } },
	}),
	{ id = "u7", role = "user", text = "Show the chart and the latency visual." },
	reply("failed", {
		{ id = "t1", kind = "text", text = "The chart:" },
		{ id = "i4", kind = "images", images = { { id = "i4", path = "assets/pictures/wide.png", name = "frames.png" } } },
		{ id = "t2", kind = "text", text = "The heat map:" },
		{ id = "f1", kind = "display_failed", tool = "image", text = "The image is too large to show." },
		{ id = "t3", kind = "text", text = "And the latency visual:" },
		{ id = "f2", kind = "display_failed", tool = "visual", text = "visual script failed: visual:2: attempt to index nil with 'rows'" },
		{ id = "t4", kind = "text", text = "I'll try the visual again with fewer rows." },
	}),
}

local messages = ui.state("fixture_messages_v3", MESSAGES :: { any })
local offset = ui.state("fixture_offset", 0)
local draft = ui.state("fixture_draft", { id = "", text = "" })

ui.command { id = "theme_light", title = "Light theme", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-light")
end }
ui.command { id = "theme_dark", title = "Dark theme", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-dark")
end }
-- A reply written word by word that shows a picture partway.
ui.command { id = "stream", title = "Stream a reply with a picture", run = function()
	local list = table.clone(messages:peek())
	table.insert(list, { id = "u9", role = "user", text = "One more?" })
	table.insert(list, { id = "live", role = "assistant", text = "", streaming = true, blocks = { { id = "t", kind = "text", text = "" } } })
	messages:set(list)
	local full = "Here:\n\n![The live shell](assets/composer/shell.png)\n\nAnd that's the whole of it, the sidebar at its own width."
	local at = 0
	local function step()
		at = math.min(#full, at + 3)
		draft:set({ id = "live", text = string.sub(full, 1, at), part = "t", part_text = string.sub(full, 1, at) })
		if at < #full then
			ui.after(40, step)
		end
	end
	step()
end }

local Page = ui.component(function(): Node
	return ui.column {
		w_full = true,
		h_full = true,
		bg = "$zeron.window",
		items = "center",
		ui.column {
			h_full = true,
			w = function(): number
				return width:get()
			end,
			T.Transcript {
				messages = messages,
				offset = offset,
				draft = draft,
				pictures = pictures,
				sent_pictures = sent,
			},
		},
		Lightbox.Host(),
		-- What's said for a moment, as the shell has it (ds/toast).
		ds.toast.Host {},
	}
end)

return Page {}
