//! QueryForge public API.
//!
//! The top-level `queryforge` crate is the public library. The workspace also
//! contains a thin CLI wrapper and a thin `build.rs` helper crate.

pub mod backends;
pub mod codegen;
pub mod config;
pub mod diagnostics;
pub mod engine;
pub mod error;
pub mod fingerprint;
pub mod ir;
pub mod names;
pub mod nullability;
pub mod parser;
pub mod runtime;
pub mod sql_ir;
pub mod type_map;

#[cfg(feature = "build-api")]
pub mod build;

pub use config::Config;
pub use engine::{
    check, generate, prepare, CheckOptions, CheckReport, GenerateMode, GenerateOptions,
    GenerateReport, PrepareOptions, PrepareReport,
};
pub use error::{Error, Result};
pub use fingerprint::Fingerprint;
