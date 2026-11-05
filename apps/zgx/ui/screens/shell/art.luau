-- Whether a new chat's wallpaper shows now: the page that draws it
-- (screens/shell/page) keeps this, and what stands over the art (the
-- composer's footer chips, their backing) reads it without loading the
-- appearance settings behind it.
--
--   art.showing   a state: true while the wallpaper is drawn

return {
	showing = ui.state("shell_art_showing", false),
}
