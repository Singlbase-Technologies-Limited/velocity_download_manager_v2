# Velocity Download Manager V2

Velocity Download Manager V2 is a desktop download manager under development: a Tauri 2 and React application with its own Rust direct-download engine (HTTP, HTTPS, FTP and SFTP over libcurl) and libtorrent for BitTorrent. It is a separate application from the original Velocity Download Manager and is designed to be installed side by side with it.

## Getting started

```sh
pnpm check:prereqs
pnpm install --frozen-lockfile
pnpm dev
```

The full setup for Windows, Linux and macOS is in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md). Common problems are in [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md).

## Contributing

Work is planned as issues on the [project board](https://github.com/users/vincentarisawork/projects/2). Contributor and AI-agent rules are in [AGENTS.md](AGENTS.md); Claude Code reads them through [CLAUDE.md](CLAUDE.md).

## Licence

Not yet chosen (issue #86). All rights reserved until then.
