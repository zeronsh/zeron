-- Pictures in Markdown as ui.markdown places them (zgx 97859035): a picture
-- on a line of its own is a block, splitting its paragraph; one among words
-- is a run `{ x = alt, img = src }`; no empty paragraph is left behind. And
-- ds/markdown_find parses as ds/markdown draws: pictures that follow one
-- another are one `pics` block, so a find's block numbers agree. On
-- ui/markdown_pictures_fixture.luau.

local function shapes(): { [string]: any }
	local app = mount("markdown_pictures_fixture.luau")
	app:frames(2)
	expect(app:error()).to_be_nil()
	return app:state("fixture_markdown_pictures")
end

test("ui.markdown: a picture under words is a block of its own", function()
	local s = shapes()
	expect(s["raw:Here it is:\n![shot](a.png)"]).to_be("p:Here it is: img:a.png")
	expect(s["raw:Here it is:\n![shot](a.png)\nAfter."]).to_be("p:Here it is: img:a.png p:After.")
end)

test("ui.markdown: pictures on consecutive lines are blocks, no empty paragraph", function()
	local s = shapes()
	expect(s["raw:![a](a.png)\n![b](b.png)"]).to_be("img:a.png img:b.png")
	expect(s["raw:![a](a.png)\n\nAfter."]).to_be("img:a.png p:After.")
	expect(s["raw:Three:\n\n![a](a.png)\n![b](b.png)\n![c](c.png)\n\nDone."]).to_be("p:Three: img:a.png img:b.png img:c.png p:Done.")
end)

test("ui.markdown: a picture among words is a run that keeps its source", function()
	local s = shapes()
	expect(s["raw:Never read: ![a URL](https://e.com/x.png) and more."]).to_be("p:Never read: [https://e.com/x.png] and more.")
	expect(s["raw:```\n![a](a.png)\n```"]).to_be("code")
end)

test("find parses as markdown draws: pictures together are one block", function()
	local s = shapes()
	expect(s["find:Three:\n![a](a.png)\n![b](b.png)\nDone."]).to_be("p:Three: pics:a.png,b.png p:Done.")
	expect(s["find:![a](a.png)\n\n![b](b.png)"]).to_be("pics:a.png,b.png")
	expect(s["find:![a](a.png)\n\nWords.\n\n![b](b.png)"]).to_be("img:a.png p:Words. img:b.png")
end)

test("find counts a paragraph's words, not a picture's alt, and no match spans a picture", function()
	local s = shapes()
	expect(s["count:A shot ![shot](a.png) of the shot.|shot"]).to_be(2)
	expect(s["count:Fi![x](a.png)sh|fish"]).to_be(0)
end)
