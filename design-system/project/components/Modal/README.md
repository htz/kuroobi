A dialog box. It consists of a heading band, the body, and a band of buttons at the bottom.

## When to use
- Confirmation (the content of `Confirm`), loading a game record, asking for a game, viewing a game record, adding and editing a server. Always place it inside an `Overlay`.
- Not for information that is always shown on the screen (`Dock` or `Section`).

## Props
- `title` (required): the heading.
- `sub`: a small note below the heading.
- `body`: the body (descriptive text).
- `children`: content placed below the body (input fields, lists, etc.).
- `actions`: the buttons at the bottom. To push them right, put an empty `span` with `margin-left: auto` first.
- `width` (default `var(--w-modal)`): ones with content (the request, the game record) use `var(--w-modal-wide)`.
- `onClose`: when given, a close button (×) appears to the right of the heading.
- `scroll`: makes the body box `k-scroll` so the overflow scrolls.
- `band`: a band below the heading (the game record viewer's "Board 1" and "Board 2").

## Rules
- Corners `r-4`, background `bg`, border `border`, shadow `sh-2`. Maximum height 88vh.
- The heading band and the `band` band are `h-bar`. The heading band is `panel`, `band` is `card`. The heading is `fs-4`, weight 600; `sub` is `fs-7`.
- Body padding is `sp-4` vertical and `sp-5` horizontal, inner spacing `sp-3`. `body` text is `fs-5`, `sub`, line height 1.7.
- The button band is `panel`, with padding `sp-3` vertical and `sp-5` horizontal and spacing `sp-2`. Buttons are `size="field"`. The confirming button is a verb; dangerous actions use `variant="danger"`, everything else `primary`. The dismissing "Cancel" is normal (`secondary`) in `Confirm` and when loading a game record, and `ghost` in the request sheet and server editing. The game record viewer's "Close" is `ghost`.

## Similar components
- `Overlay` handles the scrim and the interaction (Esc, closing by clicking the scrim, focus trapping). Modal is only the visual box and has no closing behaviour.

Used in: `gui/src-ui/js/AskSheet.tsx` (asking for a game), `Dialogs.tsx` (loading a game record), `KifuViewer.tsx` (game record), `SettingsScreen.tsx` (adding and editing a server), `components/layout.tsx` (`Confirm`).
