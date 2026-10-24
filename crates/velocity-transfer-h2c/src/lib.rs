//! Holds only tests that need velocity-curl's test-only features: the direct-download engine
//! against the local HTTP/2 server without TLS (`tests/http2.rs`, `test-h2c`), and against the
//! TLS fixture's test CA (`tests/tls.rs`, `test-ca`). See this crate's `Cargo.toml` for why they
//! aren't in `velocity-transfer`.
