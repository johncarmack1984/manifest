//! Shared logic used by both the Lambda (read-side grouping) and the local
//! `tag` operator tool. Kept out of the binaries so they can't drift apart.

pub mod anomaly;
pub mod classify;
pub mod registry;
