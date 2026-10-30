# Accessibility

What V2 does for keyboard and screen-reader users, and how each behaviour is checked. It is the audit of #66: V1's keyboard and accessibility behaviours (V1's `tests/unit` and `.claude/rules/frontend.md` at `e9d717a8`), each with an automated test in V2 or a manual check. The rule for new UI work is in `.claude/rules/frontend.md`.

## The audit

Each behaviour has a test, run with the frontend's Vitest suite (`pnpm test:focused -- frontend`), or a manual check below.

### The download list

| Behaviour | Checked by |
|---|---|
| The list is a grid named "Downloads", reachable with Tab, with the selected rows marked `aria-selected` | `src/features/downloads/DownloadsView.test.tsx`, "supports keyboard navigation and space to pause" |
| ↑ and ↓ move the selection, Home and End go to the first and last row, Shift with an arrow extends it | the same test; `src/app/Accessibility.test.tsx`, "End selects the last row…" |
| Ctrl+A (⌘A) selects every row, Escape selects none | `Accessibility.test.tsx`, "End selects the last row, Ctrl+A every row, and Escape none" |
| Enter opens the focused row's details | `Accessibility.test.tsx`, "Enter opens the details of the focused row" |
| Space pauses or resumes the selection | `DownloadsView.test.tsx`, "supports keyboard navigation and space to pause" |
| Delete asks before removing the selection, Shift+Delete before deleting its files too | `Accessibility.test.tsx`, "Delete asks before removing…"; `DownloadsView.test.tsx` (Shift+Delete) |
| A row's progress bar is named after its file; torrents and direct downloads are told apart | `DownloadsView.test.tsx`, "renders status and real progress from the snapshot"; the names sweep below |
| Column widths change with ← and →, and Home restores the default, on a separator named "Resize … column" | `DownloadsView.test.tsx`, the column tests |

### Panels, dialogs and menus

| Behaviour | Checked by |
|---|---|
| The navigation and details panel resize with the arrow keys (Shift for bigger steps), Home and End, and collapse with Enter, on separators that say their value and bounds | `src/components/Splitter.test.tsx`; `src/app/App.test.tsx`, "the panels" |
| A dialog takes focus when it opens, keeps Tab inside, closes with Escape, and gives focus back to what opened it | `Accessibility.test.tsx`, "focus in dialogs" |
| A confirmation (an alert dialog) takes focus, and Escape cancels it | `Accessibility.test.tsx`, "Delete asks before removing…" |
| Menus open with Enter, move with the arrows, close with Escape and give focus back | `Accessibility.test.tsx`, "menus from the keyboard" |
| Ctrl+N (⌘N) opens the Add dialog, Ctrl+, (⌘,) opens Settings | `src/features/add/AddDownloadDialog.test.tsx`; `src/features/settings/SettingsView.test.tsx` |
| Number fields commit with Enter and on leaving the field | `src/features/downloads/DetailsPanel.test.tsx`; `SettingsView.test.tsx` |

### Names, announcements and the look

| Behaviour | Checked by |
|---|---|
| Every button, checkbox, field, menu item, separator, progress bar, dialog and landmark has a name, in the list with the details panel, with the navigation as icons, in the Add dialog, on the Categories and History screens, and in every Settings section | `Accessibility.test.tsx`, "every control has a name, and icons stay silent" |
| Icons inside buttons and menu items are hidden from screen readers, as is the brand icon | the same tests; lucide-react hides its icons unless given a label, and `BrandIcon` is `aria-hidden` |
| Errors are announced (`role="alert"`), and saving and progress messages are read politely (`role="status"`) | `DetailsPanel.test.tsx`, `SettingsView.test.tsx`, `App.test.tsx` |
| Text has at least 4.5:1 contrast on every background it is shown on, in both themes, and the focus ring at least 3:1 (WCAG 2.2 AA) | `src/a11y.test.ts`, which reads `src/index.css` |
| Whatever has keyboard focus is outlined in the ring colour | `src/a11y.test.ts` |
| With the system's reduced-motion setting, animations and transitions stop | `src/a11y.test.ts` |
| The light and dark themes follow the system or the saved choice | `App.test.tsx`, "the theme" |
| Screen readers read the list, a dialog, the details panel and Settings in a sensible order, with what each control does | **Manual** (below) |

## What the audit found

- **Contrast below AA (fixed):** hint text measured 3.05:1 at worst in the light theme and 3.32:1 in the dark one, and the selected navigation item, success, warning and accent text fell just short on some backgrounds (4.08 to 4.49). Six colours were nudged to the smallest change that reaches 4.5:1 (PO decision on #66): light hint `#7b879a`→`#606b7d`, accent `#1d6fd8`→`#1b68cb`, success `#15803d`→`#14793a`, warning `#b45309`→`#aa4f09`; dark hint `#6f7b8f`→`#8993a3`, accent `#4b8ff0`→`#5092f0`.
- **Focus was lost when a dialog closed (fixed):** the dialogs open from the app's state rather than from a trigger button, so when one closed, focus fell to the page and the next Tab started from the top. A dialog or confirmation now gives focus back to whatever had it when it opened.
- **The row menu needs a pointer (as in V1):** each row has its own right-click menu, and the list takes focus as a whole, so Shift+F10 or the menu key doesn't open a row's menu. Its actions are reachable otherwise: Space, Delete and Enter on the list, the toolbar's buttons for the selection, and the details panel (open, show in folder, copy the link, the options). Raised with the PO as a possible follow-up rather than built here, since the issue keeps to V1's behaviour.

## The manual screen-reader check

Run by the PO during review (PO decision on #66), on Windows, with NVDA or Narrator, in `pnpm dev` with a few downloads in the list. Record each step as PASS or FAIL with what was read, in the PR and here.

1. **Start:** Tab from the window's top. The navigation is announced as "Primary", and each item by its name and count.
2. **The list:** Tab to the list. It is announced as the "Downloads" grid. ↑ and ↓ read each row's file name, status and progress; Space pauses or resumes it, and the change is read.
3. **Details:** Enter on a row opens the details panel; it is announced as "Download details", and its sections (Transfer, Source and destination, Timeline, Options for this download) by name.
4. **A dialog:** Ctrl+N opens "Add download". Focus is in the Links field; Tab goes through every field, each read with its label, and never leaves the dialog. Escape closes it, and focus is back where it was.
5. **A confirmation:** select a row and press Delete. The question is read as an alert dialog with its two buttons; Escape cancels.
6. **Settings:** Ctrl+, opens Settings. Each section button, then each setting, is read with its name and state; changing a toggle reads "Saved".
7. **Errors:** with the transfer engine stopped, or a failed download selected, the error is read when it appears.
8. **Reduced motion and contrast:** with Windows' "Animation effects" off, the progress bars and dialogs don't animate; with a high-contrast theme, text and focus stay visible.

**Result:** NOT RUN yet; it is done during the review of #66.
