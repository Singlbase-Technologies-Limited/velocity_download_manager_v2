//! The direct-download engine of V2 (ADR-0002): the logic above libcurl. Only `velocity-curl`
//! calls libcurl; this crate decides what to fetch, which bytes to accept and when a file is done
//! (`.claude/rules/transfer-engine.md`). The engine prototype of #37.

#[cfg(all(feature = "test-faults", not(debug_assertions)))]
compile_error!("the test-faults feature is for test builds only");

pub mod adaptive;
mod budget;
pub mod download;
pub mod engine;
mod failure;
#[cfg(feature = "test-faults")]
pub mod faults;
pub mod files;
pub mod integrity;
pub mod probe;
pub mod protocol;
pub mod retry;
pub mod scheduler;
pub mod sources;
pub mod stdio;
pub mod supervisor;
pub mod validator;
