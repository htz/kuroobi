A button with a text label. Use it wherever pressing triggers an action, such as starting a game, a new game or resigning.

## When to use
- Any button that shows the action's name as text (toolbars, the bottom row of dialogs, inside list rows).
- Not for: small actions an icon alone covers; use `IconButton`. Switching between two or three options is `Segmented`; on/off is `Toggle`.

## Props
- `children`: the label (text).
- `variant`: `primary` (the main action; one per place) / `secondary` (default) / `ghost` (a low-key secondary action) / `danger` (actions that are hard to undo, such as resign, delete, stop).
- `size`: `chip` / `row` / `ctrl` (default) / `field`.
- `disabled`, `onClick`, `title` (tooltip; include shortcuts such as `⌘N`), `className`.
- `square`: makes the width equal to the height (for single-symbol buttons).

## Rules
- Height per size is `h-chip` / `h-row` / `h-ctrl` / `h-field`, corners `r-1` (chip, row) / `r-2` (ctrl) / `r-3` (field), text `fs-6` / `fs-7` / `fs-5` / `fs-4`.
- Colours: primary has background `accent-dim`, border `accent`, text `on-accent`, bold. secondary has background `card`, border `border`. ghost has a transparent background, border `border`, text `sub`. danger has a transparent background, border and text `bad`.
- disabled is opacity 0.4. The pressed brightness is `.k-press` (the `hover` token).
- Buttons in a dialog's bottom row use `size="field"`; toolbars use the default `ctrl`.

## Similar components
- `IconButton`: a 32px square button with no border, icon only.
- `Segmented`: lays out mutually exclusive options.

Used in: nearly every screen of the app (the toolbar in `App.tsx`, `PlayDock.tsx`, `BookScreen.tsx`, `LearnLog.tsx`, `AskSheet.tsx`, `Dialogs.tsx`, `KifuViewer.tsx`, `SettingsScreen.tsx`, `GgsScreens.tsx` and each screen under `ggs/`) and inside components (`Confirm` and `Toolbar` in `components/layout.tsx`, `MoveScrub` in `components/data.tsx`, `FormulaEditor` in `components/ggs.tsx`).
