An indicator that work is running, such as "Thinking" or "Analyzing". Places an `accent` dot next to text.

## When to use
- To briefly report work that is running now (currently only importing on the book screen; thinking and analyzing are shown by the status bar with its own dot and text).
- Not alone: if the progress ratio is known, add `Progress` as well.

## Props
- `children`: the text. Supplementary info such as a count may follow (the caller colours it `sub`).

## Rules
- A leading `Dot` (tone `accent`), spacing `sp-2`, text colour `accent`. The dot does not blink.

## Similar components
- `Dot`: the dot alone.
- `Progress`: a ratio bar.

Used in: `gui/src-ui/js/App.tsx` (the aux "Importing {done} / {total} positions" of the book screen's Toolbar).
