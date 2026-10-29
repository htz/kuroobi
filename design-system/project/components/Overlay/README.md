The scrim laid under a dialog. It darkens and covers the screen, centres the content, and handles closing and focus trapping.

## When to use
- Always place it outside a `Modal` when showing one. The content is usually a single `Modal`.

## Props
- `children` (required): the content placed in the centre.
- `onClose`: the closing handler. When given, it is called on the Esc key and when the scrim is pressed and released on the scrim (pressing inside the box and releasing on the scrim does not close it). When Overlays are stacked, only the topmost one closes on Esc.

## Rules
- With `position: absolute; inset: 0` it covers the nearest `position: relative` ancestor (in the app, `AppFrame`). It is not `fixed`, so when placing it on its own, put it in a relative box that has a height.
- The scrim colour is `scrim` (translucent black regardless of theme), padding `sp-5`, stacking z-index 30 (above `Toasts` at 27, `Drawer` at 26 and `Dock` at 5).
- When opened, it moves focus to the first input field inside (or, if none, the first focusable element) and traps Tab / Shift+Tab inside. When closed, it returns focus to the element focused before opening.

## Similar components
- The visual box is `Modal`. For a temporary notice that should not block interaction, use `Toasts`.

Used in: `gui/src-ui/js/AskSheet.tsx` (asking for a game), `Dialogs.tsx` (loading a game record), `KifuViewer.tsx` (game record), `SettingsScreen.tsx` (adding and editing a server), `components/layout.tsx` (`Confirm`).
