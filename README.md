# command-tui

A small Rust library for applications that need an interactive command picker:
users type a command prefix, accept its completion, answer any missing
arguments, and invoke the app-provided handler.

## Usage

```rust,no_run
use command_tui::{Argument, CommandSpec, CommandTui};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let commands = vec![CommandSpec::new(
        "greet",
        "Print a greeting",
        vec![Argument::text("name", "Who should be greeted?")],
        |arguments| {
            println!("Hello, {}!", arguments[0]);
            Ok(())
        },
    )];

    CommandTui::new(commands)?.run()?;
    Ok(())
}
```

Commands are matched by case-insensitive prefix. The interface displays the
shortest matching completion (ties follow registration order) as a dark gray
suffix in the input line. Press Enter to run the completion and supply its
arguments; Tab or Space accepts the completion and inserts a space so you can
start entering arguments immediately; Esc clears the current command line.

The resolved full command name is shown during argument entry and echoed
before the handler runs.

During argument entry, Esc cancels the command. If nothing matches, no command
can be run.

## Built-in Commands

The built-in `help` command lists commands without argument details; use
`help <command>` to show that command's description and detailed arguments.
Command names and unique prefixes are accepted. `quit` exits the interface;
both built-in names are reserved.

## Command History

Use Up and Down at the command prompt to cycle through submitted commands,
including the resolved command name and command-line arguments as originally
typed. History wraps through the draft: Up from the oldest command restores
the draft, and Down from the draft returns to the oldest command.

## Arguments

Arguments are entered one at a time as indented `   * name: value` lines, with
each argument's description below its input. The terminal cursor remains at
the input while typing. Once accepted, the description is cleared and the
completed argument line remains before the next prompt.

Values are passed to the handler in declaration order as strings. `Argument`
supports text, integer, boolean, fixed-choice, path, file, and directory
values, as well as optional and default values.

* Path, file, and directory arguments accept missing paths, so commands can
  create new entries. If an entry already exists, file and directory arguments
  enforce the corresponding type. Press Tab while entering a path, file, or
  directory value, either on the command line or in an argument prompt, to
  complete matching filesystem entries; directory completions add a trailing
  separator.

* Choice arguments autocomplete in place: a dark gray suffix appears when the
  typed prefix matches exactly one choice; Enter or Tab accepts that
  completion, including for arguments typed on the command line. Ambiguous or
  unmatched choice prefixes are rejected with an explanation.
  
Arguments can also be supplied on the command line, positionally or by name:
`greet david` or `greet name: david`. Use double quotes to include spaces in a
value, and double an inner quote to escape it, for example
`greet message: "say ""hello"""`.

Supplied values are validated, and only missing arguments are prompted for.
The display adapts to terminal width, wrapping help text and clipping long
command rows and input lines. The interface stays in the normal terminal
screen, and handlers can use standard terminal I/O.

## Demo

Run the demo with `cargo run --example demo`.
