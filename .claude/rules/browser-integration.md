---
# pending: "crates/velocity-native-host/**/*" (#65)
paths:
  - "browser-extension/**/*"
  - "crates/velocity-native-host/**/*"
---

# Browser extension and native messaging

- **Targets:** Chromium Manifest V3 and Firefox. Build and test both. A successful build doesn't prove that native messaging works in a real browser.
- **Untrusted input:** every page URL, download event, extension message and native-host payload is untrusted.
- **Validated twice:** the native host checks the message shape, version, size, scheme, options and command allowlist, then the app checks them again when it accepts the request.
- **Narrow channel:** only explicitly allowed schemes and commands are accepted. Magnet links take the torrent route, never the HTTP path. The extension never gets file, shell or engine access.
- **Identity:** V2 registers its own native-host name and extension ids, separate from V1's (#11, #65). It never intercepts V1's messages.
- **Permissions and fallback:** request the minimum browser permissions. When V2 can't take a download, the browser's own download continues. Don't intercept browser-internal URLs, or the same item twice.
- **Private networks:** browser links that resolve to loopback or private addresses need the user's confirmation; typed links don't ([ADR-0006](../../docs/decisions/0006-browser-links-to-local-networks.md); built in #67).
- **Tests:** malformed messages, the app being unavailable, allow and deny rules, magnet links and normal downloads.
