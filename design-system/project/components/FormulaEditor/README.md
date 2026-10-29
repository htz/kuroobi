An editor that builds the conditions for automatically accepting or declining GGS requests (a GGS formula) as a tree of conditions and bundles.

## When to use
- In GGS settings, where the formula for requests to accept or decline is decided.
- Do not use: to only read a finished formula, use `FormulaView`.

## Props
- `value`: the condition tree (`Cond`). `null` means "Not set".
  A `Cond` is one of: a bundle `{ kind: 'all' | 'any', kids: Cond[], neg? }` (`neg` gives `!( … )`), a condition `{ kind: 'atom', name, op, val, neg, arg? }`, or a formula that cannot be a row `{ kind: 'expr', src }`.
- `onChange`: receives the edited tree. The caller holds the value.
- `onSave`: on "Apply", receives the formula string to send to GGS.
- `onClear`: "Remove the conditions". `onRaw`: on "Write the formula directly", receives the current formula string (an empty string if unset).

## Rules
- When unset, show only "Not set" with a description (`Note`) and the "Add a condition" and "Write the formula directly" buttons.
- When set, stack vertically: the tree editor, "Formula sent" (the string actually sent, shown in monospace), and the bottom-row buttons (Remove the conditions / Write the formula directly / Apply).
- A bundle chooses "All of", "Any of", "Not all of" or "None of" with a `Select` (width 160), and adds children with "+ Condition" and "+ Bundle".
- In a condition row, the item `Select` changes the input fields. Booleans get "is / is not"; colours get "is / is not" plus a colour; numbers get an operator, a comparison target (`Select` "number" (width 80) or an item of the same type) and a number field.
- Function items get an argument field: "my / the opponent's rating change" gets "at [+1.0]" (`md(x)` / `od(x)`), "random number" gets "below [100]" (`rnd(n)`), and "live games" gets "in total / random / standard" (`playing(T)+playing(F)` / `playing(T)` / `playing(F)`). Function rows compare only with numbers.
- "Write as a formula…" at the end of the items turns the row into a formula field (`TextField` mono). Formulas that cannot be rows (arithmetic such as `or+100.0`) also take this form when loaded.
- While any field is red, "Apply" cannot be pressed, and to its left "Fix the fields in red to apply" shows in `--bad` (instead of "Not applied yet").
- Number fields are checked against the GGS type and marked `invalid` when they do not match: ratings (`mr` `or` `md` `od`) allow decimals and get `.0` appended when sent; all others are integers only. Equality is sent as `==`. Colours are `B` `W` `?`.
- Row inputs may wrap, but the × stays fixed at the right end of the row.
- Bundles have corners `r-3`, background `bg`, border `border`, padding `sp-3`. The top-level bundle's left line is `accent`; nested ones use `border`. Inputs are `Select` / `TextField` at size `ctrl`.

## Similar components
- `FormulaView`: shows the same tree as read-only chips.
- `TextArea`: used when writing the formula directly as text.

Used in: GgsScreens.tsx (request conditions `FormulaField` in GGS settings).
