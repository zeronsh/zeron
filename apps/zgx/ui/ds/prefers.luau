-- The system's display settings, as states: `prefers.reduced_motion:get()`
-- (tracked) or `:peek()`, as zgx's docs read `ui.prefers` (scripting.md,
-- Theming: "each is a tracked state that follows the system").
--
-- Read them here rather than from `ui.prefers`: zgx's type definitions give
-- each a `Readable<boolean>`, whose `get` takes `self: never` and which has
-- no `peek`, so neither call type-checks though both work. This module is
-- the one place that says what they are, until the definitions do.

export type Pref = {
	get: (self: Pref) -> boolean,
	peek: (self: Pref) -> boolean,
}

-- (The definitions' type is wrong for reading, so it's restated here.)
local system = ui.prefers :: any

return {
	reduced_motion = system.reduced_motion :: Pref,
	more_contrast = system.more_contrast :: Pref,
	reduced_transparency = system.reduced_transparency :: Pref,
}
