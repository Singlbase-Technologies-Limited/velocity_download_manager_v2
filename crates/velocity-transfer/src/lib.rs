//! The direct-download engine of V2 (ADR-0002): the logic above libcurl. Only `velocity-curl`
//! calls libcurl; this crate decides what to fetch, which bytes to accept and when a file is done
//! (`.claude/rules/transfer-engine.md`). The engine prototype of #37.

pub mod adaptive;
mod budget;
pub mod download;
pub mod engine;
pub mod engine_log;
mod failure;
pub mod files;
pub mod integrity;
pub mod probe;
pub mod protocol;
pub mod retry;
pub mod scheduler;
pub mod stdio;
pub mod supervisor;
pub mod validator;
