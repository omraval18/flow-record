//! **fr** — record terminal commands once, replay them as parameterized workflows.
//!
//! # Architecture
//!
//! | module | responsibility |
//! |---|---|
//! | [`cli`] | argument parsing and dispatch |
//! | [`session`] | the per-shell log the shell hooks append to while recording |
//! | [`record`] | the post-recording review that turns a session into a workflow |
//! | [`edit`] | interactive editing of a saved workflow |
//! | [`run`] | resolving, binding arguments, confirming and rendering a replay |
//! | [`manage`] | `list`, `show`, `export`, `rm` and completion data |
//! | [`store`] | workflow files, project/global scoping, ranking and trust |
//! | [`workflow`] | the workflow data model and its invariants |
//! | [`render`] | shell-aware placeholder substitution (pure) |
//! | [`suggest`] | parameter and name suggestions (pure) |
//! | [`destructive`] | heuristic detection of dangerous steps (pure) |
//! | [`ui`] | prompts and styling behind the [`ui::Ui`] trait |
//!
//! Everything that touches the outside world takes its dependencies explicitly
//! (a [`store::Store`] for the filesystem, a [`ui::Ui`] for the terminal, a
//! [`std::io::Write`] for output), which keeps the logic testable without a TTY.

pub mod cli;
pub mod destructive;
pub mod edit;
pub mod error;
pub mod manage;
pub mod record;
pub mod render;
pub mod run;
pub mod session;
pub mod store;
pub mod suggest;
pub mod ui;
pub mod workflow;

pub use error::{Error, Result};
