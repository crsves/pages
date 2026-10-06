//! Decode Apple Pages (`.pages`) documents and render them as terminal text.
//!
//! ```no_run
//! let store = pages_cli::iwa::Store::open("essay.pages".as_ref())?;
//! let doc = pages_cli::doc::load(&store);
//! let opts = pages_cli::render::Options { width: 80, color: false, hyperlinks: false };
//! print!("{}", pages_cli::render::render(&doc, &opts));
//! # Ok::<(), anyhow::Error>(())
//! ```

pub mod doc;
pub mod iwa;
mod proto;
pub mod render;
pub mod table;
