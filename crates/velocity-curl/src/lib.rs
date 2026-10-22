//! The libcurl layer of V2 (ADR-0002): the only crate that calls libcurl. It links the libcurl V2
//! builds itself from verified sources (build.rs, docs/BUILDING_NATIVE.md).

mod capabilities;
mod runtime;

pub use capabilities::{
    check, IncompatibleCurl, LinkedCurl, Problem, ALLOWED_PROTOCOLS, EXPECTED_TLS, LIBCURL_FLOOR,
    REQUIRED_FEATURES, REQUIRED_PROTOCOLS,
};
pub use runtime::{Libcurl, StartError};
