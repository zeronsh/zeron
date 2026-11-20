-- LaTeX math in replies (ds/math, ds/markdown): the source read before
-- markdown is, formulas drawn where their TeX was, copy giving the TeX
-- back, and the TeX shown as code where it can't be set or math is off. On
-- the page in ui/math_fixture.luau.

local Source = require("@ds/math_source")

test("display math becomes a math block, inline math inline code, prices stay text", function()
	local out = Source.prepare("Say $x^2$ here.\n\n$$\\frac{a}{b} \\\\ c$$\n\nIt costs $5 and $10, or \\$3.")
	expect(out).to_contain("Say `$x^2$` here.")
	expect(out).to_contain("```math\n\\frac{a}{b} \\\\ c\n```")
	expect(out).to_contain("It costs $5 and $10, or \\$3.")
	-- Mid-paragraph display math still gets lines of its own.
	expect(Source.prepare("before $$x$$ after")).to_be("before \n```math\nx\n```\n after")
	-- Code keeps its dollars.
	expect(Source.prepare("```sh\necho $HOME $$\n```\nand `a $b$ c`")).to_be("```sh\necho $HOME $$\n```\nand `a $b$ c`")
	-- A `$$` still open (streaming) is left as it is.
	expect(Source.prepare("so $$\\frac{1}{")).to_be("so $$\\frac{1}{")
	-- No space just inside the dollars.
	expect(Source.prepare("a $ b $ c")).to_be("a $ b $ c")
	-- Backticks in the TeX get a longer fence.
	expect(Source.prepare("$a`b$")).to_be("``$a`b$``")
end)

test("skills invoked in a prompt stay links, never a formula between their dollars", function()
	local sent = "run [$commit](zeron-invoke:7b22) and [$gh-pr](zeron-invoke:7b23) to main"
	expect(Source.prepare(sent)).to_be(sent)
	-- math beside them is still math
	expect(Source.prepare("[$a](zeron-invoke:7b) costs $x^2$")).to_be("[$a](zeron-invoke:7b) costs `$x^2$`")
end)

local function open(): any
	local app = mount("math_fixture.luau")
	app:resize(900, 700)
	app:frames(20)
	return app
end

test("a reply's display math is drawn, labelled with its TeX; what can't be set shows as code", function()
	local app = open()
	local tree = app:tree()
	expect(app:has('role:img "\\frac{a}{b} \\\\ \\{x\\}"')).to_be(true)
	-- The unknown command's block is the code block it is.
	expect(tree).to_contain("nosuch")
	-- Inline math shows as its TeX for now; prices are text.
	expect(tree).to_contain("$e^{i")
	expect(tree).to_contain("pi} + 1 = 0$")
	expect(tree).to_contain("$not math$")
end)

-- (Copying is the selection's, through the window's menu keys, which a test
-- host has none of: what's checked here is that the formula's TeX is text
-- in the selectable reply, unseen, where the formula is.)
test("a formula's TeX is text in the reply, unseen, over the formula", function()
	local app = open()
	-- The formula's box and its TeX's, one over the other.
	local img, text
	for line in app:tree("#reply"):gmatch("[^\n]+") do
		if line:find(" img ", 1, true) and line:find("frac", 1, true) then
			img = line:match(" (%-?%d+,%-?%d+) ")
		elseif line:find('text "$$', 1, true) and line:find("frac", 1, true) then
			text = line:match(" (%-?%d+,%-?%d+) ")
		end
	end
	expect(img).never.to_be_nil()
	expect(text).never.to_be_nil()
end)

test("with math off, display math is its code block", function()
	local app = open()
	app:command("math_off")
	app:frames(20)
	expect(app:has('role:img "\\frac{a}{b} \\\\ \\{x\\}"')).to_be(false)
	expect(app:tree()).to_contain("frac{a}{b}")
end)
