//! What every Tern does, with no toolkit in it. The GTK app and the Android app both stand on this.

// async_trait marks the boxed futures it returns #[must_use], which clippy on stable Rust 1.99 calls doubled.
#![allow(clippy::double_must_use)]

pub mod cleanup;
pub mod compose_model;
pub mod fetcher;
pub mod format;
pub mod html_doc;
pub mod rules;
pub mod semantic;
pub mod settings;
