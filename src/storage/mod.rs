//! Durable operation journal and private atomic filesystem operations.
mod atomic;
mod journal;
pub use atomic::{atomic_write, private_options};
pub use journal::{Journal, Record, Storage};
