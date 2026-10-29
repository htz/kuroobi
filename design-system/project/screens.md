# Screen spec

Per-screen spec of the KUROOBI desktop GUI (Tauri + React). As the starting point for tidying up and redesigning the UI, it describes **what the current code does**.

- Sources are cited as files relative to `gui/src-ui/` plus the function and component names in them (e.g. `App` in `js/App.tsx`). No line numbers.
- On-screen strings quote the actual English of `locales/en.yaml` in double quotes.
- Dimensions are given as token names (`tokens.css`). Places written in px instead of tokens say so explicitly.
- Component names are those exported from `js/components/` (DS components). Local functions inside a screen file are marked "(local)". The small components shared by the GGS screens (`Page`, `ListPane`, `ListHead`, `HeadRow`, `Rows`, `Line`, `Cell`, `Band`, `Blank`, `ListNote`, `TitleBit`, etc.) are screen-side components in `js/ggsui.tsx`, not DS components; their names are followed by `(ggsui)`.
- Each screen's "use cases" are referred to by number (e.g. "Play-1"). Board notes on the canvas reference these numbers. Anything the current controls do not achieve goes under "Unmet" and is removed once fixed.

---

## App frame

```
┌───────────────────────── WindowBar (--h-window 32) ─────────────────────────┐
│ Signals (--w-signals)     Screen name · subtitle                     EnvTags │
├────────┬──────────────────────────────────────────────┬─────────────────────┤
│ Nav    │ Toolbar (--h-bar 44)                          │ Dock (--w-dock 290) │
│ --w-nav│──────────────────────────────────────────────│ Play, study, book   │
│ 208    │ Screen body                                    │                     │
│ (scrolls)                                             │                     │
│ Settings ⌘,                                           │                     │
├────────┴──────────────────────────────────────────────┴─────────────────────┤
│ StatusBar (--h-status 28)  Left: what is running          Right: CPU · Memory · GGS │
└──────────────────────────────────────────────────────────────────────────────┘
  + Watchers (z 20) / Drawer (z 26, from the right) / Toasts (z 27, bottom right) / Overlay (z 30)
    / GGS menu (z 50) / Opponent dropdown (z 60)
```

Assembled by `App` in `js/App.tsx`. `AppFrame` → `WindowBar` → `Body` (`Nav` + `Main` + `Dock`) → `StatusBar` (`GlobalStatus`), and finally the overlays (game record viewer, game record loading, request sheet, then the `Confirm` dialog, and last `Toasts`). App draws the Toolbar for the local screens and Settings; each GGS screen draws its own.

### WindowBar (`WindowBar` in `js/components/layout.tsx`)
- Height `--h-window`, with `--w-signals` margins on both sides. The whole bar is a drag region and holds no clickable elements.
- Center: screen name (`--fs-6` `--text`) and subtitle (`--sub`). The screen name is the Nav item name; in Settings it is "Settings". The subtitle is "Lv{n} · Black/White/Both/No side" for Play ("10 min · White" when playing on a main time), "Analysis Lv{n}" for Study (in Study KUROOBI does not move; its strength is used for analysis), and nothing elsewhere (`screenSub` in `App`).
- Right end: `EnvTags` (local, `js/App.tsx`). Only when launched with environment-variable overrides does it show bordered tags (`.k-env-tag`) such as "Auto connect" and "Unrated".

### Nav (`Nav` in `js/components/ggs.tsx`)
- Width `--w-nav` (`.k-nav`), background `--panel`. Top to bottom: logo (height `--h-bar`) → scroll area (local items → "● GGS" heading → GGS items) → footer. The dot in the heading shows the connection state (not connected `--sub` / connecting or authenticating `--gold` / connected `--ok`, `CONN_TONE`).
- Row `NavRow`: height `--h-field`; the selected row is filled `--accent-dim` with `--on-accent` text. At the right end, a count badge (`.k-nav-n`) or a status dot (`.k-nav-dot`).
- Local items (`navLocal`): "Play", "Study", "Book" (⌘B).
- GGS items (`ggsNav`): only "Log in" while not connected or connecting. Once connected: "Play / observe", "Lobby", "Players", "Results", "Chat", "Tournaments", "Console".
- Badges (`navBadges` in `js/adapt.ts`): Lobby = number of incoming requests (red); Play / observe = number of games in progress, plus a red dot when it is the logged-in name's turn to move; Chat = unread count (red); Tournaments = a gold dot when in a tournament.
- The footer is a single `NavRow` "Settings" (⌘,).

