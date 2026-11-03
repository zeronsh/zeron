-- Icons: Zeron's own (crates/ui/assets/icons, Solar Linear), drawn in the
-- colour of the text around them (a button's, hover included).
-- `Icon { name = "plus", size = 16 }`.

export type Name =
	"plus"
	| "arrow-right"
	| "arrow-up-right"
	| "check"
	| "clipboard"
	| "close"
	| "copy"
	| "more-horizontal"
	| "pen"
	| "refresh"
	| "settings-minimalistic"
	| "settings"
	| "trash-bin-minimalistic"
	| "folder"
	| "alt-arrow-down"
	| "alt-arrow-left"
	| "alt-arrow-right"
	| "alt-arrow-right-bold"
	| "arrow-up"
	| "microphone"
	| "git-branch"
	| "worktree"
	| "shield-check"
	| "pen-new-square"
	| "lock-keyhole-unlocked"
	| "map-arrow-right"
	| "access-supervised"
	| "access-edits"
	| "access-full"
	| "monitor"
	| "cloud"
	| "wifi-off"
	| "fast-tier"
	| "fast-tier-bold"
	| "star"
	| "star-bold"
	| "status-working"
	| "status-input"
	| "status-approval"
	| "status-failed"
	| "status-done"
	| "archive-minimalistic"
	| "archive-up-minimalistic"
	| "pin"
	| "bell"
	| "clock-circle"
	| "pull-request"
	| "pull-request-draft"
	| "search"
	| "arrow-left"
	| "arrow-right"
	| "laptop"
	| "list"
	| "bot"
	| "remote-server"
	| "project-default"
	| "calendar"
	| "keyboard"
	| "palette"
	| "palette-search"
	| "split-columns"
	| "volume-loud"
	| "moon"
	| "sun"
	| "terminal"
	| "document"
	| "file-diff"
	| "document-add"
	| "global"
	| "checklist"
	| "chat-round-line"
	| "side-chat"
	| "folder-with-files"
	| "widget"
	-- The emoji picker's kinds (smileys, people, nature, food, travel,
	-- activities, objects, symbols, flags) and an upload.
	| "emoji-funny-circle"
	| "hand-shake"
	| "leaf"
	| "donut-bitten"
	| "bus"
	| "football"
	| "lightbulb-minimalistic"
	| "heart-angle"
	| "flag"
	| "upload-minimalistic"
	| "arrow-down"
	| "alt-arrow-up"
	| "stop"
	| "action-play"
	| "action-test"
	| "action-lint"
	| "action-configure"
	| "action-build"
	| "action-debug"
	| "eye"
	| "eye-closed"
	| "collapse-arrows"
	| "expand-arrows"
	| "minus"
	| "wrap-text"
	| "danger-triangle"
	-- A row's grip, dragged to reorder it (the view menu's Organize rows).
	| "grip"
	| "file-code"
	| "info-circle"
	| "info-square"
	| "git-merge"
	| "git-commit"
	| "pull-request-closed"
	| "close-circle"
	| "check-circle"
	| "tag"
	| "inbox"
	| "chat-add"
	-- Settings' pages, as Zeron's settings sidebar names them: a person, a
	-- group of people, tuning dials, a plug, a wallet.
	| "user-rounded"
	| "users-group-rounded"
	| "tuning"
	| "plug-circle"
	| "wallet"
	| "smartphone"
	-- A picture to keep (saving a card as an image).
	| "gallery"
	-- The composer's cargo and trays: an element picked on a page (an
	-- annotation), a skill, a command, a queued row's grip, a file attached,
	-- and trying again.
	| "cursor"
	| "magic-stick"
	| "command"
	| "drag-handle"
	| "paperclip"
	| "restart"
	| "scissors"

-- `name` may be a function: the icon follows it (a checkout's folder or
-- worktree).
export type Props = { name: Name | () -> Name, size: number? }

-- One glyph for search everywhere: the sidebar's (an older name for the
-- magnifying glass draws it too).
local SAME: { [string]: string } = { magnifer = "search" }
-- Each glyph's SVG travels as code (ds/icon_svgs, made from
-- assets/icons/ui by tools/icon_svgs.py): a plugin lent the design system
-- loads no file outside its own folder, and draws these all the same.
local SVGS: { [string]: string } = require("@ds/icon_svgs")
local function file(name: string): string
	local key = SAME[name] or name
	local code = SVGS[key]
	return if code then `code:{code}` else `assets/icons/ui/{key}.svg`
end

return function(props: Props): Node
	local size = props.size or 16
	local name = props.name
	return ui.svg {
		src = if type(name) == "function"
			then function()
				return file(name())
			end
			else file(name),
		w = size,
		h = size,
		shrink = 0,
		-- A glyph says nothing a screen reader needs: what holds it is named.
		aria_hidden = true,
		tint = "currentColor",
	}
end
