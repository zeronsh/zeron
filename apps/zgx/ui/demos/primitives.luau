-- Gallery groups of pages owned by one chat's work (primitives): a list of
-- { title, pages }, each page a { id, title, about, demo, fill? } (see
-- gallery.luau's `Page`). The gallery appends these groups to its own.
-- Put each page in a file of its own under demos/ and require it here.
--
-- Each page loads on its own: one that fails to load (a slip mid-edit)
-- shows what went wrong in its place, and the rest of the gallery, every
-- other window sharing these files included, carries on.

local function page(id: string, title: string, load: () -> any): any
	local ok, result = pcall(load)
	if ok then
		return result
	end
	return {
		id = id,
		title = title,
		about = "This page didn't load.",
		demo = function()
			return ui.text { tostring(result), literal = true, wrap = true, font = "$font.mono", text_size = "$font.size.sm", color = "$danger" }
		end,
	}
end

return {
	{
		title = "Fields",
		pages = {
			page("text_field", "Text field", function()
				return require("@demos/text_field")
			end),
			page("text_area", "Multi-line field", function()
				return require("@demos/text_area")
			end),
			page("search_field", "Search field", function()
				return require("@demos/search_field")
			end),
			page("checkbox", "Checkbox and radio", function()
				return require("@demos/checkbox")
			end),
			page("choices", "Select, slider, stepper", function()
				return require("@demos/choices")
			end),
			page("date_picker", "Date picker", function()
				return require("@demos/date_picker")
			end),
			page("tabs", "Tabs", function()
				return require("@demos/tabs")
			end),
		},
	},
	{
		title = "Status",
		pages = {
			page("badges", "Badges and tags", function()
				return require("@demos/badges")
			end),
			page("notices", "Callouts, banners, toasts", function()
				return require("@demos/notices")
			end),
			page("private_text", "Private text", function()
				return require("@demos/private_text")
			end),
			page("feedback", "Empty state and progress", function()
				return require("@demos/feedback")
			end),
			page("overlays", "Dialog and sheet", function()
				return require("@demos/overlays")
			end),
			page("lightbox", "Lightbox", function()
				return require("@demos/lightbox")
			end),
			page("reply_pictures", "Reply pictures", function()
				return require("@demos/reply_pictures")
			end),
		},
	},
	{
		title = "Building blocks",
		pages = {
			page("settings", "Settings", function()
				return require("@demos/settings")
			end),
			page("onboarding_blocks", "Onboarding", function()
				return require("@demos/onboarding")
			end),
			page("structure", "Structure and actions", function()
				return require("@demos/structure")
			end),
			page("git", "Git pieces", function()
				return require("@demos/git")
			end),
		},
	},
	{
		title = "Examples",
		pages = {
			page("ex_way_in", "Intro: the way in", function()
				return require("@demos/example_way_in")
			end),
			page("ex_signin", "Sign in", function()
				return require("@demos/example_signin")
			end),
			page("ex_username", "Claim your username", function()
				return require("@demos/example_username")
			end),
			page("ex_card", "Your card", function()
				return require("@demos/example_card")
			end),
			page("ex_org_name", "Name your organization", function()
				return require("@demos/example_org_name")
			end),
			page("ex_invite", "Invite your teammates", function()
				return require("@demos/example_invite")
			end),
			page("ex_orgs", "Choose an organization", function()
				return require("@demos/example_orgs")
			end),
			page("ex_done", "All set", function()
				return require("@demos/example_done")
			end),
			-- This device's setup (onboarding.rs: Appearance, Sync, GitHub
			-- when signed in, Providers, then the ending).
			page("ex_appearance", "Make it yours", function()
				return require("@demos/example_appearance")
			end),
			page("ex_sync", "Sync your work", function()
				return require("@demos/example_sync")
			end),
			page("ex_github", "Connect GitHub", function()
				return require("@demos/example_github")
			end),
			page("ex_setup_agents", "Your agents", function()
				return require("@demos/example_setup_agents")
			end),
			page("ex_setup_done", "You're all set", function()
				return require("@demos/example_setup_done")
			end),
			page("ex_plugins", "Plugins", function()
				return require("@demos/example_plugins")
			end),
			page("ex_members", "Members", function()
				return require("@demos/example_members")
			end),
			page("ex_profile", "Profile", function()
				return require("@demos/example_profile")
			end),
			page("ex_providers", "Providers", function()
				return require("@demos/example_providers")
			end),
			page("ex_add_agent", "Add an agent", function()
				return require("@demos/example_add_agent")
			end),
			page("ex_resets", "Codex resets", function()
				return require("@demos/example_resets")
			end),
			page("ex_git_commit", "Commit", function()
				return require("@demos/example_git_commit")
			end),
			page("ex_git_pr", "Pull request", function()
				return require("@demos/example_git_pr")
			end),
			page("ex_git_review", "Review", function()
				return require("@demos/example_git_review")
			end),
			page("ex_git_merge", "Merge", function()
				return require("@demos/example_git_merge")
			end),
		},
	},
}
