# Velocity Download Manager V2

Velocity Download Manager V2 is a desktop download manager under development: a Tauri 2 and React application with its own Rust direct-download engine (HTTP, HTTPS and FTP over libcurl, SFTP over russh, and Metalink files) and libtorrent for BitTorrent. It is a separate application from the original Velocity Download Manager and is designed to be installed side by side with it.

## Getting started

```sh
pnpm check:prereqs
pnpm install --frozen-lockfile
pnpm dev
```

The full setup for Windows, Linux and macOS is in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md). Common problems are in [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md). The check commands and verification levels are in [docs/TESTING.md](docs/TESTING.md).

## Documentation

[docs/README.md](docs/README.md) maps every document. The architecture is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), and accepted decisions are in [docs/decisions/](docs/decisions/README.md).

## Contributing

Work is planned as issues on the [project board](https://github.com/users/vincentarisawork/projects/2). Start with [CONTRIBUTING.md](CONTRIBUTING.md). Contributor and AI-agent rules are in [AGENTS.md](AGENTS.md); Claude Code reads them through [CLAUDE.md](CLAUDE.md). AI agents work as the machine account `velocity-claude-agent`; see [docs/AI_AGENT_ACCOUNT.md](docs/AI_AGENT_ACCOUNT.md). The branch, pull-request, merge and release workflow is in [docs/DEVELOPMENT_WORKFLOW.md](docs/DEVELOPMENT_WORKFLOW.md), and the review checklist is in [docs/REVIEWING.md](docs/REVIEWING.md).

## Licence

Not yet chosen (issue #86). All rights reserved until then.
