---
# Neither path exists yet: the extension is ported in #64, the native host in #65.
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
- **Private networks:** browser links to loopback or private addresses follow the policy decided in #68.
- **Tests:** malformed messages, the app being unavailable, allow and deny rules, magnet links and normal downloads.
