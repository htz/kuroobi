# Design system: the web

The design system of the site and Kuroobi for web (`web/`). A web page is
read at arm's length, on any screen from a phone up, and used with a
finger as often as a mouse; the app's design system (`design-system/gui/`)
is sized for a desktop window and is not used here. The implementation in
`web/src/` is the source of truth; this folder describes it, and the
canvas (`design/web/`) draws with it.

Every page and every change to one follows this, in this order: the
canvas, then this design system if the canvas needs something it lacks,
then the code.

## Colour

The app's colour tokens, shared as they are (`crates/gui/src-ui/tokens.css`):
grounds `bg` / `panel` / `card`, lines `border` / `border-weak`, text
`text` / `sub`, `accent` for actions and the current item, `bad` for losses
and errors, and the board, stones and board marks. Dark is the default;
the theme menu or the OS switches to light. Text keeps 4.5:1 on its ground.

## Type

Two families: `ff-web` (Noto Sans JP) for everything, `ff-code` (JetBrains
Mono) for moves, records, numbers in columns and code.

| Token | Size | For |
|---|---|---|
| `t-display` | 34–60px | A page's own title (Notes) |
| `t-h1` | 28–40px | Section headings, an article's title |
| `t-h2` | 22–28px | Headings inside a section or article |
| `t-h3` | 19px | Card titles, the play screen's title |
| `t-lead` | 16–19px | The paragraph under a title |
| `t-body` | 16px | Body text, panel headings, player names |
| `t-small` | 14px | Controls (buttons, selects, toggles), table and list rows, notes under a heading |
| `t-meta` | 13px | Captions, labels over a control, table headings, legends, axis labels, dates |

**Nothing is smaller than 13px (`t-meta`) as it appears on the screen.**
That holds for text inside graphics as much as for HTML: a board or a
graph that scales with its box keeps its text at 13px or more at the
smallest size it is drawn, growing its text back as it shrinks (or
drawing in screen pixels). Text that does not fit at 13px is left out and
said once beside the graphic instead (a board's eval circles show the
value only, and a badge at its upper right says where a value came from:
a check on `text` for a solve to the end, an open book on `accent` for the
opening book, none for a search). `EvalLegend`, right under the board
whenever it shows evals, names each mark the board draws with a sample
of it, and nothing for a search, which has no mark; the book's entry joins
once values can come from the book.

Line heights: 1.9 for body text, 1.35 for headings, 1.5 for controls and
rows. Numbers in columns use tabular figures.

## Space and size

- Page: content at most `c-max` (1200px) wide with `gutter` (16–40px) at
  the sides; prose at most `c-text` (680px). Sections are `s-section`
  (64–112px) apart.
- Inside a panel or card: 20px padding, 16–18px between its parts, 8–12px
  between the items of a row.
- **Anything pressed is at least 44px tall** (buttons, selects, toggles,
  radio cards, links styled as buttons). Small buttons inside a panel's
  heading row are 36px. Rows of a table or list that can be pressed are
  40px.
- Corners: 14px for panels and cards (`r-card`), 10px for controls, 8px
  for small buttons, a pill for tags.
- A page works at 390px wide: columns stack, toolbars wrap, wide tables
  scroll inside their box, and nothing scrolls the page sideways.

## Labels

A control's label names its object and the action on it, and nothing
else: 棋譜を読み込み, 棋譜を分析, リンクをコピー (Load a record, Analyze
the game, Copy link). 開く is for files only; a record comes in pasted, picked
or dropped, so it is 読み込み. No する, no sentence, no explanation; the object is never
dropped either. A switch says what turning it on shows (盤に評価値を表示).
Sending something elsewhere is one Share menu (`MenuButton` with the share
icon), not a button per way of sending.

A control that cannot be pressed loses its ground, its border goes to
`border-weak` and its text to `sub`, so it never reads as one that can.

## Components

Each is a React component in `web/src/components/` (the site's markup
pieces are classes in `web/src/styles/site.css`). The canvas mounts them
from the built bundle as `window.KuroobiWeb`.

