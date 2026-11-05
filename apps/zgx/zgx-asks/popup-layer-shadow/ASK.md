# Ask: a node tooltip's popup sees through its clear layer

**Done:** zgx `b75d8e74f` (#1390), in Zeron's pin `0d42292ef`. Adopted: ds/hover_card is the menus' glass.

**Priority: medium.** A hover card can't be glass, and a card with its own shadow gets a second frame.

**Seen on:** zgx `f6506b6e6` (Zeron's pin), macOS 26.

**Today.** A node `tooltip` (`tooltip = card`) shows in a popup window whose panel is the tooltip's clear layer (`compose_tooltip::wrap`: `popover(..).layer()`, the card a child of a fade column inside it). zgx-desktop's `popup_window` decides the window from the root's *first child* only:

- `window_shadow(scene, root)`: the window server shadows the window unless that child paints a shadow. A card one level down painting its own shadow isn't seen, so the window takes in the shadow's soft ink and the window server outlines all of it: a dark rim round a box the shadow's reach bigger than the card (28 at the sides, 8 above, 48 below for `0 20 56`). In Zeron that read as a second, bigger frame round the context card.
- `Material::of`: AppKit's material (the only blur a popup window can have) is set from that child's fill and corner, so a translucent card inside the layer gets no blur, and a layer with a clear fill would get a square material of its own box.

So an app can't give a hover card the menus' look (a translucent fill over a blur, its own shadow): only a solid card with no shadow, the window server's shadow its depth. Zeron does that meanwhile (ds/hover_card; tests/hover_cards.rs keeps every node tooltip on it).

**Ask:** when the panel is a clear layer with one child (a tooltip's, through its fade column), take that child as the panel for `window_shadow` and `Material::of`: its shadow (window shadow off, as for a menu painting its own), its fill and corner (AppKit's material under it). Or let a node tooltip style its panel (`tooltip_panel = { … }`), as `ui.popover`'s props style a menu's.

**Repro:** `ZGX_CHECK=0 zgx run zgx-asks/popup-layer-shadow/repro`, rest the pointer on the button: the card shows inside a dark rim, the rim the card's shadow's reach bigger than it. Remove the card's `shadow`: one frame.
