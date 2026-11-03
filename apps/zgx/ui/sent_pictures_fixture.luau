-- A conversation whose prompts carry pictures (ds/transcript → ds/
-- user_message), for tests/sent_pictures.test.luau: the chat on another
-- device (`test_sent_remote`), so each picture is read from there (the
-- read stubbed: `test_sent_reads`, one line each; a path with "missing"
-- in it can't be read), or on this one, each path as it is. The
-- conversation is made again, as a reload makes it, when
-- `test_sent_generation` moves. Here, beside app.luau, as a mounted script
-- loads modules from its own directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local T = require("@ds/transcript")
local Lightbox = require("@ds/lightbox")
local ReplyImages = require("@app/screens/chat/reply_images")
-- (Files that have gone, as after a sent picture is deleted: any path
-- under /gone/, which isn't there, so `ui.image` says so.)
-- On this device, a picture whose file has since been deleted
-- (`test_sent_gone`).
local gone = ui.state("test_sent_gone", false)

local remote = ui.state("test_sent_remote", true)
local reads = ui.state("test_sent_reads", "")
local generation = ui.state("test_sent_generation", 1)

local sent = ReplyImages.for_attachments(function(): any
	return { id = "chat-1", device = if remote:get() then "other" else "me", here = "me" }
end, function(_target: any, path: string): any
	reads:set(reads:peek() .. path .. "\n")
	return {
		after = function(_self: any, ok: (any) -> (), bad: (any) -> ())
			if string.find(path, "missing", 1, true) then
				bad("Not on that device")
			else
				ok({ src = "assets/composer/shell.png" })
			end
		end,
	}
end)

-- On another device, its paths there; on this one, files here.
local function conversation(on_other: boolean): { any }
	return {
		{
			-- (Each its own message: a chat doesn't move between devices.)
			id = if on_other then "u-other" elseif gone:get() then "u-gone" else "u-here",
			role = "user",
			text = "Look at these",
			attachments = if on_other
				then {
					{ kind = "image", name = "shot.png", src = "/host/uploads/shot.png" },
					{ kind = "image", name = "gone.png", src = "/host/uploads/missing.png" },
				}
				elseif gone:get() then {
					{ kind = "image", name = "deleted.png", src = "/gone/deleted.png" },
				}
				else {
					{ kind = "image", name = "shot.png", src = "assets/composer/shell.png" },
				},
		},
		{ id = if on_other then "a-other" else "a-here", role = "assistant", text = "Seen." },
	}
end
local messages = ui.computed(function(): { any }
	return conversation(remote:get())
end)

return ui.column {
	w_full = true,
	h_full = true,
	ui.presence(function(): string
		return tostring(generation:get())
	end, function(): Node
		return T.Transcript { messages = messages, sent_pictures = sent }
	end),
	Lightbox.Host(),
}
