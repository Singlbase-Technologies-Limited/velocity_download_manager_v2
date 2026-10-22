# Engine internals

How the direct-download engine in `crates/velocity-transfer` fetches a file. The contract the manager sees is in [ENGINE_CONTRACT.md](ENGINE_CONTRACT.md), and how the engine's child process talks to the app is in [ENGINE_PROTOCOL.md](ENGINE_PROTOCOL.md). The design comes from section 5 of the [research report](research/2026-10-21-libcurl-engine-research-r2.md); the terms are in the [glossary](GLOSSARY.md).

## The probe (#41)

The probe is the download's first request and also its first segment, so its bytes are kept. It is `GET` with `Range: bytes=0-(S−1)`, where S is `DownloadOptions::probe_bytes`. That's 4 MiB in the engine, the report's starting value for #47 to tune.

- **No HEAD:** a HEAD request is never sent, and `Accept-Ranges` is never taken as proof of range support.
- **No encoding:** no `Accept-Encoding` is sent. A body with any other encoding than identity isn't the file's own bytes.
- **Decided before any byte:** the final response is classified in libcurl's header callback ([`probe.rs`](../crates/velocity-transfer/src/probe.rs)). An answer the engine refuses is aborted there, so none of its body is written.

| Final response | What the engine does | `Mode` |
|---|---|---|
| `206`, `Content-Range: bytes 0-b/T` | Keeps bytes 0 to b, then fetches the rest as parallel segments | `Ranges` |
| `206` with fewer bytes than asked (b < S−1) | Keeps them; the rest is part of the segments | `Ranges` |
| `206`, `bytes 0-b/*` (no total) | Keeps bytes 0 to b. It asks for the rest with one open range, `bytes=(b+1)-`, and repeats from the first missing byte until a `416` says the file ends there. No parallel segments | `SequentialRanges` |
| `206` `multipart/byteranges` | Aborts, and retries once without `Range` | `SingleStream` after the retry |
| `200` | Keeps this response as the whole file: no second request, nothing wasted | `SingleStream` |
| `416` with `bytes */0` | An empty file | `Empty` |
| `401`, `407` | Fails: the server needs credentials. Not retryable | — |
| `429`, `503` | Fails as throttled, retryable. Waiting and honouring `Retry-After` are #48 | — |
| Other `4xx` | Fails, not retryable | — |
| Other `5xx` | Fails, retryable | — |
| A malformed `Content-Range`, a `Content-Length` that disagrees with it, a start other than 0, an end past the request or the total, an unrequested encoding | Fails before writing, not retryable | — |

## Later segments

After a probe that gave a total, the rest is split into at most `connections` segments, requested at once as `Range: bytes=a-b`. Each answer is checked in the header callback (`check_segment`):

- **Accepted:** a `206` for exactly `a` to at most `b`, of the same total, with identity encoding. Its bytes are written only up to the end its own `Content-Range` gave.
- **Ranges lost:** a `200` is aborted before its body is written. The rest of the file is fetched as one stream that writes only the missing bytes, so the file is never mixed.
- **Anomaly:** an answer that contradicts its request, such as a wrong or malformed `Content-Range`, a length that disagrees with it, or a multipart body. Also a body cut off before its range ended, a chunked body that stopped short of its `Content-Range`, or one that ran past it.
- **Failure:** an error status, or a different total, which means the file changed (#45).

After each round, the missing parts are requested again as ranges, at most one per connection:

- **Short answers:** a `206` with fewer bytes than asked has only its remainder requested.
- **Anomalies:** the first is tolerated, and the bytes involved are requested again. With the **second** on the same download (`MAX_ANOMALIES`), ranges stop for it and the rest comes as one stream that writes only the missing bytes (`RangesLost`).
- **No progress:** a round that delivers no new byte counts as an anomaly itself, so the loop always ends.

## Accounting

Every received byte is either written into the part file once, or counted in `wasted_bytes`. That includes bytes past a segment's end, bytes the single stream sends again, and bytes already covered. Progress counts written bytes only. A server that ignores ranges costs exactly one GET with no wasted bytes, which a fixture test checks.

## Tests

`crates/velocity-transfer/tests/download.rs` runs the real engine against the local test server ([`velocity-test-http`](../crates/velocity-test-http/src/lib.rs)), whose `RangeMode` makes it misbehave on demand. There is one test per row of report section 5.1's table, each checking the file's bytes, the number of GETs and the wasted bytes. The unit tests in `probe.rs` cover each rule on its own.

## Not yet

- **HTTP/2:** the fixtures run over HTTP/1.1. The HTTP/2 fixtures, with the HTTPS test server they need, are #47 (PO decision on #41). libcurl gives the engine the same final status and headers over either version.
- **Retries and waits** for errors, throttling and stalls: #48.
- **Validators and changed files:** a changed `Content-Encoding` between responses is an anomaly for now. Treating it, a changed total and changed validators as a changed object is #45.
- **The lease scheduler** with work-stealing, which replaces the rounds above: #42.
