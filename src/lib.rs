//! `.vni`: a version-controlled image.
//!
//! One file holds a base image plus an append-only chain of commits, each with
//! an author, timestamp and message, and a compact pixel delta from its parent.
//! See `SPEC.md` for the byte-level format.

mod commit;
mod error;
mod file;
mod pixels;
mod time;

pub use commit::{to_hex, Author, Commit, Payload, FORMAT_VERSION, MAGIC};
pub use error::{Error, Result};
pub use file::{short_id, VniFile};
pub use pixels::{DiffStats, Id, Pixels};
pub use time::{format_utc, now};
