-- A template's own theme: a palette for each scheme it has, which
-- ds/palettes maps onto every colour token (ds/tokens.setup), over the
-- design system's themes (ds/themes/*.json), whose glass and shape it keeps.
--
-- The default interface wears its own themes as they are, so this returns
-- nil. A template that brings colours returns `{ name, dark = Palette?,
-- light = Palette? }` in ds/palettes' shape, as data only (no code: it's
-- read as data for the Themes page). It's worn for a scheme only where you
-- haven't chosen a theme for that scheme (Settings → Themes), and is listed
-- there as the template's theme. A theme for one scheme fills only that one.

return nil
