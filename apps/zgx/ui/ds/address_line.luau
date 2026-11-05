-- The line under a name's field: the address the name makes, and at its
-- end what's known of it (being checked, free, or taken), a glyph and a
-- word in the ink of the news. The slot keeps one line's height whatever it
-- says, and says nothing for any other `status`. Setup's Name your
-- organization uses it, and the gallery's.
--
--   AddressLine { prefix = "zeron.sh/", value = function() return slug end, status = function() return "free" end }

local Icon = require("@self/icon")
local progress = require("@self/progress")
local locale = require("@self/locale")

export type Props = {
	prefix: string,
	value: () -> string,
	-- "checking", "free", "taken", or anything else (nothing shows).
	status: () -> string,
}

return function(props: Props): Node
	local function show(kind: string, glyph: Node, label: string | Message, ink: string): Node
		return ui.show(function()
			return props.status() == kind
		end, ui.row {
			gap = "$space.1",
			items = "center",
			shrink = 0,
			color = ink,
			glyph,
			ui.text { label, text_size = "$font.size.sm", line_height = "$font.line.sm" },
		})
	end
	return ui.row {
		w_full = true,
		items = "center",
		justify = "between",
		gap = "$space.3",
		ui.row {
			items = "center",
			min_w = 0,
			ui.text { props.prefix, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.tertiary" },
			ui.text {
				props.value,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = function()
					return if props.status() == "taken" then "$zeron.tone.danger.ink" else "$text.secondary"
				end,
			},
		},
		ui.row {
			items = "center",
			show("checking", progress.ProgressRing { size = 12, tone = "neutral", label = locale.t("ds.address_line.checking_label") }, locale.t("ds.address_line.checking"), "$text.tertiary"),
			show("free", ui.div { y = -0.5, Icon { name = "check-circle", size = 12 } }, locale.t("ds.address_line.available"), "$zeron.tone.success.ink"),
			show("taken", ui.div { y = -0.5, Icon { name = "close-circle", size = 12 } }, locale.t("ds.address_line.taken"), "$zeron.tone.danger.ink"),
		},
	}
end
