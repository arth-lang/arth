//! Lint pass implementations.
//!
//! Each lint pass checks for a specific category of issues.

mod async_lint;
mod unused;

pub use async_lint::AsyncWithoutAwaitPass;
pub use unused::UnusedVariablePass;
