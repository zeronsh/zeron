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
	-- What the store needs of one before it's published, in the order a person
	-- fixes them (each passes unless a problem the engine found names it).
	checks: { Check },
	-- The listing's fields whose fixes it has words for (`fix.missing_listing`,
	-- `fix.bad_listing`), and the engine's rules (`fix.<rule>`).
	fields: { [string]: boolean },
	rules: { [string]: boolean },
	-- Fields one has none of (in its words: take it out).
	none_of: { [string]: boolean }?,
}

-- (`rule` "listing": missing or bad, `field` which.)
export type Check = { id: string, rule: string, field: string? }

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
