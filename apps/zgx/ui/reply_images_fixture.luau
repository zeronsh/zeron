-- A reply that shows pictures, for tests/reply_images.test.luau: two that
-- read, one that can't, and three that are never read (a URL, a home path,
-- …). A GIF reads as any picture, and plays. Its reads are counted in `test_reply_reads` (one line each). The
-- reply is made again, as a reload makes it, when `test_reply_generation`
-- moves: what each picture is outlives it, so nothing reads twice. Here,
-- beside app.luau, as a mounted script loads modules from its own
-- directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local Markdown = require("@ds/markdown")
local Lightbox = require("@ds/lightbox")
local ReplyImages = require("@app/screens/chat/reply_images")
-- (A kept copy pruned since it was read: any path under /kept/, which
-- isn't there, so `ui.image` says so.)

local reads = ui.state("test_reply_reads", "")
local generation = ui.state("test_reply_generation", 1)

local pictures = ReplyImages.for_session(function(): any
	return { id = "chat-1", cwd = "/work" }
end, function(_target: any, path: string): any
	reads:set(reads:peek() .. path .. "\n")
	local rel = string.match(path, "^/work/(.*)$")
	return {
		after = function(_self: any, ok: (any) -> (), err: (any) -> ())
			if rel == "pruned.png" then
				ok({ src = "/kept/pruned.png" })
			elseif rel and string.sub(rel, 1, 7) == "assets/" then
				ok({ src = rel })
			else
				err("No such file in the chat's folder")
			end
		end,
	}
end)

local TEXT = table.concat({
	"The shell:",
	"![The shell](assets/composer/shell.png)",
	"The transcript:",
	"![The transcript](./assets/composer/transcript.png)",
	"Missing:",
	"![Settings](captures/settings.png)",
	"Pruned:",
	"![Pruned](pruned.png)",
	"Never read:",
	"![A URL](https://example.com/x.png)",
	"![Home](~/x.png)",
	"![A GIF](assets/pictures/blink.gif)",
}, "\n\n")

return ui.column {
	w_full = true,
	h_full = true,
	ui.presence(function(): string
		return tostring(generation:get())
	end, function(): Node
		return ui.scroll {
			w_full = true,
			h_full = true,
			ui.column {
				w = 600,
				p = "$space.4",
				Markdown { text = TEXT, pictures = pictures, group = "reply:r1" },
			},
		}
	end),
	Lightbox.Host(),
}
