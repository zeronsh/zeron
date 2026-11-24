--!strict
-- What the store's screens (screens/plugins/store_browse, store_listing,
-- store_publish) take to be one kind's: its store, where its pages are, and
-- what this Zeron has of it. The plugins' is screens/plugins/kind.

export type Kind = {
	id: "plugin" | "template",
	-- What its state is named by, so neither kind's is the other's.
	keys: string,
	store: any,
	nav: any,
	-- What's installed, and what may be done with it (the plugins':
	-- screens/plugins/state).
	state: any,
	-- The settings of one that's on, if its kind has any.
	settings: any?,
	-- Its own sentences, by the plugins' (none: those).
	words: { [string]: string }?,
	-- Shown as pictures in a grid, its cover first; and said over its list.
	cards: boolean?,
	fact: string?,
	-- Whether the store sorts it into categories.
	categories: boolean?,
}

-- A sentence as this kind says it: `ui.t`'s key, the plugins' or its own.
export type Say = (key: string, args: { [string]: any }?) -> Message

local M = {}

function M.say(kind: Kind): Say
	local words = kind.words or {}
	return function(key: string, args: { [string]: any }?): Message
		return ui.t(words[key] or key, args)
	end
end

return M
