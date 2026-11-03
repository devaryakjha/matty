//! SQLite cache for Matty: schema, migrations and typed queries.
//!
//! Only the sync engine writes here, and the main thread never opens the
//! database (PLAN §6.2, §6.5). Plain Rust: this crate must never depend on GPUI.
