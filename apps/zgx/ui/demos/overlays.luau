-- Dialogs and sheets.

local dialog = require("@ds/dialog")
local Button = require("@ds/button")
local TextField = require("@ds/text_field")
local TextArea = require("@ds/text_area")
local Select = require("@ds/select")
local Checkbox = require("@ds/checkbox")
local kit = require("@demos/kit")

local ask = ui.state("ov_ask", false)
local discard = ui.state("ov_discard", false)
local delete = ui.state("ov_delete", false)
local sheet = ui.state("ov_sheet", false)
local bottom = ui.state("ov_bottom", false)
local name = ui.state("ov_name", "")
local title = ui.state("ov_title", "Design system primitives")
local body = ui.state("ov_body", "")
local base = ui.state("ov_base", "main")
local draft = ui.state("ov_draft", false)

return {
	id = "overlays",
	title = "Dialog and sheet",
	about = "Panels that hold the window until answered: focus stays in them, Escape or a click outside closes them, and focus goes back where it was. A destructive confirm is red and starts on Cancel; one can ask for a word to be typed first.",
	demo = function()
		return kit.page(
			kit.block(
				"DIALOGS",
				kit.row(
					Button { "Rename", tone = "secondary", on_click = ui.set(ask, true) },
					Button { "Discard changes", tone = "secondary", on_click = ui.set(discard, true) },
					Button { "Delete branch", tone = "danger", on_click = ui.set(delete, true) }
				)
			),
			kit.block(
				"SHEETS",
				kit.row(
					Button { "New pull request", tone = "secondary", on_click = ui.set(sheet, true) },
					Button { "From the foot", tone = "ghost", on_click = ui.set(bottom, true) }
				)
			),
			dialog.Dialog {
				open = ask,
				title = "Rename the chat",
				confirm = { label = "Rename" },
				TextField { value = name, placeholder = "Design system primitives", w_full = true, autofocus = true, name = "Name" },
			},
			dialog.Dialog {
				open = discard,
				title = "Discard 3 changes?",
				body = "The edits to badge.luau, tokens.luau and zeron-dark.json go for good. Stashed and committed work stays.",
				tone = "danger",
				confirm = { label = "Discard" },
			},
			dialog.Dialog {
				open = delete,
				title = "Delete zeron/primitives?",
				body = "The branch goes from this Mac and from origin. Its pull request closes.",
				tone = "danger",
				size = "md",
				confirm_text = "zeron/primitives",
				confirm = { label = "Delete branch" },
			},
			dialog.Sheet {
				open = sheet,
				title = "New pull request",
				description = "From zeron/primitives into the base you choose.",
				foot = ui.row {
					gap = "$space.2",
					Button { "Cancel", tone = "secondary", on_click = ui.set(sheet, false) },
					Button { "Create pull request", on_click = ui.set(sheet, false) },
				},
				TextField { value = title, label = "Title", w_full = true },
				TextArea { value = body, label = "Description", placeholder = "What changed, and why", rows = 5, max_rows = 14, w_full = true },
				Select {
					value = base,
					label = "Base",
					w_full = true,
					options = { { value = "main", label = "main" }, { value = "release/0.3", label = "release/0.3" } },
				},
				Checkbox { value = draft, label = "Open as a draft", description = "Reviewers aren't asked until it's ready." },
			},
			dialog.Sheet {
				open = bottom,
				title = "Run output",
				side = "bottom",
				size = 300,
				ui.text { "pre-commit: luau-lsp … ok\npre-commit: zgx lint … ok", font = "$font.mono", text_size = "$font.size.sm", color = "$text.secondary", wrap = true },
			}
		)
	end,
}
