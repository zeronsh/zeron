-- How Markdown's pictures parse (tests/markdown_pictures): each text in
-- CASES as ui.markdown gives it ("raw:<text>") and as ds/markdown_find gives
-- it, which is how ds/markdown draws it ("find:<text>"), each shaped
-- "p:words img:src pics:a,b" (a picture run in words as `[src]`); and how
-- many times find counts a query in the first block ("count:<text>|<q>").

local Find = require("@ds/markdown_find")

local CASES = {
	"Here it is:\n![shot](a.png)",
	"Here it is:\n![shot](a.png)\nAfter.",
	"![a](a.png)\n![b](b.png)",
	"![a](a.png)\n\nAfter.",
	"Three:\n\n![a](a.png)\n![b](b.png)\n![c](c.png)\n\nDone.",
	"Never read: ![a URL](https://e.com/x.png) and more.",
	"```\n![a](a.png)\n```",
	"Three:\n![a](a.png)\n![b](b.png)\nDone.",
	"![a](a.png)\n\n![b](b.png)",
	"![a](a.png)\n\nWords.\n\n![b](b.png)",
}
local COUNTS = {
	{ "A shot ![shot](a.png) of the shot.", "shot" },
	{ "Fi![x](a.png)sh", "fish" },
}

local function shape(list: { any }): string
	local out = {}
	for _, b in list do
		if b.t == "p" then
			local words = {}
			for _, r in b.s do
				table.insert(words, if r.img then `[{r.img}]` else r.x)
			end
			table.insert(out, `p:{table.concat(words)}`)
		elseif b.t == "img" then
			table.insert(out, `img:{b.src}`)
		elseif b.t == "pics" then
			local srcs = {}
			for _, it in b.list do
				table.insert(srcs, it.src)
			end
			table.insert(out, `pics:{table.concat(srcs, ",")}`)
		else
			table.insert(out, b.t)
		end
	end
	return table.concat(out, " ")
end

local shapes: { [string]: any } = {}
for _, text in CASES do
	shapes[`raw:{text}`] = shape(ui.markdown(text))
	shapes[`find:{text}`] = shape(Find.parse(text))
end
for _, c in COUNTS do
	shapes[`count:{c[1]}|{c[2]}`] = Find.count(Find.parse(c[1])[1], c[2])
end
ui.state("fixture_markdown_pictures", {} :: { [string]: any }):set(shapes)

return ui.text { "Markdown pictures" }
