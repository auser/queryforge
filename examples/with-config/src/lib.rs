//! Example app consuming QueryForge generated code.
//!
//! This assumes `queryforge-build` writes generated files to:
//!
//! ```text
//! $OUT_DIR/queryforge/mod.rs
//! ```
//!
//! If your build helper writes to `src/db` instead, replace this module with:
//!
//! ```text
//! pub mod db;
//! ```

pub mod db {
    include!(concat!(env!("OUT_DIR"), "/queryforge/mod.rs"));
}
