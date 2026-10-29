One item in the left nav. A clickable row with an icon and a name, and a count or status dot at the right end.

## When to use
- Every item inside `Nav` is drawn with this (the items of `navLocal()` and `ggsNav()`).
- The settings item placed in `Nav`'s `footer` is also a single NavRow. To give it the same shape and the same selected look as the nav items, do not build a separate button by hand.

## Props
- `item` (required): `{ id, label, icon, hint?, count?, alert?, dot? }`. `hint` is a shortcut ("⌘,") shown in the title together with the name. `count` is the number at the right end, `alert` gives that number a `bad` background, and `dot` is the dot at the right end (`gold` / `bad`).
- `active` (required): selected. `onSelect(id)`: when clicked.

## Rules
- Height `h-field`, corners `r-2`, text `fs-4`, `sp-3` between the 16px icon and the name. Selected is `on-accent` text at weight 600 on an `accent-dim` background.
- Has `k-nav-row` and `k-row`. In the 48px column at widths of 1040px or less, the name disappears leaving only the centred icon, and counts and dots overlap the icon's corner (held by CSS, so do not write it in style).

## Similar components
- `Nav`: the left column itself that lays out the items. NavRow is one row of it.

Used in: `gui/src-ui/js/components/ggs.tsx` (the items of `Nav`), `gui/src-ui/js/App.tsx` ("Settings" at the bottom of the left nav).
