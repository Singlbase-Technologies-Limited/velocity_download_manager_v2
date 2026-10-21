---
paths:
  - "*.md"
  - "docs/**/*.md"
---

# Documentation

- **Evidence first:** code, tests and accepted decisions are the evidence. Check every command, path and example against the repository before writing it.
- **One canonical document per topic.** Link to it instead of copying instructions. New documents go into [docs/README.md](../../docs/README.md). New terms go into the [glossary](../../docs/GLOSSARY.md).
- **Root README:** it stays an entry point. Review it in every PR, and change it only when the change is relevant.
- **Say how you know:** distinguish verified behaviour, documented capability, assumptions and checks that were blocked or not run. Show real commands and real results.
- **Decisions:**
  - Accepted decisions become ADRs ([docs/decisions/](../../docs/decisions/README.md)), and accepted ADRs are never rewritten.
  - Research is input, and copies in `docs/research/` stay verbatim.
- **Don't invent** licensing, security contacts, release dates, platform support or PO decisions. Surface them as open questions.
- **Write plainly:** short sentences, plain words, and specifics such as numbers, units, names and dates, instead of adjectives.
