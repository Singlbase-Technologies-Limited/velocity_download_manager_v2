# ADR-0015: Logins are saved in the system keychain only when the user asks, one download at a time

- **Status:** Accepted
- **Date:** 2026-10-29
- **Decided by:** the Product Owner
- **Issue:** #70

## Context

V1 kept logins, custom headers and SSH keys in memory only, so a download that needed them came back after a restart without them, and the user typed them again (V1's `docs/ARIA2_INTEGRATION.md`; V2 does the same since #58). #70 asked whether V2 should offer the operating system's keychain instead, with explicit consent, and noted that a keychain crate is a new dependency.

- **What is checked:** the `keyring` crate, 4.2.0 (MIT OR Apache-2.0, maintained by the open-source-cooperative project), with its default `v1` feature: Windows Credential Manager on Windows, the Secret Service (GNOME Keyring, KWallet) on Linux through `zbus`, and Keychain Services on macOS. No advisories in OSV for it or the 11 crates it brings (checked 2026-10-29; [DEPENDENCIES.md](../DEPENDENCIES.md)).
- **Not everywhere:** a Linux session without a Secret Service, such as WSL or a minimal desktop, has no keychain. The crate then reports an error, which V2 shows.

## Decision

**Save a login in the keychain only when the user asks, for that download.**

- **Consent:** "Remember this login in the system keychain", next to the login fields in the Add dialog, unticked by default. Ticked, V2 saves that download's login, custom headers and SSH key passphrase after adding it (`remember_login`).
- **One entry per download**, under V2's own service name (`<identifier>.logins`), never V1's. Removing the download deletes it.
- **After a restart** the download gets its login back from the keychain and carries on as it was. Without one (not remembered, deleted from the keychain, or the keychain unavailable), it waits for the login as before, with a note saying why.
- **Everything else stays as it was:** nothing secret goes to the database, checkpoints, logs or a process's arguments, and secrets in memory are wiped when dropped (`zeroize`).

## Options considered

| Option | For | Against |
|---|---|---|
| **Keychain, with consent per login (chosen)** | Logins survive a restart for the user who wants it; nothing is saved without asking; the OS protects the secret | A new dependency; Linux needs a Secret Service |
| Memory only (V1's) | No dependency, nothing stored | Every restart asks for every login again |
| One setting that saves every login | Simplest to use | Saves secrets the user may not expect to be saved |

## Consequences

- **The Add dialog** has the "Remember this login" box, and the contract has `remember_login` and `KEYCHAIN_UNAVAILABLE` ([CONTRACTS.md](../CONTRACTS.md)).
- **The stored options** record that a login was saved (`loginSaved`), never the login itself ([DATA_AND_MIGRATIONS.md](../DATA_AND_MIGRATIONS.md)).
- **What is saved and checked** is in [SECURITY.md](../SECURITY.md#secrets-70).

## Follow-up

- **Not in this decision:** proxy passwords (#192 decides where they go), and a torrent's tracker passkeys, which still aren't kept across a restart ([TORRENTS.md](../TORRENTS.md)).
- **Revisit** if a platform's keychain proves unreliable, or users ask to remember a login given again after a restart (`set_download_login`).
