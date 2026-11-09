#![warn(missing_docs)]

//! A small terminal interface for registering and running typed commands.
//!
//! Commands can be selected by typing a prefix, completed inline, and supplied
//! with positional or named arguments. Missing arguments are prompted
//! interactively before the command handler is called.
//!
//! # Example
//!
//! ```no_run
//! use command_tui::{Argument, CommandSpec, CommandTui};
//!
//! let commands = vec![CommandSpec::new(
//!     "greet",
//!     "Print a greeting",
//!     vec![Argument::text("name", "Who should be greeted?")],
//!     |arguments| {
//!         println!("Hello, {}!", arguments[0]);
//!         Ok(())
//!     },
//! )];
//!
//! CommandTui::new(commands)?.run()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
mod argument;
mod command;
mod completion;
mod history;
mod parser;
mod ui;

pub use argument::{Argument, ArgumentKind};
pub use command::{CommandAction, CommandConfigError, CommandSpec, CommandTui};

#[cfg(test)]
mod tests;