### Main and Toolbar
- `Main` (`Main` in `js/components/layout.tsx`) stacks vertically. First comes the `Toolbar`: height `--h-bar`, controls on the left; on the right `end` (controls kept at any width), `aux` (auxiliary info, disappears when narrow), the graph toggle (Study only), and the panel toggle (screens with a Dock).
- A GGS screen's Toolbar has `TitleBit` (ggsui, bold name `--fs-3` + `--sub` subtitle) or the screen's controls on the left, and the screen's primary action in `end` on the right.
- When the controls do not fit, `useToolbarCompact()` becomes true and each screen shrinks itself (currently only the Play screen's side picker changes from Segmented to Select, `RolePicker` in `App.tsx`). Measured rather than a px threshold (the ResizeObserver inside `Toolbar`).

### Dock (`Dock` in `js/components/layout.tsx`)
- Width `--w-dock`, background `--panel`. Every screen has a single tab, so no tab strip is shown. Used by Play and Study (`PlayDock`: strength on top, game record below) and the book tab (`BookDock`).

### Drawer (`Drawer` in `js/components/layout.tsx`)
- Shows the details of the row selected in a list, laid over the screen from the right (Players, Results). Shown only while something is selected; a click outside, × or Esc closes it and clears the selection. Width `--w-drawer` (480px; on a narrow window it leaves 120px on the left, 120 hard-coded), top edge below the WindowBar, bottom edge above the StatusBar, z 26. The outside is not dimmed. The header (`--h-bar`) has who / which game on the left and the tabs and × on the right.
- A press outside closes it and the press still takes effect (pressing another row opens that row's details; pressing the Nav switches screens). There is no transparent cover.
- Clicks inside overlays (request sheet, game record viewer, confirm) and toasts are not "outside". While an overlay is shown, neither a press outside nor Esc closes the Drawer (Esc closes the overlay).

### StatusBar (`GlobalStatus` in `js/GlobalStatus.tsx`; frame is `StatusBar` in `js/components/layout.tsx`)
- Shared by all screens. Height `--h-status`, `--fs-6` `--sub`, values bold `--text`. No per-screen information.
- **Left = what is running** (comes and goes): "● Auto match" (pressing it opens the request sheet in continuous-play mode), "● Analyzing n / m", "● Thinking 12.3 s" (same for local search and GGS games), "● Learning a / b" (gold), "Nodes 58.7M" (k/M/G/T regardless of language, the same unit system as the Mnps speed), "Speed 12.3 Mnps". Items that do not fit are **hidden whole, from the end** (never shown cut off, `fit` in `StatusBar`).
- **Right = always present** (gap `--sp-4`): `Load` (local) with "CPU" and "Memory" (a thin 32px-wide bar + number, hard-coded; below 70% `--ok`, 70% and up `--gold`, 90% and up `--bad`), and `GgsConn` (local) "GGS Connected (alice · local)" / "GGS Not connected" / "GGS Connecting…" / "GGS Reconnecting…".
- Pressing the GGS item while connected opens a menu (opens upward, width `--w-popover`, z 50, `GgsConn`): heading "Switch account", saved accounts per server (✓ on the current one; list height at most 240px, hard-coded), "Log in with another account…" (disconnects and goes to the new-account form on the login screen), and "Log out" in red. Closes on an outside click and Esc.

### Overlays and toasts
- `Overlay` (`Overlay` in `js/components/layout.tsx`): full-screen scrim `--scrim`, z 30, closes on Esc and on a scrim click, with a focus trap. When stacked, only the topmost Overlay closes on Esc. The content is almost always a `Modal`: corners `--r-4`, header `--h-bar` (centered title + optional subtitle + × on the right), optional `band` (`--h-bar`, `--card`), body (`--sp-4`/`--sp-5`), optional footer `actions` (`--panel`). Width `--w-modal` (340) or `--w-modal-wide` (520).
- The confirm dialog (`Confirm`) always comes frontmost (App draws it last, including when it asks over the game record viewer or game record loading).
- `Toasts` (`Toasts` in `js/components/ggs.tsx`): bottom right, above the StatusBar. Max width `--w-modal`, `--card` + `--sh-2`, warning icon colored `bad` / `gold`. z 27: in front of the Drawer, below the scrim. Disappears after 5 seconds (`TOAST_MS` in `js/state.ts`) or on click. GGS notices appear here too (`App`).

### List rows and arrow keys (`js/main.tsx`)
- In every list, ↑↓ moves from the last clicked `button.k-row` to the adjacent row and selects it as a click would. The range is the same `.k-scroll`. The Nav is excluded (arrows after choosing a screen belong to that screen); rows with `role="menuitem"` only move focus without selecting. Inactive while an input field has focus.

### Collapsing (the collapsing section of `base.css`)

| Condition | Change |
|---|---|
| Width ≤ 1120px | Hides the Dock; only screens with a Dock (Play, Study, and the book tab when a book exists) show the panel toggle (`IconButton panel` "Open panel") at the right end of the Toolbar. The learning log's right column is not a Dock, so it is narrowed to `--w-list-narrow` instead of hidden (`.k-learn-side`). When opened, the Dock is laid over the body (`--sh-2`) and the body leaves `--w-dock` free on the right (`.k-dock-inset`). The Players and Results `Drawer` is laid over at any width |
| Width ≤ 1040px | The Nav becomes a 48px icon column (labels, logo and the "GGS" heading go; badges overlay the icon's top right). The list pane `.k-listpane` goes from `--w-list` to `--w-list-narrow` |
| Width ≤ 940px | The Toolbar's `aux` disappears (`end` stays) |
| Height ≤ 620px | Hides Study's eval graph and shows a "Graph" toggle in the Toolbar |

Switches by in-screen measurement: on Play / observe, a board under 340px drops the ratings and evals (`MatchBoard` in `js/GgsScreens.tsx`); Results lowers the graph when the window is under 700px tall (`Results` in `js/ggs/Results.tsx`).

### Screen navigation rules
- Choosing a GGS item while not connected redirects to "Log in"; choosing "Log in" while connected redirects to "Play / observe" (`reachable` in `App.tsx`).
- "Play" and "Study" share one board state and only switch modes (`setNav` in `App`).
- Leaving the Play screen for another screen (Study, Book, Settings, GGS) during a game pauses it (the clock stops too, `setNav` in `App`). Coming back and pressing "Start game" continues from the same position.
- Continuous reading of the board's evals ("Evals") runs only on screens with a board (Play, Study). It stops when moving to another screen and resumes on return (`setBoardShown` in `App`).
- Starting to watch, and a tournament's "Open board", open Play / observe with that game selected via `focusGame` (`focusGame` in `js/ggs/Lobby.tsx`). A player's "Chat" opens the conversation with that person via `openChat` (`openChat` in `js/ggs/Chat.tsx`). A watcher's name opens Players with that person's details open via `focusPlayer` (`focusPlayer` in `js/ggs/Players.tsx`).
- Shortcuts (keydown in `App`; disabled while typing and while any overlay is shown — `Overlay` ones such as confirm, game record loading, game record viewer, request sheet, server editing): ⌘, Settings / ⌘B Book / ⌘N new game / ⌘Z take back / ⌘S save game record / ⌘O load game record (the four ⌘N ⌘Z ⌘S ⌘O work only in Play and Study; ⌘N in Study also starts a new game, with a confirm. On other screens ⌘Z stays the input field's undo) / in the book tab ← back ↑ to start → to best move / in Study ← → one move (Shift for 10 moves, ⌘ for the end).

---

## Play (`play`)

Purpose: play a local game against KUROOBI.

**Use cases**
- Play-1 Play KUROOBI: choose KUROOBI's color with the side picker → in the right panel's "Strength", choose "Level" (a step) or "Time control" (minutes) and the book → "Start game" → press the board. Pressing the board without pressing "Start game" also starts (if it is your turn that move is played; if it is KUROOBI's turn KUROOBI starts moving). With side "None" on a level, "Start game" cannot be pressed (there is nothing to start). On a main time, KUROOBI decides its search from the time left, and the human gets the same main time. The clock runs only during a game (from "Start game" to "Stop game", game end or pause), and only the side that moves loses time. Running out of time shows the toast "Black ran out of time" / "White ran out of time" (`gold`) and stops the game. At game end `ScoreRow` shows the result, and if learning is ON the game is imported (progress on the StatusBar).
- Play-2 Play human vs human / watch KUROOBI vs KUROOBI: side "None" / "Both" → "Start game".
- Play-3 See evals while playing: `Toggle` "Evals" (kept separately from Study; off at first in Play).
- Play-4 Take back: "Take back" (⌘Z) returns to the position before the human's previous move (passes do not count). KUROOBI's moves and passes in between go back with it. It does not stop at a position where the opponent passes. With side "None" both players are human, so it goes back one move at a time; with side "Both" it goes back one move. When there is no human move to take back (KUROOBI only moved first), it does nothing. The main time left also returns to its value at the restored position.
- Play-5 Start over: "New game" (⌘N). A board with a game record (including moves ahead of the current one) gets a `Confirm`.
- Play-6 Play from a mid-game position: the Dock's "Paste" (⌘O) / "Load" → (if the current board has a game record, a `Confirm` to replace it) → "Start game".
- Play-7 Keep the game record: Dock "Save" (⌘S).
- Play-8 Check learning: Settings > Engine "Learning" (import ON/OFF, number of learned book positions, "View learning log" → the book's learning log tab).
- Play-9 Review: Nav "Study". The board stays as it is (to Study-2).
- Play-10 Leave and come back: moving to another screen during a game pauses it (the clock stops too). Coming back and pressing "Start game" continues from the same position.
- Play-11 Replay from an earlier move: during a game, pressing a past move in the game record table pauses at that position. Pressing the board there continues the game as a take-back to that move (KUROOBI responds). No confirm is shown.
- Play-12 Play from any position: the Dock's "Edit board" (⌘E) → board editing (section below) → "Play from here".

- **Toolbar** (`App`): "Start game" (primary) ⇄ "Stop game" (danger; "Start game" cannot be pressed at game end or while thinking), "New game" (⌘N; on a board with a game record (including moves ahead) a `Confirm` "Start a new game?", body "The current record will be cleared (study shows the same one). Save it first to keep it.", OK "New game". The game record is shared with Study, so it is cleared from both), "Take back" (⌘Z; cannot be pressed while thinking or at a position with no moves), `Divider`, side picker (`RolePicker` local: "KUROOBI" + `Segmented` "Black" "White" "Both" "None"; when narrow a `Select` 116px wide, hard-coded; title "KUROOBI plays"), `Divider`, `Toggle` "Evals".
- **Body** (`App`): `Board` in `BoardFit` (min 200px, padding `--sp-2 --sp-4`), with `ScoreRow` below (height `--h-bar`: disc counts (black and white in the same frame, min 64px each; only the side to move gets an `--accent` border and a 16% fill. The frames are always there, so the counts do not move when the turn passes), at game end "Black wins" "White wins" "Draw", and a clock at the right end. When playing on a level: the game's cumulative thinking time. When playing on a main time: nothing in Play (the time left is in the right panel's "Strength"), both sides' time left in Study).
- **Dock** `PlayDock` (`js/PlayDock.tsx`). A single sheet without tabs, strength on top and game record below:
  - Strength (`Section` "Strength", with a `border` line below): a full-width `Segmented` "Level" "Time control" → for a level, `Strength` (preset `Select` "Lv1 — depth 1 / solve 2" … "Lv13 — depth 24 / solve 26" and "Custom…" with two Selects "Depth" and "Solve"); for a main time, `Select` "1 min" "3 min" "5 min" "10 min" "15 min" "20 min" "30 min" (the same choices as GGS requests, `clockChoices`). The last choice is "Custom…", which only when chosen shows a `ClockField` below to set it in seconds (below) → "Book" `Toggle` (one line: words on the left, switch on the right. Cannot be pressed without a book file, with a `Note` "— No file (you can choose one in settings)" below). Once the game record has even one move (including while viewing the position before the first move), the `Segmented` and the main time `Select` cannot be pressed until "New game" (the level step can be changed during a game). On a main time with moves in the record, `KeyValue big` "Black · You 7:48" "White · KUROOBI 8:31" (time left) replaces the `Select`.
  - Game record: `KifuTable` (columns "#" "Move" "Eval" "Time" "Source"; source is "Book" gold / "Learned" gold / "Solve" / "Search". The time column appears only for records that have times) + a bottom bar "Paste" (⌘O) "Load" "Edit board" (⌘E; cannot be pressed during a game, analysis or thinking) "Save" (⌘S; cannot be pressed with no moves). Clicking a row goes to that move (during a game it pauses, Play-11).
- At startup the saved main time is in effect as is (`useGame`). The main time left is remembered per move of the game record, and going back via take-back or a game record table click returns it to that point. Moves from a loaded record, and moves played without a main time, count as "both sides at full main time" (`Clocks` in `gui/src/main.rs`).
- **States**: before the game / during the game (Board is `disabled` only while thinking) / paused ("Stop game", moving to another screen, a game record table click, out of time) / game over ("Start game" disabled, result in `ScoreRow`). Legal move marks take the color of the side to move (`turn` of `Board`): with Black to move, a translucent black dot (`--mark-b-dot`); with White to move, a translucent white dot (`--mark-w-dot`). Themes do not change them. With evals ON, legal moves get an `EvalCell` (value and source; the best in gold). The value is always from the side to move's view (+ favors the side to move), and the circle's fill is the color of the side to move too (Black to move: black 50% with light text; White to move: white 50% with a white border and dark text. The board's green shows through, so it is distinguishable from a disc). No evals are read while it is KUROOBI's turn. During a GGS game no evals are shown at all (no error toast either). A local game cannot start during a GGS game (toast "A local game cannot start during a GGS game").
- Board orientation follows the "Orientation" setting and the side's color (`flipped` in `App`).

### ClockField (field that sets a main time in seconds)

- A single "00:00" field with two segments, minutes and seconds. Height `--h-field`, `--ff-mono` `--fs-4`; the segment being typed gets an `--accent` underline. Initially it holds the value of the choice selected just before "Custom…" (e.g. 05:00). The first time a main time is chosen for a local game it is 10 minutes.
- Entering by Tab selects the whole value; typed digits go in from the minutes and replace the whole value. Both segments are right-aligned two digits, reformatted on every keystroke (1 → 01:00, 15 → 15:00).
- Clicking the minutes or seconds segment selects that segment (fill `--accent` 30%). A selected segment is overwritten by the next digit, and further digits append up to two (no segment exceeds two digits).
- The seconds segment is selected when the minutes reach two digits or on `:` (or →) (3 : 3 0 → 03:30). ← selects the minutes segment. A segment reached by an arrow is also overwritten by the next digit.
- Backspace deletes from the last typed digit; when the seconds are empty, the next Backspace goes back to the minutes.
- Only digits, `:`, ←→, Backspace, Delete, Tab and Enter are accepted. A digit that would make the seconds 60 or more is rejected. The range is set by the caller (main time is 00:01–99:59 for both local games and GGS requests, because a local 0 means playing on "Level"; extension is 00:00–99:59), and out-of-range values are clamped on Enter and on leaving the field.

## Study (`study`)

Purpose: load a game record and look for bad moves on the eval graph.

**Use cases**
- Study-1 Open a game record: Dock "Paste" (⌘O) / "Load" (same as Play) / "Open in study" on another screen (Book-3, Learn-2, Viewer-3) / move over from Play via Nav (Play-9). Wherever it is loaded from, if the current board has a game record (including moves ahead), a `Confirm` "Replace the record?" (body "The current record ({n} moves) will be cleared (the game uses the same one). Save it first to keep it.", OK "Replace") is shown (`replaceOk` in `App`; "Open in study", Dock "Load", and `PasteKifu`'s "Load" and "From a file…").
- Study-2 Step through move by move: `MoveScrub`, ← →, the rows of the game record in the Dock, clicks on `EvalGraph`.
- Study-3 Analyze the whole game to find bad moves: in the right panel's "Analysis settings", the analysis strength and book (kept separately from Play's "Strength"; initially Lv7 with book) → "Analyze" → `EvalGraph` and the red lines for losing moves. "Stop analysis" stops it.
- Study-4 See the best move of a position: `EvalCell` on the board. Toolbar's "Evals" toggles it. On/off is kept separately from Play; Study starts with it on.
- Study-5 Try another move: press the board. At an intermediate position, the same move as the game record advances one move without confirmation. A different move shows a `Confirm` "Play a different move?" (body "The record from move {n} on ({m} moves) and its analysis will be cleared. Save it first to keep it.", OK "Play"); playing it discards the rest of the game record and the analysis. ⌘Z goes back one move at a time.
- Study-6 Keep the game record: Dock "Save" (⌘S).
- Study-7 Examine any position: Dock "Edit board" (⌘E) → Board editing (section below) → "Open in study".

- **Toolbar** (`App`): "Analyze" (primary; disabled with no game record or during analysis), `Toggle` "Evals" (the board's evals are read at the strength of the analysis settings; initially on), `Divider`, "{n} / {total} moves" or "No game record". At height ≤620px, a "Graph" toggle.
- **Body**: the same `Board` + `ScoreRow` as Play (meta changes to "Eval of this move +3.2 ▼1.5 f5 · Black Book", `curMoveMeta` in `App`), below it `MoveScrub` (|◀ ◀ ▶ ▶| + move slider + red lines for losing moves), and at the bottom `EvalGraph` (`--panel`, fixed to values from Black's view (up favors Black). The "Eval" column of the game record is also from Black's view. Legend "Book" "Solve" "Search"; hover shows the move and value, click moves there; during analysis "Analyzing n/m" + a `Progress` 72px wide, hard-coded + "Stop analysis" (danger)).
- **Dock**: same shape as Play with `PlayDock`. On top `Section` "Analysis settings" (`Strength` and a book `Toggle`; no "Level / Main time" switch), the game record below. The bottom edge of the game record has the same "Paste" "Load" "Edit board" "Save" as Play.
- **States**: no game record (`EvalGraph` shows "Run analysis to see evals"; a background is laid under the words so the 0 line does not cross them), analyzing, analyzed (losing-move marks). No analysis during a GGS game (toast "Analysis is held back during a GGS game"). Switching between analysis and a game shows a `Confirm` "Stop the analysis and start a game" (OK "Start a game") / "Stop the game and analyze" (OK "Analyze") (`useStartGame` and `useGraph` in `js/engine.ts`).

## Board editing (inside Play and Study, `js/BoardEdit.tsx`)

Purpose: build any position, such as an Othello puzzle, by clicking the board, and start a game or study from it.

**Use cases**
- Edit-1 Enter: Play's or Study's Dock "Edit board" (⌘E). Starts from the current board (position and side to move). The game record is not changed until the edit is committed.
- Edit-2 Place and remove discs: choose Black / White / Remove (keys 1–3) under "Place" on the right, then press the board or drag across it while pressed. If the square where the press starts holds a disc of the chosen color, that press removes instead (pressing the same color removes it).
- Edit-3 Revert: "Undo" (⌘Z) / "Redo" (⇧⌘Z). One press (the whole drag) or one button press is one step.
- Edit-4 Change everything at once: "Start position" (4 center discs, Black to move), "Clear", "Swap colors" (flips every disc and also swaps the side to move; title "The side to move swaps too").
- Edit-5 Start: "Play from here" (⌘↩, moves to Play) / "Open in study" (moves to Study). If the current game record has moves, the replace `Confirm` (same as Study-1) comes first. If the side to move has no legal move, that side's pass becomes the first move (`load_board` in `gui/src/main.rs`).
- Edit-6 Quit: "Cancel" (Esc) / move to another screen via Nav. The board stays as before the edit.

- **Toolbar**: "Edit board" (bold), `Divider`, "Undo" "Redo" (ghost; cannot be pressed when there is nothing to revert).
- **Body**: the same `BoardFit` + `Board` as Play, with `edit` (every square can be pressed, crosshair pointer, the chosen disc overlaid on empty squares at opacity 0.35). The side to move's legal moves get the same marks as Play. No evals and no last-move mark. Below it `ScoreRow` (disc counts and side to move, meta "{n} empty").
- **Dock** `EditDock` (a single sheet without tabs, no explanatory text):
  - `Section` "Place": three-column cards (height 60px, hard-coded; a 24px disc and its name, key 1–3 at the top right). The chosen card has an `--accent` border and an `--accent-dim` fill.
  - `Section` "To move": full-width `Segmented` "Black" "White" (with `StoneDot`), below it in `--fs-6` "Moves for Black: {n}" (`--sub`) / "White has no move, so the game starts with its pass" (`--gold`) / "Neither side can move" (`--bad`; then "Open in study" and "Play from here" cannot be pressed).
  - `Section` "Board": three columns of "Start position" "Clear" "Swap colors".
  - Bottom bar (`border-weak` line): the upper row has "Play from here" (primary, ⌘↩) full width; the lower row has "Cancel" (ghost, Esc) and "Open in study" at half width each.

## Book (`book`)

Purpose: walk through the book file's contents and check the results of learning from play. Two tabs via the `Segmented` "Book" "Learning log" at the head of the Toolbar (`App`). While learning, the Toolbar's aux shows `Busy` "Importing {done} / {total} positions". When importing finishes, the open book and learning log are reloaded.

**Use cases**
- Book-1 Walk the book: tree rows / book moves on the board / rows of Dock "Next move" / → ← ↑, "Back" "Start".
- Book-2 See a position's eval and source: Dock "This position".
- Book-3 Study a line: the tree's "Open in study" → Study (with the Study-1 confirmation).
- Book-4 Specify the book when there is none: "Open settings" → Settings > Engine.
- Learn-1 See when and what was learned: list and filters on the Learning log tab.
- Learn-2 Study a move that lost a lot: "Open in study" / a row of "Updated book entries" → that move in Study (with the Study-1 confirmation).
- Learn-3 See a learned move in the book: "Show in book" → that position on the Book tab.
- Learn-4 Undo an import: "Undo" → `Confirm`.

### Book tab
- **Toolbar**: tabs, `Divider`, "Back" "Start", "Move {n}" / "Initial position".
- **Body** (`App`): on the left `BookTree` (`js/BookScreen.tsx`, width `--w-book-tree`; `TableHead` "Line" "Value" "Games" has the same widths as the rows' value 44 and games 58 (hard-coded), so it sits right above the numbers. Rows expand with ▸/▾; clicking makes that move the new root (no selection mark); only learned moves get a gold "Learned". "Open in study" at the bottom edge) + on the right `BookPane` (`Board` with evals on the book moves, best move in gold, only book moves among the legal moves can be pressed, `ScoreRow` below).
- **Dock** (one tab, so no strip) `BookDock` (`js/BookScreen.tsx`): `Section` "This position" (aside "{n} moves", the line or "Initial position", the eval in `--fs-1` + "Disc difference · {n}-ply search" / "Disc difference", "Source" + "book.txt" / "Learned from play" (gold)) and `Section` "Next move" (count; row = move, value, games. Column widths match the tree).
- **States**: no book → `EmptyState` "No book" + body "The book file could not be loaded. You can set its location in settings." + "Open settings" (primary, goes to Settings' engine section. The tree and Dock disappear too; `BookPane`, `App`). Line end → `ScoreRow` shows "The book ends at this position". While fetching, the board keeps the previous position and moves cannot be pressed.

### Learning log tab (`LearnLog` in `js/LearnLog.tsx`)
- Three columns:
  - Left (width `--w-book-tree`): filter bar (height `--h-field`, `Segmented` "All" "Lost games" + `Select` "All time" "Last 7 days" "Last 30 days" "Last 90 days") → `TableHead` "Date" "Opponent" "Discs" "Positions" (column widths 40 / 48 / 36, hard-coded) → `TableRow` → total row "{n} games in total" ("{n} games in total ({hidden} more hidden)" when the filter excludes games) + total position count + "Positions".
  - Center `BlunderPane` (local): shows the position of the biggest loss on a `Board`, with "Move {n} f5" + "▼x" in `--bad` + "Open in study" + explanatory text.
  - Right (`.k-learn-side`, width `--w-dock`; not the Dock): `Section` "This game" (`KeyValue` "Opponent" "Discs" "Import" = "{n} positions · {when}"), `Section` "Updated book entries" (aside count; row = move number, move, old value → new value ("New" for a new position); clicking a row opens that move in Study. An entry with no details shows "This is an older entry, saved without details."), "Show in book" "Undo" (danger, cannot be pressed without details → `Confirm` "Undo this game's import", OK "Undo").
- **States**: empty → `Section` "Imported games" + "Nothing yet."; empty by filter → the filter bar stays, and in place of the list "No games match ({n} outside the filter)" + "Clear filter" (resets both period and win/loss). No losing move → "No move lost much in this game.".

---

## Common shape of the GGS screens

- Dispatch is `GgsScreen` (`js/GgsScreens.tsx`). Each screen is built from `Toolbar` + `Page` (ggsui; the body does not scroll, only the lists inside it do).
- List + detail screens have a `ListPane` on the left (ggsui, `.k-listpane` width `--w-list`, `--w-list-narrow` at 1040px or less), headed by `ListHead` (ggsui).
- Tables are `HeadRow` + `Rows` + `Line` (ggsui; row height defaults to `--h-field`, selection is `--card` + a 2px `--accent` left edge), cells are `Cell` (ggsui). A row that cannot be pressed gets the same selection color when given `picked` (the logged-in name's row in a tournament). Sections are stacked with `Section` inside `Sections` (ggsui, padding `--sp-4 --sp-1 0`).
- A one-line notice at the top of the body is a `Band` (ggsui, `--card` + a 2px `--gold`/`--bad`/`--accent` left edge); when the body is empty, a centered `Blank` (ggsui).
- People are shown by login name, never as "You". The logged-in name's games list the logged-in name first; watched games list board 1's Black first (`GgsPlay`).
- Failures of operations a person asked for go from the screen to a toast (`act` → `onGgsFailure`, ggsui).

## GGS login (`ggs-login`, `Login` in `js/ggs/Login.tsx`)

Purpose: connect to GGS. The only item in the GGS section while not connected.

**Use cases**: GGS-Enter-1 Log in with a saved account / -2 Log in with a new account / -3 Remove a saved account.

- **Toolbar**: `TitleBit` "Log in to GGS".
- **No saved accounts**: a form of width `--w-modal` in the center. Heading "Log in to GGS", vertically stacked labels (`Field`): "Server" (`Select` "name · address" + "Add server…" at the end → `ServerDialog`), "Login name", "Password" (Enter connects), "Log in" (primary, `size="field"`; while connecting "Connecting…" and disabled), status line ("Connecting to {server}…" `--sub` / error `--bad`), note "Accounts that log in successfully are saved on this machine and can be picked next time.".
- **With saved accounts**: on the left `ListPane` (heading "Saved accounts"; a `ListHead` per server (name + monospaced address); row = login name + last date "10/25"; a full-width row "+ Log in with another account" at the bottom edge). If nothing is selected, the last used account is selected. On the right, the selected account (login name in `--fs-1`, server name + address + `Tag` "Public" for the public server, "last …", "Log in", status line, ghost "Remove this account" → `Confirm` "Remove alice?" (danger, OK "Remove")), or for a new one the form headed "Log in with another account".
- The StatusBar menu's "Log in with another account…" and Settings' "+ Add an account on this server" arrive with the new-account form already open via `openNewLogin`.

## GGS play / observe (`ggs-play`, `GgsPlay` in `js/GgsScreens.tsx`)

Purpose: see the logged-in name's games and watched games on a board. Playing and watching differ only in the operations and in the search/analysis display.

**Use cases**: GGS-Board-1 Follow a game's progress / -2 Adjourn or resign / -3 Watch or stop watching / -4 See the game record of a finished game (→ viewer) / -5 With no game, ask for one, resume one, or go to the lobby / -6 See who is watching (→ Players).

- **Toolbar**: `TitleBit` "alice vs bob" + supplement "Tournament game · Synchro, random 14 · rated game". On the right `end`: for a game in progress (whether the logged-in name's game or a watched one), the "Watching this game" button; for the logged-in name's game in progress, "Adjourn" (not shown for tournament games) and "Resign" (danger); for a watched game in progress, "Stop observing"; when the game is over, "Record" → game record viewer. "Adjourn" and "Resign" use a `Confirm` ("Adjourn?" OK "Adjourn", with three body variants depending on whether it is a rated game; "Resign?" OK "Resign", danger).
- **Watching this game** `Watchers` (local): a ghost `Button` with `Icon watch` + "Watching" + the head count (bold), title "Watching this game". Cannot be pressed at 0 people. Pressing it opens a popover below the button (width `--w-popover`; near the window's right edge, its right edge aligns with the button; z 20): heading `Caption` "Watching this game" + "{n}" on the right, name rows (`ListRow`, title "Open the player's details", up to 7 rows, then scrolls). Choosing a name moves to the Players screen and opens that person's details (`Drawer`). Closes on an outside click and Esc, and when the number of watchers drops to 0.
- **List** (left `ListPane`): at the top a full-width `Segmented` "All" "In progress" "Over", `ListHead` "In progress" "Over" (aside "{n} games"), rows `MatchRow`: line 1 has `Tag` "Observing" when watched, or for a finished game `Tag` "Adjourned" (red) / "Aborted" / "Finished", + "A vs B"; line 2 has the format and result ("regu wins +2" / "alice left"). Finished games have an × "Remove from the list" at the top right, and "Close {n} finished games" at the bottom edge. A synchro match is one row for its 2 boards; newest first.
- **Game-over strip** (height `--h-field`, `--panel`): `Tag` + the outcome in bold ("regu wins +2" / "Draw" / "Adjourned: alice left" / "Aborted") + a supplement in `--sub` (for a finished synchro match "From alice: Board 1 +4 · Board 2 −2", for an adjourned rated game "Resume it from the lobby", the format).
- **Boards and graph** `BoardsAndGraph` (local): the boards (one for a single game; for synchro, two split left and right by a 1px line) and the eval graph share the height. A board is the largest square that fits (min 120px); the graph is normally 132px, shrinks down to 64px when the board would drop below 300px, and when the board is capped by width it takes the surplus up to 260px (constants such as `TREND_H` in `GgsScreens.tsx`).
- **Board column** `MatchBoard` (local): on top a row of height `--h-head` ("Board 1" for synchro, "32 – 30 · {n} moves" on the right) → upper `PlayerRow` (disc, name, rating, last eval "Eval +2.0 · 12s", clock pill. For the side to move, the frame around disc, name and rating has an `--accent` border and 16% fill (the same frame as `ScoreRow`; the side not to move also has a transparent frame so the name does not shift), and the clock's border is `--accent` too) → `Board` (always `disabled`; while searching, the best candidates are shown with `EvalCell`) → lower `PlayerRow` → status line (`--h-row`): "alice to move · Thinking · depth 20" / "Analysing" / "Solving" / "Analysis +1.5 (f5)", at game end "Over · alice +4". `--accent` while searching. The top is the opponent (when watching, the first player); the bottom is the logged-in name (when watching, the second player).
- The graph is `EvalTrend` "Evaluations (from alice's side)", legend with the two names, bars for thinking time; for synchro, the sum of both boards (matched by move number) and a gold "Split at move {n}". The hovered move's two evals and thinking times appear in a callout inside the chart.
- **Empty** (`EmptyState` side by side + `EmptyBoard`): if there are adjourned games, "{n} adjourned games" ("Resume" primary when there is one); if requests have arrived, "{n} requests for you"; if the logged-in name has a request out, "Waiting for a reply" ("Withdraw" when there is one). The body lists the matching rows, with "Resume to play on from where it stopped" / "Accept in the lobby and the game starts here" / "The game starts here once it is accepted" + "Open lobby". With none of these, "No games in progress" / "Ask for a game, or pick one to watch in the lobby" + "Ask for a game" (primary) "Find one in the lobby".

## Lobby (`ggs-lobby`, `Lobby` in `js/ggs/Lobby.tsx`)

Purpose: deal with what is addressed to the logged-in name, and find games to watch and open requests.

**Use cases**: GGS-Lobby-1 Answer a request that arrived / -2 Resume an adjourned game / -3 Withdraw a request sent / -4 Start watching (→ Play / watch) / -5 Accept an open request / -6 Ask for a game (→ Request sheet) / -7 Switch accepting requests on and off.

- **Toolbar**: "Ask for a game" (primary → Request sheet), a divider, `Toggle` "Accept requests". "Refresh" (ghost) in `end`.
- When not accepting, a `Band` "Not accepting requests" at the top edge.
- `Section` "Waiting for you" (aside "n", scrolls at most `--h-row2` × 4 rows): rows `Todo` (local, `--h-row2`): `Tag` kind + name and rating ("Random 2051.2") + conditions and supplement + operations. An adjourned game has "Adjourned" + "Resume" (primary); a request that arrived has "To you" (an `--accent` line on the left) + "just now" / "n min ago" + "Decline" (ghost) "Accept" (primary); the logged-in name's request has "Sent" + "Waiting 1:23" + "Withdraw" (ghost). Empty: "Nothing".
- Below it, split 3:2, `Section` "In progress" (aside "n games", `HeadRow` "Player" "Player" "Format", rows are name + rating, at the right end "Observe" (secondary) ⇄ "Stop observing" (ghost). Starting to watch moves to Play / watch and selects that game. Empty: "No games in progress") and `Section` "Open requests" (aside "n", other people's requests with no named opponent, rows of `--h-row2` with name, rating, conditions + "Accept". Empty: "No open requests").

## Request sheet (`AskSheet` in `js/AskSheet.tsx`)

Purpose: decide a single game request and auto match (asking repeatedly) in the same place.

**Use cases**: GGS-Ask-1 Ask for one game / -2 Start auto match / -3 Change or stop the auto match conditions.

- `Overlay` + `Modal` (width `--w-modal-wide`, `scroll`, title "Ask for a game"). A two-column grid with a label column (`--w-label-sm`) on the left. Drawn only while connected (`App`; closes when the connection drops).
- "Opponent": `OppPicker` (local). Closed, it shows the chosen name, rating, state and ▾; pressing it opens a dropdown below (above if there is no room) (z 60, height at most 6 rows): search field "Find by name" + rows (`--h-field`, name, rating, "Open" `--accent` / "Closed" / "Playing" `--ok`). The first row is "Anyone (open request)" (not shown when repeating). If the search hits nobody, "Nobody here.". ↑↓ moves, Enter decides, Esc, an outside click, scroll or resize closes. While searching, name matches come first. When repeating with no opponent, gold text "Pick an opponent to keep playing (anyone is not allowed)"; if there is an adjourned game with that opponent, a card with a gold line "You have an adjourned game with X" / "format · Resume it from the lobby".
- "Format" `Select`, "Time control" `Select` (pairs of main time and extension: "1 min · 30 s extra", "3 min · 30 s extra" (recent tournaments), "5 min · 2 min extra", "10 min · 2 min extra", "15 min · 2 min extra" (pairs common outside tournaments, and the GGS default), "Custom…". A 2-minute extension is the GGS default, so it is left out of the sent text. "Custom…" puts the "Time control" and "Extension" `ClockField`s side by side below (main time 00:01–99:59 (same as local games), extension 00:00–99:59. The two fields start from the pair chosen just before "Custom…" (03:00 and 00:30 for "3 min · 30 s extra"). If nothing was ever chosen, 5 min · 2 min extension; they wrap when the width does not fit). Increment is not offered, because KUROOBI's time allocation does not use it), "Rated" `Toggle` (for an unregistered account it is faded and cannot be pressed + "This account is not registered for rated games").
- "Games" `Segmented` "One game" "Repeat (auto match)". When repeating, "[20] games, waiting [20] seconds between games", "0 games keeps going until stopped", `Toggle` "Also accept requests that arrive meanwhile".
- The opponent's acceptance conditions: choosing an opponent fetches its finger, and its "Auto-decline condition" (decline) is evaluated by the same rules as the server (from the opponent's standpoint: the "m" in `mt1` etc. is the opponent, "o" is the logged-in name). It is re-evaluated whenever the format, time control or rated setting changes; when it hits, a card with a `--bad` line below the opponent field: heading "piglet does not take this request", below it in `--sub` "Turns a request away when it is any of:" ("Turns a request away when it is all of:" if the top of the formula is "and"; "Turns a request away when it is:" for a single condition), followed by the opponent's formula as is, as an outline. Bundles are only "Any of" (`|`) and "All of" (`&`), indented 16px each; conditions are short phrases describing a state ("not synchro", "the main time under 3 min 20 s"; the formula is not shown). `!( … )` has its negation pushed inward before listing, and `saved` is not shown. Rows that hit get ✕ (`--bad`); rows that do not get "·" (`--sub`). When opened, only the rows that hit are shown; the rows that do not are folded into "▸ n conditions not met" (`--accent`) (pressing it unfolds, "▾ Hide the conditions not met"). An "any" bundle passes when every red row is cleared; an "all" bundle passes when any one red row is cleared. Rules that keep it from breaking: indentation is 16px per level up to 4 levels (64px); deeper levels line up at the fourth. Long text wraps, with the second and later lines aligned to the right of the mark (`overflow-wrap: anywhere`). The row list is at most 240px; beyond that only the box scrolls (`k-scroll`). While it hits, the primary cannot be pressed. When the outcome depends on values unknown before sending, such as a random number (`rnd`) or the opponent's game count (`playing`), it does not block (the server decides).
- Auto match makes the same check before each request, and when it hits, stops without asking (notification "piglet does not take this request, so auto match stopped without asking: not synchro / the main time under 3 min 20 s", listing the conditions that hit). It also stops when a request slips past the check and the server replies that it "doesn't fit what the opponent accepts".
- Footer: the left is left empty (the input fields are visible, so no summary is shown; only on error, the error text in `--bad`); on the right, "Stop auto match" (danger) while auto match runs, "Cancel" (ghost), and the primary "Ask" / "Start auto match" / "Continue with these settings".
- Opened from: the lobby, the empty state of Play / watch, a player's Summary (with the opponent filled in), the results' "Rematch" (with opponent and format filled in), the StatusBar's "Auto match" and the tournament warning band (repeat mode). The last format, time control and rated setting are kept in localStorage.

## Players (`ggs-players`, `Players` in `js/ggs/Players.tsx`)

Purpose: see ratings, who is connected and the ranking, and look up opponents.

**Use cases**: GGS-People-1 See the logged-in name's rating / -2 Find an opponent (connected, top, by name) / -3 Look up an opponent (summary, game history, profile) / -4 Ask that person for a game or talk to them / -5 See the game record of a game in the history (→ viewer).

- **Toolbar**: search `TextField` "Search by name" (180px, hard-coded), `Segmented` "Online" "Top", for top a `Segmented` "Random" "Standard", "Refresh" (ghost). In `end`, paging "Previous" "n / m" "Next" + `Select` "25 per page" (title "Rows").
- `Section` "alice's rating": `RateRow` in the order "Random opening" "Standard" (`RankRates` (ggsui). Label of `--w-gtype`, rating in `--fs-1`, ± deviation + "Provisional" (gold), "#n", "nW nL nD"). Empty: "No records yet.".
- `Section` "Online" / "Top players (Random)" (aside "n players"): the online columns are "Name" "Random" "Standard" "Open" ("Accepting" `--accent` / "Closed" / "Disconnected" `--bad`) "Status" ("Playing" `--ok` / "Online" `--accent` / "Offline"). The top columns are "Rank" (#1 in gold) "Name" "Rating" "Status". The logged-in name's row is bold. Empty is `Blank` "No players match "q"" + "Players who are offline can be found in the ranking." / "Nobody is connected." / "The ranking has not arrived yet.".
- **Detail** (`Drawer`, `PlayerDetail` (local)): shown only when a row is pressed (with nothing selected nothing is shown and the list takes the full width). The heading is the name (`--fs-3`), beside it, when playing, a `Tag` "Playing" and the acceptance state ("Accepting" `--accent` / "Closed" / "Disconnected" `--bad`, `--fs-6`), on the right a tab `Segmented` "Summary" "Game history" "Profile" + ×.
  - Summary: the "Random" and "Standard" ratings in two columns; for someone else, "Ask for a game" (primary) and "Chat" (both as wide as their text); "Rating trend" + `Segmented` + `RateChart` (448 × 160px, hard-coded; hovering shows a callout with date and time, opponent, disc difference and rating). Fewer than 2 points: "No history yet"; before fetching: "Loading…".
  - Game history: `Segmented` "Random" "Standard" + "n games", the same table as the Results screen (`ResultHead note` + `ResultRow`, columns "W/L" "Opponent" "Format" "Discs" "Date"; win `--ok` / loss `--bad` / draw and adjourned `--sub`). Row → game record viewer (fetched from the server). Empty: "No games played yet."; before fetching: "Loading…".
  - Profile: the finger items under small headings such as "Identity" and "Settings", with a label column `--w-label-sm` + value. Request conditions are a `FormulaView` ("All of" / "Any of" bundles and condition chips); unset is "Not set". Before fetching: "Loading…".

## Results (`ggs-results`, `Results` in `js/ggs/Results.tsx`)

Purpose: see the logged-in name's finished games and rating history.

**Use cases**: GGS-Results-1 See the record and rating history / -2 Open the game record of a finished game (→ viewer) / -3 Rematch on the same conditions (→ Request sheet).

- **Toolbar**: `Segmented` "Random" "Standard" "All", a divider, `Select` "All time" "Last 7 days" "Last 30 days" "Last 90 days" (128px, hard-coded).
- With a format chosen, `Section` "Rating history": `RateChart` (measured width, height 168px, 96px when the window is short) and a band of 6 `KeyValue`s "Rating" "Change" "W / L / D" "Win rate" "Avg diff" "Best rating" (wraps when narrow). With "All", instead a `Section` "Ratings" with 2 rows of `RateRow`.
- `Section` "Finished games" (aside "n games", the only part that scrolls): `ResultHead boards` + `ResultRow` (`js/components/data.tsx`, height `--h-field`, columns "W/L" (win `--ok` / loss `--bad` / draw and adjourned `--sub`) "Opponent" "Format" "Discs" (GGS's value; for synchro, the mean of the 2 boards. The result band's "Avg diff" also counts this value) "Decided by" (width 72, only how the whole match was decided: "Opp. resigned" `--gold` / "Resigned" `--bad` / "Adjourned" `--sub`. On GGS, resign and adjourn apply to the whole match, for synchro to both boards) "Board 1" "Board 2" (width 170, `ResultBoardCell`: that board's disc difference (the result GGS gave that board: out of time is ±64, over time is the corrected value, a board played to the end is the disc difference with the empties added to the winner. The mean of the 2 boards is the match's disc difference), and a time tag "Won on time" "Won on over time" `--gold` / "Lost on time" "Lost on over time" `--bad`. Empty for a match with a decided-by. A single-board match has only Board 1) "Rating" (only when there is a value) "Date"). Hover is linked to the graph's points; a click shows the detail in a `Drawer`. Rows with no game record stay translucent but can be pressed. Matches that exist only in the server's history are mixed in too (board columns empty).
  - Out of time = the extension is used up too (`fatal-timeout`, that board is ±64). Over time = the main time was used up first and the extension started (`timeout`; whoever entered it first cannot win even if the extension ends the board). The server reports a resign only as a ±64 result, so a board that ended at ±64 at a position with legal moves and without running out of time is taken as a resign (`match_ending` in `gui/src/ggs.rs`).
- Empty: only when there are no records at all, `Blank` "No records yet." / "Your results build up here as you play on GGS.". If a period filter leaves 0, `Blank` "No games in this period".
- **Detail** (`Drawer`): shown only when a row is pressed (with nothing selected nothing is shown and the graph and list take the full width). The heading is only the opponent's name and ×. At the very top of the body, a result card (`ResultCard`: a 3px left band and a 12% fill in the outcome's color (win `--ok` / loss `--bad` / draw and adjourned `--sub`). Line 1 has "Won" "Lost" "Draw" "Adjourned" and the match's disc difference in bold `--fs-1`, followed by "Disc diff" in `--sub`, and for resign or adjourn a decided-by tag at the right end. With 2 boards, line 2 has "Board 1 +n tag" "Board 2 +n tag"). Then `KeyValue` "Opponent rating" "Format" "Time control" "Your colour" (for synchro "Board 1 White · Board 2 Black") "When" "Rating" (change from the previous game), a per-board clock table (`BoardTable`: headings "Board" "Player" "Time left". One row per player per board, with disc color and name, and the clock at game end (written the same way as the in-game clock: if the extension was not used, the main time left "0:08"; if in the extension, the extension left "Ext 1:48"; if out of time, "Ext 0:00". Once in the extension, the first field of GGS's clock becomes the extension left, so the side that went over time is recorded as main time 0 and extension at that value)), `EvalTrend` if the game record has evals (height 180px), "Open kifu" (primary; cannot be pressed for a match with no game record) "Rematch" (both as wide as their text). For old matches with no per-board record, the disc difference is filled in from the result field of the board's game record (or the final position if none), and over time, out of time, resign, adjourn and the clocks at game end are filled in if the wire log (`ggs_wire.log` and its previous generations) still has them (`wire_endings` and `backfill_from_wire` in `gui/src/ggs.rs`).

## Chat (`ggs-chat`, `Chat` in `js/ggs/Chat.tsx`)

Purpose: the public chat and private conversations.

**Use cases**: GGS-Talk-1 Talk to everyone / -2 Talk to one person (start a new conversation) / -3 Read translated, send translated.

- **Toolbar**: conversation name (bold `--fs-3`) + `Tag` "Goes to everyone here" / "Goes to this person only".
- Left `ListPane`: heading row (`--h-field`) "Conversations" + "New person" (`size="row"`) → a small popover above the list (width `--w-popover`, list height at most 320px, hard-coded: search field "Search online players" (Enter picks the first person) + name rows, empty "Nobody here."). Row (`--h-row2`) = the other person's name (the first is always "Public chat"; unread in bold) + time (today: the time; the day before: "Yesterday"; earlier: the date) + the last message + unread count `Badge`. Read state per conversation is in localStorage.
- Right: the body (`DayMark` date separators + `Bubble`; the name is omitted when the same speaker continues; the logged-in name's messages are right-aligned in `--accent-dim`. The other person's messages get a text button "Translate" below the bubble; only the pressed message is translated into the app's display language (Japanese / English) and shown in a lower section inside the bubble. While translating, "Translating…" (cannot be pressed); once shown, "Hide translation" folds it. If translation fails, "Could not translate" in `--bad` in the lower section; pressing "Translate" again retries. Nothing is translated automatically), input row `TextField` "Type a message (Enter to send)" + "Send" (primary). Text that could not be sent goes back into the input field, and the reason appears in a toast.
- Empty: `Blank` "No messages yet." / "No messages with bob yet" + "Send the first message from the box below.". The Nav unread count clears when the screen is opened.

## Tournaments (`ggs-tourney`, `Tourney` in `js/ggs/Tourney.tsx`)

Purpose: join tournaments and see their progress, standings and games.

**Use cases**: GGS-Tourney-1 Find a tournament / -2 Join or withdraw / -3 See the progress, crosstable and games per round / -4 Open the board of the logged-in name's game (→ Play / watch) / -5 Check or change the auto match while joined.

- **Toolbar**: `TitleBit` "Tournament 12" + state `Tag` ("Not started" before the start) + `Tag` "Joined" (`ok`) when joined. In `end`, "Open board" (primary) when the logged-in name's game is in progress, "Join" (primary) / "Withdraw" before the start, "Refresh" (ghost). With no tournament, `TitleBit` "Tournaments".
- Left `ListPane`: `ListHead` "Live and open" and "Finished tournaments". Row (`--h-row2`) = "Tournament 12" (a gold dot when joined) + "in 3:20 · 12 players" / "round 3/11 · 12 players" / "winner X · 12 players", "Join" if not started and not joined, state `Tag` ("Playing" `accent` / "Break" "Not started" `ok` / "Finished" "Cancelled" `sub`). Empty: "No tournament is scheduled or running.". With nothing, the right side is `Blank` "Pick a tournament to see it here" / "Tournaments appear on the left; results and games show here".
- **Detail** `TourneyDetail` (local):
  - Band at the top edge: a failed operation is a `Band tone="bad"` (put into Japanese by `tdErrors`, shown for 60 seconds); when joined and auto match is ON, a `Band` "Auto match is on. Other games will still be accepted and played alongside the tournament." + "Change auto play".
  - Overview (separated by an underline): "Format" "Time control" "Pairing" "Breaks" "Entrants" "Director" flow on one line. In progress: "round 3 / 11" + `Progress` + "next round in m:ss" or "Finished this round a / b" + "· alice's next game Round n". Before the start and after the end, bold "in m:ss" / "Open" / "Cancelled" / "winner X" / "Finished" + supplement.
  - Before the start: "Entrants" + "n players" + `Tag` "Open" / "closed", table "#" "Player" "Random".
  - After the start: a single `Segmented` "Crosstable" "All games", showing one sheet at a time in the same place (the crosstable opens first).
    - Crosstable: on the right of the bar, "alice: 3 of 8". The table's rows and columns are both in rank order; on the left "Rank" "Player" "Points" "W-D-L" "Avg discs", then column headings with the opponents' names (the logged-in name in `--accent`). A cell (width 50px, row `--h-row2`) has two lines, the points and W-D-L against that opponent from the row person's view (the same opponent can be met many times). A winning record is filled with `--ok`, a losing one with `--bad`, via `color-mix` up to 32% according to the size of the margin. A pair playing in the current round gets a 1px `--accent` border; a person's cell against themselves is hatched. Pressing a cell moves to All games, filtered to those two people. The heading row sticks to the top and the left 5 columns stick to the left (with a `--border` line at the right edge); only the opponent cells scroll both ways. The logged-in name's row has `--card` and a 2px `--accent` left edge in the left 5 columns.
    - All games: on the right of the bar, "Players" `Select` "All" + "−" + `Select` "All" (both list only entrants; a person chosen in one is removed from the other; the two fields are symmetric), and "Clear" (ghost) only while either is chosen. With both "All", round paging (‹ "Round" [n] "/ m" › "Current round"; ← → keys page too). With a choice, a band in the same place shows "ymatioun 70 games · 36 finished" / "alice − ymatioun 10 games · 6 finished", and the table gains "R" and lists games from all rounds. In that case the first field's person (the second's if the first is "All") is always placed on the left, and the result is colored `--ok` / `--bad` by the disc difference from that person's view. The logged-in name's games are seen in this list by choosing the logged-in name as a player (the "Open board" for a game in progress is in the Toolbar). Table "Players" "Players" "Diff" (GGS's value; for synchro, the mean of the 2 boards).
    - In every table, the logged-in name's rows (for the logged-in name's games, the rows in progress) get the selection color even when they cannot be pressed (`picked` of `Line`). Not yet received: "Loading…"; no games in that round: "No games in this round".

## Console (`ggs-console`, `Console` in `js/ggs/Console.tsx`)

Purpose: see and send raw traffic with GGS.

**Use cases**: GGS-Comm-1 See, search and keep the traffic / -2 Send a command.

- **Toolbar**: `Segmented` "All" "Sent" "Received", search `TextField` "Search the log" (220px, hard-coded), a divider, `Toggle` "Follow latest". In `end`, a failure message (`FlashNote`, 2.5 s, `--bad`: "Could not copy" / "Could not save (…)"), "Copy" "Save" (normal), "Clear" (ghost; only removes it from the view).
- Body `ConsoleLog` (monospaced; sent lines are `--accent` + "›", app-originated lines are `--sub`; follows only while at the end). Empty: `Blank` "Nothing received yet".
- Input row (`--panel`): an `--accent` "›" + monospaced `TextField` "Command (e.g. tell /os who 8; Enter to send)" + "Common commands ▴" (a small popup above, width `--w-popover`, height at most 280px hard-coded: 12 commands with descriptions; pressing one puts it in the input field) + "Send" (primary).

---

## Settings (`settings`, `SettingsScreen` in `js/SettingsScreen.tsx`)

Purpose: engine, display and GGS settings. A standalone screen (not a modal) opened from the Nav's "Settings", ⌘, or the book's "Open settings".

**Use cases**: Settings-1 Specify the weights and book files / -2 Decide the search resources / -3 Change the look / -4 Manage servers and accounts / -5 Decide how GGS games and watching behave / -6 Decide the conditions for automatically accepting and declining / -7 Decide learning from finished games (import ON/OFF, number of learned book positions, go to the learning log).

- **Toolbar** (`App`): the category name ("Engine", "Display", "GGS · Servers and accounts", etc.) and, for 2.5 s after a change, `--ok` "✓ Applied" at the right end (`js/applied.ts`). There is no save button; changes apply immediately.
- Categories in the left `.k-listpane`: "App" ("Engine", "Display"), "GGS" ("Servers and accounts", "Playing and watching", "Request rules"). The selection is `--card` + a 2px `--accent` line on the left.
- The right side is a vertical stack at most `--w-settings` wide. Sections are `SettingGroup` (`js/Dialogs.tsx`, = `Section`), rows are `SettingRow` (`js/Dialogs.tsx`: label column `--w-label` + control + a one-line supplement or a `--bad` error).
- **Engine** (`EngineSettings`, `js/Dialogs.tsx`):
  - "Files": for each of "NNUE weights", "Linear eval" and "Book", a read-only monospaced `TextField` ("Not set" when unset) + "Choose…", with the supplement "Loaded · 12.3 MB" or an error ("Not found. Playing on the linear eval alone" / "Not found. KUROOBI cannot evaluate" / "Not found. Using only book_learn.txt, which grows from played games"). "File locations" + "Reset to defaults" (supplement "Forget the chosen locations and search the default places. Files are kept") → `Confirm` "Reset the file locations to defaults?" (OK "Reset"). Only the file locations are reset; threads are not.
  - "Search": "Threads" (`Select`, "n (auto)", supplement "The parallelism used by local games, study, learning and GGS games."), "Hash table (midgame)" "Hash table (endgame)" ("… (default)", supplement "Applies from the next launch").
  - "Learning": `Toggle` "Import finished games" + supplement "Local and GGS games are imported win or lose, and the final disc difference is written back to the root.". ON/OFF is saved (it does not revert on launch), and switching it shows "✓ Applied". "Learned book" = "Stored positions" "Of which learned" + "View learning log". The counts are reloaded when an import finishes.
  - The main time for local games is decided in the Play screen's right panel.
- **Display** (`ViewSettings`, `js/Dialogs.tsx`): "Screen" ("Theme" as three cards with previews "System" "Dark" "Light", "Language", "Decimals" 0/1/2), "Board" ("Coordinates" "Grain" `Toggle`, "Disc flip" "None" "Fast" "Slow", "Orientation" "Black at bottom" "White at bottom" "You at bottom" + supplement "Applies to GGS games too", "Mat color" color swatches of `--h-ctrl` squares).
- **Servers and accounts** (`GgsAccountSettings` (local)): "Logged in" ("Account" = name, server, address + "Log out" (danger) + supplement "After logging out, the next start does not log in"; when not connected, "Not logged in". "Log in at start" `Select` "Don't" / each account + supplement "Set to the last account that logged in"). "Servers and accounts" (aside "n servers · m accounts"): under each server heading row (`--h-row2`, `--card`: name, address, `Tag` "Public" / "Test", "Edit" "Remove") come the account rows (login name + `Tag` "Connected" "At start" + "last …" (width 120px, hard-coded) + "Remove") and "+ Add an account on this server"; last, "Add a server" and the note "An account is saved when it logs in. Passwords stay on this Mac". Every removal uses a `Confirm` (danger, OK "Remove").
- `ServerDialog` (`js/SettingsScreen.tsx`, `Modal` width `--w-modal`): "Add a server" / "Edit server", "Name", "Address" (monospaced, "host:port"), "Kind" `Segmented` "Public" "Test" + supplement "Games on a test server are not learned into the book"; the footer has the error and "Cancel" (ghost) "Save" (primary).
- **Playing and watching** (`GgsPlaySettings` (local); when not connected, "Log in to GGS to change these"): "Book" `Toggle` (supplement "Use book moves in GGS games". If there is no file, "Choose a book file" and the supplement "No file"), `Toggle` "Play automatically on my turn", `Toggle` "Analyze observed games too", "Search log channel" (monospaced, 120px hard-coded, supplement "Posts each move's search result to this channel, where spectators who join it can read it. Left empty, nothing is posted.", validation error "Enter "." followed by 1-7 letters, digits, _ or -. This value is not used."). Changes are sent together 400ms after the last change.
- **Request rules** (`GgsFormSettings` (local), aside "Kept by the server even when the app is closed"; when not connected, "Log in to GGS to change these"): `FormulaField` (`js/GgsScreens.tsx`) ×2, "Auto-accept condition" and "Auto-decline condition". When unset, the box shows "Not set" + the explanation "Requests are not handled automatically; you decide when one arrives." + "Add a condition" "Write the formula directly"; when set, a `FormulaEditor` (bundle `Select` "All of" / "Any of" / "Not all of" / "None of", condition rows, "+ Condition" "+ Bundle", "Formula sent", "Remove the conditions" "Write the formula directly" "Apply"). A condition row can build every element of a GGS formula: comparisons between items ("my clock < the opponent's clock"), functions ("my rating change at [+1.0] ≤ [0.0]", "random number below [100] ≥ [50]", "my live games [in total] > [0]"), and "Write as a formula…" at the end of the item list gives a formula field. A loaded formula binds `&` and `|` at equal strength from the right, as GGS does; when written out, nested bundles are always parenthesized. The direct-writing mode is a `TextArea` + "Back to the tree" "Apply". Only here nothing is sent until "Apply" is pressed; while unsent, gold "Not applied yet". While the tree has a red field, "Apply" cannot be pressed and "Fix the fields in red to apply" (`--bad`) is shown. "Applied" appears only once the sent formula shows up in the server's finger. If the server rejects the formula (`dform ERR …`), "The server refused these conditions (reason); the previous ones stay" appears in `--bad` under the heading, and the content being edited is kept. Only under "Auto-decline condition" is there "Use the defaults" (secondary) and the explanation (`--fs-6` `--sub`) "The tournament director's guidance (decline under 14 discs or under 1 minute of main time), plus declining games KUROOBI cannot play or would play at a disadvantage.". Pressing it only puts the default formula in the field; sending is "Apply". It cannot be pressed if the formula is already the same. The default formula is `DEFAULT_DECLINE` (`js/ggs.ts`).

## Dialogs

| Name | Location | Contents | Opened from |
|---|---|---|---|
| `Confirm` (DS component) | `Confirm` in `js/components/layout.tsx` | A `Modal` (width `--w-modal`) with title and body, a close button at the top right, and a right-aligned footer "Cancel" (normal `Button`) + OK (primary or danger). Always comes frontmost | New game (board with a game record), replacing a game record (loading onto a board with a game record), a different move partway through in Study, undoing a learning import, adjourn and resign, resetting the file locations to defaults, removing an account or server (Settings and the login screen), switching between analysis and a game (via `js/engine.ts`) |
| `PasteKifu` (game record loading) | `PasteKifu` in `js/Dialogs.tsx` | Width `--w-modal-wide`, supplement "A move list (f5d6…), a board, a board plus moves, GGF or Othello Quest: the form is worked out for you." (the format is not chosen by the user: GGF is `(;`, Othello Quest is JSON `moves`, a board is 65 characters after removing spaces and newlines: 64 squares + side to move (empty `-` `.`, black `X` `x` `B` `b` `0` `*`, white `O` `o` `W` `w` `1`, the same characters for the side to move); from the rest only a1–h8 moves are picked up. Numbers such as "1." and stray characters are ignored), a monospaced text field (5 lines), a "Preview" area (a board 96px hard-coded + disc counts "Black — White" + "n moves · Final position" / "n moves · Last position"; for a board alone "Board only · Black to move" / "Board only · White to move". Before pasting, the board's place is shown by a dotted `border` frame with "Paste a record and the result appears here."; when unreadable (neither a board nor moves found), a `--bad` error text), footer left "From a file…", right "Cancel" (normal) "Load" (primary, disabled until readable). For both "Load" and "From a file…", if the current board has a game record, the replace `Confirm` comes first | Play and Study Dock "Paste", ⌘O |
| `ServerDialog` | `ServerDialog` in `js/SettingsScreen.tsx` | Above | Settings, the login screen's "Add server…" |
| Request sheet `AskSheet` | `AskSheet` in `js/AskSheet.tsx` | Above | Above |

## Game record viewer (`KifuViewer` in `js/KifuViewer.tsx`)

Purpose: replay a GGS game's record on the spot and hand it to Study.

**Use cases**: Viewer-1 Replay / -2 Copy or save / -3 Open in study (→ Study, the confirm in Study-1). This button is the only way into Study.

- `Overlay` + `Modal` (width `--w-modal-wide`, `scroll`, title "alice vs bob" / "Game with bob · 10/26 21:15" etc., supplement "format · main time · outcome". The outcome is written as on the Play screen, "regu wins +2" / "Draw"; with no result, "alice − bob"). For a synchro game, `Segmented` "Board 1" "Board 2" in the `band`.
- Body: on the left a `Board` (with coordinates, not pressable; side length 160–300px hard-coded, derived from the window height), on the right the black and white players (disc, name, rating, disc count `--fs-2`), "This move" + `--fs-1` "12. f5" / "Initial position" + "Final +4" (disc difference from Black's view), "Move list" (scrolls at most 96px, hard-coded). Below, |◀ ◀ ▶ ▶| and ▶▶/❚❚ (autoplay at 450ms intervals) and "n / m moves", `MoveScrub` (`nav=false`).
- Footer: "Close" (ghost), a temporary message (`FlashNote`, 2.5 s: "Copied" / "Could not copy" / "Could not save (…)"), "Copy game record" "Save to file" "Open in study" (primary, disabled until the game record is readable). "Open in study" closes the viewer and moves to Study only when the replace confirm is OK (or when no confirm is needed). On Cancel the viewer stays open.
- **States**: while fetching, "Fetching…"; unreadable → "Could not fetch the kifu" + error text (if there is a source to fetch from, it refetches automatically). If the fetch itself fails, the viewer closes and a toast is shown. If closed while fetching, a game record that arrives later is discarded (closing after it arrived does nothing) (`closeViewer` in `App`).

## Empty states

| Screen | Condition | Display | Component |
|---|---|---|---|
| Play / watch | No game | Four variants — adjourned game, request received, awaiting reply, nothing at all — + buttons | `EmptyState` + `EmptyBoard` |
| Book | No book file | "No book" + "Open settings" | `EmptyState` |
| Learning log | No imports / 0 after filtering | "Nothing yet." / "No games match (n outside the filter)" + "Clear filter" | `Section` + `Empty` / `Empty` inside the list |
| Game record (Dock) | No moves | "No moves yet." | `Empty` |
| Book Dock / tree | Line end / fetching | "The book ends at this position." / "Loading…" (the board keeps the previous position; moves cannot be pressed) | `Empty` |
| Eval graph | No points | "Run analysis to see evals" / "Analyzing…" | Built into `EvalGraph` |
| Rating history | Fewer than 2 points | "No history yet" | Built into `RateChart` (the player overview has its own one line) |
| Lobby sections | 0 items | "Nothing" "No games in progress" "No open requests" | `ListNote` (ggsui) |
| Players, Results, Chat, Tournaments, Console | 0 items, nothing selected, before fetching | Players: "No players match "q"" "Nobody is connected." "The ranking has not arrived yet." "No games played yet."; Results: "No records yet." (when there are none at all) "No games in this period" (0 in the period); Chat: "No messages yet." "No messages with bob yet"; Tournaments: "Pick a tournament to see it here" "No games in this round"; Console: "Nothing received yet"; before fetching: "Loading…". Players and Results show no `Drawer` when nothing is selected, so there is no empty display either | `Blank` (ggsui) |
| Tournament list, chat partners, request opponents | 0 items | "No tournament is scheduled or running."; when a search hits nobody, "Nobody here." | `ListNote` (ggsui) |

Empty states come in three shapes by size: a whole screen is `EmptyState`, one pane is `Blank`, a list inside a pane is a one-line `ListNote` (inside a local screen's `Section`, `Empty`).

---

## Places not yet componentized

The following use shared components and are not hand-written in screen files: the selected row (`ListRow` / `picked`; the mark alone is `marked`), two-line rows (`TwoLines`), small headings (`Caption` / `captionStyle`), a stacked label + input (`Field`), the label column width (`w-label` / `w-label-sm`), floating boxes (`popoverStyle` + `useDismiss`), temporary messages (`useFlash` / `FlashNote` / `FLASH_MS`), date/time formatting (`js/when.ts`), `useNow`, synchro totals (`sumBoards`), the rating table (`RankRates`), the one-line "nothing here" in a list (`ListNote`), confirmation of irreversible actions (`Confirm`), details of the selected row (`Drawer`), toolbar dividers (`Divider`).

### Two families of table rows
- Local screens use `TableHead` + `TableRow` (columns passed via `cols`); GGS screens use `HeadRow` + `Line` + `Cell` (`js/ggsui.tsx`, columns laid out as children). The heading row (`HeadRow` uses `TableHead`) and the selected look (`Line` is a `ListRow` when pressable, and even when not pressable uses the same color via `picked`) match, but how columns are passed and the default row height (`TableRow` is `h-row`, `Line` is `h-field`) differ. `ResultHead` / `ResultRow` are result-specific columns on top of `TableHead` (Results and the players' game history).

### Hard-coded px
- Column widths: the request sheet's opponent candidates 64 / 56 and the game-count and interval fields 56 / 48 (`js/AskSheet.tsx`), the lobby's waiting items 56 / 180 and the in-progress format and watch columns 128 / 64 (`js/ggs/Lobby.tsx`), the column widths of the player and tournament tables (`js/ggs/Players.tsx`, `Tourney.tsx`), `KifuTable` 22 / 58 / 56 / 34 and `ResultRow` 28 / 44 / 60 / 64 (`js/components/data.tsx`), the book's eval 44, occurrences 58, move 30, expand/collapse 22 (`js/BookScreen.tsx`), the learning log's 40 / 48 / 36 and the updated book entries' 26 / 24 (`js/LearnLog.tsx`), `FormulaEditor` fields 132 / 88 / 84 / 74 / 58 (`js/components/ggs.tsx`), the settings account row's "last" 120 and the search log channel field 120 (`js/SettingsScreen.tsx`).
- Size limits: the console's search field 220 and the command list height 280, chat's candidate list height 320, the account menu's list height 240, the players search field 180, the results period `Select` 128, the row of theme previews 420 (`js/Dialogs.tsx`), the game record paste board 96 (`js/Dialogs.tsx`), the game record viewer's board 160–300 and move list 96 (`js/KifuViewer.tsx`), the constants of `BoardsAndGraph` (`js/GgsScreens.tsx`), the players `RateChart` 448 × 160 and the results heights 168 / 96 and the detail's `EvalTrend` 180, the dimensions of `EmptyState` / `EmptyBoard` (`js/components/layout.tsx`), the 120 that `Drawer` leaves on the left, the status bar's usage bar 32×4 (`js/GlobalStatus.tsx`), the side `Select` 116 (`js/App.tsx`), the analysis `Progress` 72 (`js/App.tsx`).
- Spacing: `MatchRow`'s 9px and the close button's 40, `Tag`'s 0 5px, the Nav heading's 18px / 6px (`js/components/ggs.tsx`), `ScoreRow`'s side-to-move box minimum width 64 and height 30 (`js/components/data.tsx`), `BookRow`'s indent 16 (`js/BookScreen.tsx`), `Button`'s main button (`ctrl`) horizontal padding 14 (`js/components/primitives.tsx`; the same value as `--sp-3h`, but the token is not used).
