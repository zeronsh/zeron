-- The colours the interface wears: a palette for each scheme, which
-- ds/palettes maps onto every colour token (ds/tokens.setup), over Zeron's
-- own themes (ds/themes/*.json), whose glass and shape it keeps.
--
-- Zeron's own wears its own themes as they are, so this returns nil. A
-- template that recolours Zeron ships only this file (and its manifest),
-- returning `{ name, dark = Palette?, light = Palette? }` in ds/palettes'
-- shape. With both, the window follows the system's appearance (Settings →
-- Appearance), as Zeron's own does; with one, it wears that one in either
-- scheme. Zeron's built-in themes are templates of this kind
-- (src/plugins/theme_templates.rs).

return nil
