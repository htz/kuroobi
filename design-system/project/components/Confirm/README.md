A yes / no confirmation shown before an action that cannot be undone. Shows a `Modal` over an `Overlay`, with cancel and confirm buttons at the bottom right.

## When to use
- Anything that cannot be reverted once pressed: resign, adjourn, a new game (on a board with a game record), loading onto a board with a game record (replacing the record), playing a different move in the middle of study, undoing learning, switching between analysis and play, resetting a file location to the default, deleting an account or server, and so on.
- Not for: anything that needs input (use `Modal` directly). Anything that only reports a result (a toast).

## Props
- `title` (required): the question ("Start a new game?").
- `body`: what will happen, in one or two sentences.
- `ok` (required): the confirm button's wording. Write it as a verb ("Resign", "New game").
- `danger`: makes the confirm button `danger` (a loss becomes final, data is erased). Defaults to `primary`.
- `onOk` (required), `onCancel` (required). Clicking outside, Esc and the close button at the top right also cancel.

## Rules
- Width is `w-modal`. Title centred, close button at the top right (the `Modal` shape).
- Buttons are right-aligned in the order "Cancel" (a normal `Button`, not ghost) → confirm. Size `field`.
- The confirmation always appears frontmost. Even when asking over another overlay (game record viewer, game record loading), the app draws the confirmation last. Esc closes only the topmost confirmation.
- Do not hand-build a confirmation box per screen.

Used in: `gui/src-ui/js/App.tsx` (new game, replacing the game record, a different move, undoing learning, switching between analysis and play), `gui/src-ui/js/Dialogs.tsx` (resetting a file location to the default), `gui/src-ui/js/GgsScreens.tsx` (resign, adjourn), `gui/src-ui/js/SettingsScreen.tsx` (deleting a server or account), `gui/src-ui/js/ggs/Login.tsx` (deleting an account).
