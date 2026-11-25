# Release Desk: working rules

## UI rules
- **Native form controls only.** `<input>`, `<select>`, `<textarea>`, checkboxes and radios use the
  operating system's own controls (dark through `color-scheme: dark` in `src/index.css`). Do not
  restyle them, build custom dropdowns, comboboxes or toggles, or add a component library for them.
  `inputCls` in `src/components/ui.tsx` is layout only (width). Use `<datalist>` for suggestions.
- Use the native file and folder pickers and message boxes from `@tauri-apps/plugin-dialog`.
- Dark theme by default; keyboard shortcuts for every main action.

## Safety rules (spec §0)
- Every remote action goes through `exec::Ctx::side_effect` (dry run) and `pipeline::guard_remote`
  (local-only). Never force-push. Tokens only in the OS keychain; logs go through `redact`.
- Conventional Commits. Commit locally; push only when the owner says so.

## Checks
- `npm run test:core` (Rust core, offline) and `npm run build` (type check + bundle).