| Component | What |
|---|---|
| `Icon` | A 24-grid line icon in the text's colour, 18px beside a label: share, link, copy, x, more, open, chevron, close, file, load, check, first, prev, next, last |
| `Button` | `primary` (accent ground) or `secondary` (outline); 44px, never smaller; text `t-small` 500/700; an `icon` before the label, which sits in `.w-btn-label` so a narrow bar can keep only the icon (the button then carries the label as its `aria-label`) |
| `MenuButton` | A `Button` that opens a list of actions under it, right-aligned: `card` ground, 44px items with an icon in `sub`; Escape and a click outside close it, arrow keys move between items. An item with `checked` is a switch (`menuitemcheckbox`): it stays open when pressed and shows an `accent` check at the right while on |
| `Dialog` | A modal over the page (`<dialog>`): `panel` ground, `border`, 20px padding, title `t-body` 700 with a 44px close button at its right, actions in a wrapping row at the foot; Escape, the backdrop and the close button close it; `inline` draws it in place for the canvas |
| `Tag` | A value's label inside a component (the format a record was read as): a 24px pill, `t-meta` 500 on `card` |
| `LoadRecord` | The load dialog: a 5-row `ff-code` text box that also takes a dropped file (dashed `accent` edge while one is over it), a 120px board of where the record ends with its format `Tag`, file name, disc counts and moves · side to move; Choose a file at the left of the foot, Cancel and Load at the right. Reads what the app reads: moves, a board, a board with moves, GGF, Othello Quest. Its picker, like Load an obf's, lists every file: a type filter greys out .ggf and .obf on iOS, which has no type for them |
| `BoardCover` | A card over the board for what has to be settled before the board is played (a game's colour and strength, and Start the game): the board dimmed behind it in the page's ground, a `panel` card up to 380px with a `t-body` 700 title. It sits in the board's frame, so it is in view wherever the board is, a phone's first screen included |
| `StepNav` | Moving through a record from just under the board: first, back, `n / m` (`ff-code`, the current ply bold), forward, last; 44px buttons, the ends disabled where there is nowhere to go |
| `Segmented` | A choice of pages or modes shown side by side (Play / Study / Benchmark); 44px items in a 3px-padded frame |
| `RadioCards` | Two to four exclusive choices as cards; 44px each. Where the choice is a colour each card holds its `stone`, the name and a `t-meta` `sub` `hint` at the right (first, second); the radio dot goes (it stays for keyboards and readers) and the chosen card shows as `card` ground with a 2px `accent` edge. Disabled, the whole set goes to 0.7 and `sub` text, the chosen card keeps a `border` edge and the others `border-weak` |
| `Select` | A native select styled to 44px, `t-small`, with its own `chevron` 14px in from the right edge (the browser's arrow is turned off); full width in a panel |
| `Strength` | KUROOBI's level: a `Select` of Lv1–Lv13, each named with how it reads (Lv7 · 12 手 · 読切 16 マス), and Custom, which opens two `Select`s for depth and solve under it |
| `Field` | A `t-meta` `sub` label over a control or a value (`.pa-value`, `t-small`), 8px apart |
| `BenchTable` | A run of endgame problems, one 40px row per problem: #, empties, then time (3 decimals), nodes and NPS ending flush right so a phone shows them without scrolling, then the answer (two best moves, `/` between, the rest counted in `sub` `t-meta` (他 7); empty where the obf gives none; a cell too narrow ends in …) and KUROOBI's move and value 12px apart from them; a value that is not the answer in `bad`; the problem being solved shows `Busy`'s dot. A row pressed shows its board and is the current one (`card` ground, `accent` edge). The totals (time, nodes, NPS, and right answers of those solved that have one) close it in 700 once one is solved. `ff-code`, wider than its box scrolls sideways inside it |
| `Toggle` | An on/off switch with its label on the left; a 44px row, a 44×26 switch |
| `Progress` | A known share of work done; a 6px bar, `track` groove, `accent` fill |
| `Busy` | Work running now: an 8px `accent` dot and `t-small` `accent` text |
| `StoneDot` | A disc's colour: 28px in the `Scoreboard`, 18px beside a label, 12px in a table |
| `Panel` | A titled box on the right of the play screen: `panel` ground, `border`, 20px padding, heading `t-body` 700; a `note` (`t-meta` `sub`) at the heading's right |
| `Scoreboard` | Both sides' disc counts above the board, one card each, Black left: 28px stone and a 32px `ff-code` 700 count; no names (the panel says which colour is yours). The side to move stands out: its card raised (`card` ground, `border` edge) and its stone ringed in `accent` with a 3px gap; the other side's stone at half strength and its count in `sub`, on a `border-weak` card. While KUROOBI thinks its ring breathes and `Busy` sits at the right. A card under 260px wide (a phone) sits closer (10px gaps, 12px sides) and keeps only Busy's dot, the words left to screen readers, so the card never wraps. Once the game is over the same two cards, at the same 72px, say who won at that same right, so the stone and the count never move, coming in with a short rise: the winner's card raised with Win (`t-body` 700, `t-small` on a phone), the other's back as an idle side's with Loss in `sub`; a draw raises both with Draw. A card too narrow for that (under 166px, a small phone) puts it under the count. In a game the player's own card carries the tone: a win fills it in `accent-dim` with `on-accent` text, a loss gives it a `bad` edge and a `bad` Loss; study stays neutral. The full `result` (You win / KUROOBI wins / Draw; in study Black wins / White wins / Draw) with the count is the status screen readers hear |
| `Toast` (`.w-toast`) | That something was done (copied): a 44px `card` pill with an `accent` check, at the foot of the screen, centred, for 2 seconds |
| `.w-more` (class) | A link that stands on its own (all notes, open in Study, the related notes): `t-small` 500, a 44px target |
| `.tag` (class) | A label on a card or an article (Web, Search): a pill, `t-meta` 500, `sub` on `panel` |
| `Board` | The board, as the app draws it, with web text: coordinates and eval values never under 13px on screen; an eval circle shows the value only, the best in `mark-*-best` and bold. A move is shown being played: the placed stone grows in (200ms), each stone it turns narrows to nothing and opens in its new colour (320ms), nearer ones first, 60ms apart; until they settle the board shows no legal marks and takes no clicks. With reduced motion there is no movement |
| `CandidateList` | The moves of a position, best first: square (`ff-code`), value; 40px rows, `t-small` |
| `KifuTable` | The record: #, move with `StoneDot`, then by `columns`: `eval` (study) the value with the loss (▼, `bad`) before it, then how the analysis read that position in `sub`: its depth, Solve when solved, or a selective endgame solve's confidence (確率 73%); `none` nothing; `reading` (a game) nothing either, but KUROOBI's own moves end in a `sub` chevron and pressing one (tap, click, Enter) opens under it what it read, as one block with the row (one `card` ground, one hover, the accent edge running down both when it is the current move): a three-column list under the move column, each a `t-meta` `sub` label over its `t-small` `ff-code` value — Eval, Depth (or Solve), Time on the first line, Nodes and NPS on the second. Nothing of it shows until someone asks for that move. 44px rows, `t-small`, heading `t-meta`; its box grows with the record up to 360px and scrolls beyond |
| `EvalGraph` | Eval over the 60 moves of a game, Black's view: drawn in screen pixels, axis labels and legend `t-meta`, the readout of the move under the pointer `t-small` in the heading row; the losing move marked in `bad`. A point solved is filled in `text`, one searched in `accent`, and one solved selectively a `text` ring between them, its readout giving the confidence (確率読み 73%). Points are sized for 9px between moves and shrink with a narrower graph, so a phone's stay apart. A mouse reads out the move under it and jumps there when pressed, and on while dragged; a finger sliding sideways reads out and jumps the same, its readout staying after it lifts, while a vertical slide scrolls the page |

Evals on the screen are whole numbers; which move is best, and the order of
moves, come from the unrounded values.

## Play screen states

A game keeps an unhurried pace whatever KUROOBI's speed: its stone lands no
sooner than 800ms after yours (yours has turned its stones by then), and the
turn comes back to you only once its stones have settled; until then its
card stays the one to move. The result shows once the last stones settle.

The page opens with a `BoardCover` over the board: your colour, KUROOBI's
strength, and Cancel beside Start the game, which waits for the evaluation.
Cancel always closes it on the board as it stands; with no game yet that is
the opening, under it Take back (disabled) and New game as the primary button.
The game panel then holds what applies while playing: Take back and New game
first, the switch for evals on the board (off, and there before a game too),
then the colour and strength the game was started with, disabled (a setting
that cannot change now is shown as one, not as text). New game is the primary
button once the game is over. It brings the cover back over the board as it
stands, so the final position stays in sight and a slip loses nothing; Start
the game clears the board. A game in progress is kept in the tab
(`sessionStorage`), so going to study, a page of its own, and back finds it
again. The evaluation loads, or fails to,
only before a game; a failure is an alert row (`bad` edge and text) with
Reload the page.

The cover has three uses, one title each: Set up a game (New game, from the
opening); Play from this position, when a record that has not ended is loaded
in play or Play from this position is pressed in study, with the record's board,
scores and moves under it, a Starting position `Field` (move n · side to move)
above the colour (the side to move by default) and Start the game; and Game
settings, from Change settings in the game panel (under the colour and strength
it shows disabled) or in a phone's game menu, with Continue the game. Cancel leaves
the game as it was in all three. A changed colour or strength applies from the
position as it stands: KUROOBI reads again if it is now its turn. A loaded record
that has ended goes to study.

Study has a `StepNav` under the board. It reads the position's moves only
when the worker is free: while the whole record is analyzed the candidate list
says so, and at the end of a game there is nothing to read.

The layout is `.pa-*` in `site.css`, which the canvas boards use too. Wide,
the board and what belongs to it (score, `StepNav`, graph) are the left
column and the panels the right. Below 860px the two become one column in the
order a phone is used: score, board, then what moves it, the graph, the record.
In a game that is one row under the board in place of the game panel: Take back,
New game and an icon-only game menu (`more`) holding the evals switch and,
while the game goes on, Change settings; in study it is
the `StepNav` and the analysis panel. Below 560px the bar above keeps to one row: the
page's name is left to the header (it stays for readers), and Load and Share
keep only their icons.

## Benchmark

`/play/bench/` times endgame problems solved to the end in the browser: FFO's four
sets (`problems/fforum-*.obf`) built in, and any obf one loads. The results `Panel`
with the `BenchTable` is the left column; the right holds the run's `Panel` (Benchmark,
the threads the engine searches on as its note, the problem set as a
`Select`, Load an obf, then Start the benchmark, which while it runs becomes `Busy`, n / m, Stop and a
`Progress`) and under it the board of the current problem, with each move's value
from the obf on it where the obf gives them (the best bold, no mark: nothing says
whether they were solved), and its set, number, empties and side to move. A loaded obf joins the sets under its file name and is
chosen; its problems are numbered from 1, and a file with no board in it, or one over
10 MB, is said so in a `Toast`. Each problem is solved
to the end from empty tables. On a narrow screen: the board, the run's panel, the
results.

Moving between play, study and the benchmark does not reload the page (Astro's
`ClientRouter`, with no animation): the engine and its evaluation load once per
visit, so the loading row shows only the first time. A page left while the engine
is still searching restarts it, so the next page does not wait behind that search.

## Site pages

The header, footer, notes index, articles and the 404 page are markup with
classes in `site.css` (`.hd-*`, `.ft-*`, `.nx-*`, `.na-*`, `.np`, `.nf-*`);
the canvas boards use the same classes, so a board and its page cannot drift.
On a phone the header's sections take a second row and the language button
keeps only its globe. Every link standing on its own is 44px; links inside a
sentence are not. An article's code block is a framed figure with the
language and a Copy button in a bar over it, and copying shows a `Toast`;
tables sit in a bordered, scrolling frame. The contents list does not follow
the scroll and marks the first section until another comes into view. An
article has at least one tag; the first is its breadcrumb. The top page leaves
out its notes section, and the play page its related notes, while there are
none. The 404 page answers every missing path in both languages, so it says
everything in both.

A link out of the site opens in a new window and says so: an arrow leaving a
box after its text (`.w-ext-icon`, 14px), and "opens in a new window" for
screen readers. The header's GitHub button, an icon already, carries a smaller
arrow at its corner; a menu item that leaves the site (Share on X) carries it
at its right. Links out of a note get the same when the page loads.

Japanese the site writes breaks only between phrases: `ja.json` carries U+200B
there (and a no-break space inside a name such as Kuroobi for web), and the
element carries `.w-keep`. A note's body, written elsewhere, asks the browser
to break by phrase (`word-break: auto-phrase`) where it can.

## Building

The canvas mounts the components from a bundle built from `web/src/` (not in
git). Run after `npm ci` in `web/`:

```sh
node design-system/web/tools/build.mjs   # -> design-system/web/project/components/bundle.{js,css}
```

`bundle.js` is `window.KuroobiWeb` (it reads `window.React`); `bundle.css` is
the site's style layer (`web/src/styles/site.css`) without the colour tokens,
which the canvas has from its copy of the app's `tokens.css`. Copy both to
the canvas's `project/ds/kuroobiweb/components/` when they change.
