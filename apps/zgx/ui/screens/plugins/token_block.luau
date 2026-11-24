--!strict
-- A new deploy token as it's shown, once: as the app shows a line of code to
-- copy (a chat reply's code block: its strip, a label and Copy, over its line
-- in the code face, wrapped so the whole token shows to check by eye), drawn
-- as private text: blurred while addresses are hidden (the screen may be
-- seen), shown by a click, and never in a picture an agent is handed
-- (src/plugins `PRIVATE_REGIONS`). Copy puts it on the clipboard as a secret
-- (src/secrets.rs), whether it's shown or not. (Not a Markdown block: no copy
-- of it is kept with the block.)
--
--   token_block.Block { secret, copy, copied }   `secret()` the token (nil:
--                         none); `copy(token)` the promise of it on the
--                         clipboard; `copied` set once it's copied

local ds = require("@ds")
local control = require("@ds/control")
local locale = require("@ds/locale")
local PrivateText = require("@ds/private_text")

local M = {}

M.Block = ui.component(function(props: { secret: () -> string?, copy: (token: string) -> any, copied: State<boolean> }): Node
	local secret, copied_once = props.secret, props.copied
	-- Copied: its Copy says so a moment; its line under the block stays.
	local copied = ui.use_state(false)
	return ui.column {
		w_full = true,
		gap = "$space.3",
		ui.column {
			id = "key-secret",
			w_full = true,
			min_w = 0,
			rounded = "$radius.xl",
			bg = "$zeron.prose.code.bg",
			border = 1,
			border_color = "$zeron.prose.code.edge",
			overflow = "hidden",
			ui.row {
				h = 34,
				pl = "$space.4",
				pr = ui.raw(5, "even with Copy's room in the 34 strip, as a code block's"),
				items = "center",
				gap = "$space.1",
				selectable = false,
				ui.text {
					"token",
					literal = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
					grow = 1,
					min_w = 0,
				},
				ui.button {
					id = "key-secret-copy",
					label = locale.t("ds.markdown.copy_code"),
					h = 24,
					px = "$space.2",
					gap = "$space.1",
					items = "center",
					rounded = ui.raw(control.nest("$radius.xl", 1, 5), "concentric with the block: its corner less its edge and inset, as a code block's Copy"),
					bg = "#00000000",
					color = "$text.tertiary",
					text_size = "$font.size.xs",
					hover = { bg = "$hover", color = "$text.primary" },
					active = { bg = "$pressed" },
					focus = { border = 0 },
					on_click = function()
						local token = secret()
						if not token then
							return
						end
						props.copy(token):after(function()
							copied:set(true)
							copied_once:set(true)
							ui.after(1400, function()
								copied:set(false)
							end)
						end, function()
							ds.toast.show { title = ui.t("plugins.store.key.copy_failed"), tone = "warning" }
						end)
					end,
					ds.Icon {
						name = function(): string
							return if copied:get() then "check" else "copy"
						end,
						size = 12,
					},
					ui.text(function(): string
						return if copied:get() then locale.s("ds.markdown.copied") else locale.s("common.copy")
					end),
				},
			},
			PrivateText {
				function(): string
					return secret() or ""
				end,
				name = ui.t("plugins.store.key.show_token"),
				font = "$font.mono",
				text_size = "$zeron.prose.code.size",
				line_height = "$zeron.prose.code.line",
				color = "$text.primary",
				wrap = true,
				w_full = true,
				pl = "$space.4",
				pr = "$space.5",
				pb = "$space.3",
			},
		},
		-- Copied: how long it stays there.
		ui.show(function(): boolean
			return copied_once:get()
		end, ui.text {
			id = "key-secret-copied",
			ui.t("plugins.store.key.copied"),
			w_full = true,
			wrap = true,
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			color = "$text.tertiary",
		}),
	}
end)

return M
