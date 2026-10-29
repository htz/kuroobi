Shows a GGS match-request condition formula (accept, decline, request) as a readable tree of chips. Display only; it does not edit.

## When to use
- In a player's profile on the player screen, to show that player's conditions (`accept` / `decline` / `request`).
- Not for rewriting conditions (`FormulaEditor`).

## Props
- `node` (required): the condition tree `Cond`. A group is `{ kind: 'all' | 'any', kids, neg? }`; a leaf is `{ kind: 'atom', name, op, val, neg, arg? }` or `{ kind: 'expr', src }`. In the app it is built from a string with `parseCond()` (as in GGS, `&` and `|` have equal precedence and associate to the right).
- `top`: `true` for the outermost group (its left line becomes `accent`).
- `who`: `{ me, them }`. The names used in phrases like "{who} rating". In the profile, `me` is that player's name and `them` is "my".

## Rules
- A group has a 2px line on its left (`accent` outside, `border` inside) and a heading (`fs-7`, weight 600). "All of" and "Not all of" are `sub`; "Any of" and "None of" are `gold`. The chips inside wrap (5px gap); nested groups are drawn inside as they are. Left padding is `sp-2h`.
- Leaf chips have a `panel` background, `border` frame, `r-1` corners, `ff-mono`, `fs-6`. A formula that cannot be shown as a row appears with a dashed frame and `sub` text as "formula mr>or+100.0". Functions read as "alice rating change at a +1.0 finish ≤ 0.0", "random number below 100 ≥ 50", "alice live games in total > 0".
- Passing only a leaf renders a single chip.

## Similar components
- `FormulaEditor`: for building and sending your own conditions. FormulaView is for reading someone else's conditions.

Used in: `gui/src-ui/js/GgsScreens.tsx` (the profile on the player screen).
