-- Gallery: a reply's pictures (ds/reply_picture), as a reply names them in
-- Markdown and as an agent shows them: alone, together on a tray, the odd
-- shapes, coming, and the quiet line for one that can't be shown.

local Markdown = require("@ds/markdown")
local Lightbox = require("@ds/lightbox")
local ReplyPicture = require("@ds/reply_picture")
local ReplyImages = require("@app/screens/chat/reply_images")
local kit = require("@demos/kit")

-- Reads: the gallery's own files at once; `slow/…` never lands; anything
-- else isn't in the chat's folder.
local pictures = ReplyImages.for_session(function(): any
	return { id = "gallery-pictures", cwd = "/gallery" }
end, function(_target: any, path: string): any
	local rel = string.match(path, "^/gallery/(.*)$") or path
	return {
		after = function(_self: any, ok: (any) -> (), err: (any) -> ())
			if string.sub(rel, 1, 5) == "slow/" then
				return
			elseif string.sub(rel, 1, 7) == "assets/" then
				ok({ src = rel })
			else
				err("No such file in the chat's folder")
			end
		end,
	}
end)

local function reply(group: string, text: string): Node
	return ui.column {
		w_full = true,
		max_w = ui.theme.get("zeron.prose.measure") :: number,
		Markdown { text = text, pictures = pictures, group = group },
	}
end

return {
	id = "reply_pictures",
	title = "Reply pictures",
	about = "Pictures a reply names or an agent shows. Alone, a picture takes its own shape, as wide as the reading column and 420 tall at most, never past its own size. A long page shows its top and a long strip its middle, and an icon sits whole in a 64-point tile. Several together sit on a tray in justified rows, each corner the tray's less its inset. While one comes, its frame holds the place it will take. One that can't be shown is a quiet line, with why on hover. Hover for Copy and Show in Finder, secondary-click for the rest, and click to open it in the lightbox among the reply's pictures.",
	demo = function()
		return kit.page(
			kit.block("ALONE", reply("gp-alone", "![The shell](assets/composer/shell.png)")),
			kit.block(
				"TOGETHER",
				reply("gp-run", "![The shell](assets/composer/shell.png)\n![The transcript](assets/composer/transcript.png)\n![Pixels](assets/lightbox/pixels.png)")
			),
			kit.block(
				"LONG, WIDE AND SMALL",
				reply("gp-shapes", "![Frame times](assets/pictures/wide.png)\n\n![The whole page](assets/lightbox/tall.png)\n\n![The icon](assets/pictures/tiny.png)")
			),
			kit.block("COMING", reply("gp-coming", "![The build log](slow/log.png)")),
			kit.block(
				"COULDN'T BE SHOWN",
				reply("gp-missing", "![Settings](captures/settings.png)"),
				ReplyPicture.Line { words = "Couldn't show this image", reason = "The image is too large to show." },
				ReplyPicture.Line { icon = "widget", words = "Couldn't show this visual" }
			),
			Lightbox.Host()
		)
	end,
}
