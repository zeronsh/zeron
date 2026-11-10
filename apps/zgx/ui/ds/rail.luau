-- A scrollbar as Zeron draws one: a thin thumb floating over a scroll
-- view's trailing edge, there while the view scrolls and for a moment
-- after, or while the pointer is on it, and gone otherwise, so a list
-- shows its rows and nothing else. It takes the pointer as a scrollbar
-- does: dragged, the view follows; pressed beside the thumb, the thumb
-- comes under the pointer and follows from there.
--
-- zgx's own bar, drawn and timed in Rust (a scroll runs no script for it),
-- set as the app's (popover.rs's floating rail): a 3-point thumb, 5 under
-- the pointer, 2 in from the edge, its ends 4 in from the view's so it
-- clears a card's corners, lingering 1.4 s after the last scroll and fading
-- over 0.26 s. Hidden, it takes no pointer: what is under it does. It
-- lights up only for a scroll someone makes: a view placing itself as it
-- mounts (a kept place put back as its rows are measured, as a chat opens
-- or the scripts reload) leaves it dark.
--
-- What a scroll view (`ui.list`, `ui.scroll`, `ui.scroll_x`) takes as its
-- `scrollbar`: `ui.list { offset = at, scrollbar = Rail {}, … }`. Along the
-- foot for a view that scrolls sideways. Its view's edge `fade` doesn't
-- fade it.
--
-- `track`: how long its track is, where it stops short of the view's end
-- (a transcript's, above the composer laid over its foot); the view's
-- length by default. `hint`: shown while this holds too, for what can't be
-- seen to scroll until it's tried (a table a pointer rests on).

export type Length = (() -> number) | State<number>

export type Props = {
	track: Length?,
	hint: (() -> boolean)?,
}

return function(props: Props?): any
	local p: Props = props or {}
	return {
		platform = false,
		-- The thumb across, at rest and under the pointer; the strip that
		-- takes the pointer; its room from the edge; the track's ends in from
		-- the view's; the shortest thumb.
		thickness = 3,
		active_thickness = 5,
		strip = 10,
		side = 2,
		ends = 4,
		inset = 0,
		min = 24,
		-- How long it stays once the view rests, and how long it fades (in
		-- as well as out).
		linger = 1400,
		fade = 260,
		fade_in = true,
		ease = "outQuad",
		-- Brighter under the pointer or dragged ($scrollbar.thumb.active), no
		-- track drawn; a press beside the thumb brings it under the pointer.
		active_color = true,
		track_fill = false,
		press = "jump",
		linger_from = "scroll",
		-- A press leaves focus where it was (a composer keeps it), shown as it
		-- scrolls whatever the system's setting, and not a control of its own
		-- for assistive technology (the view itself scrolls for a reader).
		focusable = false,
		always_overlay = true,
		accessible = false,
		track = p.track,
		hint = p.hint,
	}
end
