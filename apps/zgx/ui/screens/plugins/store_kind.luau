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
}

return {}
