---
paths:
  - "src/**/*.{ts,tsx,css}"
  - "index.html"
  - "vite.config.ts"
---

# Frontend

- **Components handle presentation and interaction.** State transitions and business rules that repeat belong in one hook, store or native command, never copied across components.
- **Reuse** the existing components, dialogs, notifications, error presentation and design tokens. Add no new styling system or component framework, and no dependency for something the stack already does.
- **Native calls** go through the typed bridge only. Every native response is validated before use with the strict schemas in `src/contract.ts`, which contract fixtures generated from Rust keep in step (docs/CONTRACTS.md). No direct file, process or network access from the UI.
- **Every state is explicit:** loading, empty, success, error, paused, failed and unavailable. Never show fake progress, invented statistics or success after a failed command.
- **Validation:** validate input in the UI for usability. Security-critical validation is repeated in Rust.
- **Accessibility:** keep keyboard operation, focus management, screen-reader labels, reduced-motion preferences and light and dark themes working (#66).
- **Long lists stay responsive.** No state updates or storage writes on every pointer or animation frame.
- **Tests** check what the user sees and does, with Vitest, not component internals.
- **Names:** see [docs/CONVENTIONS.md](../../docs/CONVENTIONS.md#typescript-and-react).
