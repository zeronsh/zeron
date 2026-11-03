-- The gallery's Math page: LaTeX formulas as replies set them (ds/math),
-- in each face and size the settings offer, and what a reply shows for TeX
-- that can't be set (its source, as code).

local ds = require("@ds")
local math_ = require("@ds/math")

local SAMPLES = {
	{ tex = "e^{i\\pi} + 1 = 0", display = false },
	{ tex = "\\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}", display = true },
	{ tex = "\\sum_{n=1}^{\\infty} \\frac{1}{n^2} = \\frac{\\pi^2}{6}", display = true },
	{ tex = "\\int_0^1 x^2 \\, dx = \\left[ \\frac{x^3}{3} \\right]_0^1", display = true },
	{ tex = "\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}^{-1} = \\frac{1}{ad - bc} \\begin{pmatrix} d & -b \\\\ -c & a \\end{pmatrix}", display = true },
	{ tex = "\\mathbb{R}^n \\to \\mathcal{L}(V, W), \\quad \\alpha \\le \\beta", display = false },
	{ tex = "x + \\nosuchcommand{y}", display = true },
}

-- A sample: its formula, or its TeX as code where it can't be set.
local function sample(s: { tex: string, display: boolean }): Node
	local formula = math_.Math { tex = s.tex, display = s.display }
	return ui.column {
		w_full = true,
		gap = "$space.2",
		ui.text {
			if s.display then "Display" else "Inline",
			text_size = "$font.size.sm",
			color = "$text.tertiary",
		},
		formula or ui.text {
			s.tex,
			literal = true,
			wrap = true,
			font = "$font.mono",
			text_size = "$font.size.sm",
			color = "$text.secondary",
		},
	}
end

-- The samples (each follows the face and size chosen in place).
local function samples(): Node
	local list: { Node } = {}
	for _, s in SAMPLES do
		table.insert(list, sample(s))
	end
	return ui.column { w_full = true, gap = "$space.5", table.unpack(list) }
end

local face_options: { any } = {}
for _, f in math_.FACES do
	table.insert(face_options, { value = f.value, label = f.label })
end
local size_options: { any } = {}
for _, n in math_.SIZES do
	table.insert(size_options, { value = n, label = `{n}%` })
end

return {
	id = "math",
	title = "Math",
	about = "LaTeX in replies, set by the shipping app's layout over its bundled math faces, in the text's ink. TeX that can't be set shows as its source.",
	demo = function(): Node
		return ui.column {
			w_full = true,
			gap = "$space.6",
			ui.row {
				gap = "$space.3",
				items = "center",
				flex_wrap = true,
				ds.Select { value = math_.face, name = "Math font", size = "sm", w = 180, options = face_options },
				ds.Select { value = math_.size, name = "Math size", size = "sm", w = 100, options = size_options },
			},
			samples(),
		}
	end,
}
