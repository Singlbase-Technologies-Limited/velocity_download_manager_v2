# Security policy

## Reporting a vulnerability

**Please don't report security problems in a public issue.** Report them privately, through GitHub: on the repository's **Security** tab, choose **Report a vulnerability** ([GitHub's guide](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability)). Only the maintainers see the report.

Please include what you found, how to reproduce it, the version (Settings, About and diagnostics), and the operating system. A diagnostics export helps; it has passwords, cookies, tokens and keys removed already ([docs/DIAGNOSTICS.md](docs/DIAGNOSTICS.md)).

**What happens next:** the maintainers acknowledge a report within 7 days, keep you informed while they fix it, and credit you in the advisory unless you'd rather not be named. A fix is released before the advisory is made public.

## Supported versions

V2 is before 1.0. Only the latest release gets security fixes; please update before reporting.

| Version | Security fixes |
|---|---|
| The latest 0.x release | Yes |
| Earlier releases | No |

## What is in scope

Velocity Download Manager V2: the app, the transfer engine, the torrent helper, the browser extension and its native host, and the installers. What V2 protects, and how, is in [docs/SECURITY.md](docs/SECURITY.md): TLS verification, the link and redirect rules, secrets kept out of storage, and file safety.

Vulnerabilities in a library V2 uses, such as libcurl, OpenSSL or libtorrent, are best reported to that project. Tell us too if V2 is affected; V2 tracks its libraries' advisories ([docs/DEPENDENCIES.md](docs/DEPENDENCIES.md)).

The original Velocity Download Manager (V1) is a separate project.
