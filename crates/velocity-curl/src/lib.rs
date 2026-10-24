//! The libcurl layer of V2 (ADR-0002): the only crate that calls libcurl. It links the libcurl V2
//! builds itself from verified sources (build.rs, docs/BUILDING_NATIVE.md).

// HTTP/2 without TLS is for the tests against a local server only (PO decision on #47): an
// optimised build with it doesn't compile, so a release can't contain it.
#[cfg(all(feature = "test-h2c", not(debug_assertions)))]
compile_error!("the test-h2c feature is for test builds only");
// Trusting a test CA file is for the TLS tests only (PO decision on #69), likewise.
#[cfg(all(feature = "test-ca", not(debug_assertions)))]
compile_error!("the test-ca feature is for test builds only");

mod capabilities;
mod fetch;
mod ftp;
mod head;
mod pipe;
mod rate;
mod redirect;
mod runtime;
mod url;

pub use capabilities::{
    check, IncompatibleCurl, LinkedCurl, Problem, ALLOWED_PROTOCOLS, EXPECTED_TLS, LIBCURL_FLOOR,
    REQUIRED_FEATURES, REQUIRED_PROTOCOLS,
};
pub use fetch::{
    CurlFailure, CurlLoop, FetchError, FetchEvent, FetchRequest, Gate, HostKeyProblem, LoopError,
    LoopOptions, RequestOptions, ResponseHead, SshKeyFile,
};
pub use head::file_head;
pub use pipe::{Fetch, FetchControl, FetchLimit, FetchPipe, GateFn, Offered, Transport};
pub use rate::RateLimit;
pub use runtime::{Libcurl, StartError};
pub use url::{origin, Origin};
