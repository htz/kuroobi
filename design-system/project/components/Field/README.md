One input field with its name above it. Used for forms stacked vertically in a narrow box (login, editing a server).

## When to use
- When stacking input fields vertically in a narrow box.
- Not for: names in a left column as on the settings screen; use `SettingRow` in `Dialogs.tsx` (name width `w-label`). The two-column tables of the request sheet and the profile use a `w-label-sm` grid.

## Props
- `label` (required): the field's name.
- `children` (required): the input (`TextField`, `Select`, etc.).
- `help`: a note below the field.

## Rules
- Spacing between name, field and note is `sp-1`. The name and note are `fs-6`, `sub`.
- It wraps in a `<label>`, so clicking the name moves focus to the field.
- The spacing between fields is set by the container (`sp-3`).

## Similar components
- `SettingRow`: a settings row that lays out name, field and description side by side.

Used in: `gui/src-ui/js/ggs/Login.tsx`, `SettingsScreen.tsx` (editing a server).
