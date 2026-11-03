//! The sync engine against a real Mattermost server (#40), such as the seeded
//! dev server (docs/dev-server.md):
//!
//! ```text
//! MATTY_TEST_SERVER=http://localhost:8065 MATTY_TEST_PASSWORD='Password1!' \
//!     cargo test -p matty-sync --test integration
//! ```
//!
//! Without `MATTY_TEST_SERVER` every test returns at once. Each test makes
//! its own team, users and channel through the admin account, so the tests run in
//! parallel without seeing each other's posts or read state.
#![allow(clippy::unwrap_used)] // Tests unwrap.

mod crt;
mod load;
mod reconnect;
mod world;
mod writes;
