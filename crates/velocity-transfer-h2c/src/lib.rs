//! Holds only tests that need test-only features: the direct-download engine against the local
//! HTTP/2 server without TLS (`tests/http2.rs`, velocity-curl's `test-h2c`), against the TLS
//! fixture's test CA (`tests/tls.rs`, `test-ca`), and with a full disk or a refused write
//! mid-download (`tests/disk_faults.rs`, velocity-transfer's `test-faults`, #74). See this crate's
//! `Cargo.toml` for why they aren't in `velocity-transfer`.
