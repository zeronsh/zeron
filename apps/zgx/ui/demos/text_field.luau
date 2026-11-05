-- Text field: every piece and state.

local ds = require("@ds")
local kit = require("@demos/kit")

local name = ui.state("tf_name", "")
local email = ui.state("tf_email", "ada@")
local password = ui.state("tf_password", "correct horse")
local token = ui.state("tf_token", "zrn_live_4f9a")
local url = ui.state("tf_url", "")
local small = ui.state("tf_small", "")
local large = ui.state("tf_large", "")
local readonly = ui.state("tf_disabled", "Can't change this")
local branch = ui.state("tf_branch", "zeron/primitives")

return {
	id = "text_field",
	title = "Text field",
	about = "One line of text in the field's frame: label, hint, error, count, a leading icon, clear and reveal. Hover firms the hairline; focus takes the accent and the ring.",
	demo = function()
		return kit.page(
			kit.block(
				"WITH A LABEL AND A HINT",
				ui.row {
					gap = "$space.6",
					items = "start",
					flex_wrap = true,
					kit.form(
						ds.TextField { value = name, label = "Name", placeholder = "Ada Lovelace", hint = "As it shows to your team." },
						ds.TextField {
							value = email,
							label = "Email",
							placeholder = "you@example.com",
							icon = "global",
							inputmode = "email",
							error = function()
								local v = email:get()
								return if v ~= "" and not string.find(v, "@.+%.") then "That isn't a full address yet." else nil
							end,
						}
					),
					kit.form(
						ds.TextField { value = password, label = "Password", password = true, hint = "At least 12 characters." },
						ds.TextField {
							value = branch,
							label = "Branch",
							icon = "git-branch",
							clearable = true,
							count = {
								of = function()
									return #branch:get()
								end,
								soft = 40,
								max = 60,
							},
						}
					),
				}
			),
			kit.block(
				"SIZES",
				kit.row(
					ds.TextField { value = small, size = "sm", placeholder = "Small", w = 180 },
					ds.TextField { value = url, placeholder = "Medium", w = 200 },
					ds.TextField { value = large, size = "lg", placeholder = "Large", w = 220 }
				)
			),
			kit.block(
				"PIECES",
				kit.row(
					ds.TextField { value = token, icon = "lock-keyhole-unlocked", clearable = true, w = 240, name = "Token" },
					ds.TextField { value = url, placeholder = "Jump to…", icon = "map-arrow-right", trailing = ds.Kbd { keys = "cmd+k" }, w = 220 },
					ds.TextField { value = readonly, disabled = true, w = 200, name = "Disabled" }
				)
			)
		)
	end,
}
