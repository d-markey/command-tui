use std::{error::Error, fmt};

use crate::argument::{Argument, ArgumentKind};

/// A command handler, invoked with validated argument values in declaration order.
///
/// Return `Ok(())` on success or an error message to display to the user.
pub type CommandAction = dyn Fn(&[String]) -> Result<(), String> + Send + Sync;

#[derive(Debug)]
/// An error found while validating a command registry.
pub struct CommandConfigError(String);

impl fmt::Display for CommandConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for CommandConfigError {}

impl From<String> for CommandConfigError {
    fn from(message: String) -> Self {
        Self(message)
    }
}

/// A command registered with [`CommandTui`].
///
/// Construct commands with [`CommandSpec::new`]. The name must be one word and
/// cannot be `help` or `quit`, which are reserved built-ins.
pub struct CommandSpec {
    /// Command name users type or autocomplete.
    pub name: String,
    /// Short summary shown by the built-in `help` command.
    pub description: String,
    /// Arguments accepted by the command, in handler parameter order.
    pub arguments: Vec<Argument>,
    pub(crate) action: Box<CommandAction>,
    pub(crate) is_quit: bool,
    pub(crate) is_help: bool,
}

impl CommandSpec {
    /// Creates a command with a handler receiving validated argument strings.
    ///
    /// The handler must be `Send + Sync + 'static` because it is stored by the
    /// command interface.
    pub fn new<F>(
        name: impl Into<String>,
        description: impl Into<String>,
        arguments: Vec<Argument>,
        action: F,
    ) -> Self
    where
        F: Fn(&[String]) -> Result<(), String> + Send + Sync + 'static,
    {
        Self {
            name: name.into(),
            description: description.into(),
            arguments,
            action: Box::new(action),
            is_quit: false,
            is_help: false,
        }
    }
}

/// Registry and interactive runner for application commands.
///
/// Create it with [`CommandTui::new`], then call [`CommandTui::run`] from a
/// terminal application.
pub struct CommandTui {
    pub(crate) commands: Vec<CommandSpec>,
    pub(crate) help_commands: Vec<HelpCommand>,
}

pub(crate) struct HelpCommand {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) arguments: Vec<Argument>,
}

impl CommandTui {
    /// Validates and registers commands, adding the built-in `help` and `quit`
    /// commands.
    ///
    /// Returns an error for an empty registry, duplicate or invalid command
    /// names, reserved names, invalid arguments, or invalid defaults.
    pub fn new(mut commands: Vec<CommandSpec>) -> Result<Self, CommandConfigError> {
        if commands.is_empty() {
            return Err("at least one command is required".to_owned().into());
        }

        for (index, command) in commands.iter().enumerate() {
            if command.name.trim().is_empty() || command.name.chars().any(char::is_whitespace) {
                return Err(format!(
                    "command names must be non-empty single words: {:?}",
                    command.name
                )
                .into());
            }
            if command.name.eq_ignore_ascii_case("quit") {
                return Err("'quit' is a built-in command name".to_owned().into());
            }
            if command.name.eq_ignore_ascii_case("help") {
                return Err("'help' is a built-in command name".to_owned().into());
            }
            if commands[..index]
                .iter()
                .any(|previous| previous.name.eq_ignore_ascii_case(&command.name))
            {
                return Err(format!("duplicate command name: {}", command.name).into());
            }

            for (arg_index, argument) in command.arguments.iter().enumerate() {
                if argument.name.trim().is_empty() {
                    return Err(format!(
                        "argument names must not be empty (command {})",
                        command.name
                    )
                    .into());
                }
                if command.arguments[..arg_index]
                    .iter()
                    .any(|previous| previous.name == argument.name)
                {
                    return Err(format!(
                        "duplicate argument {} in command {}",
                        argument.name, command.name
                    )
                    .into());
                }
                if let ArgumentKind::Choice(choices) = &argument.kind
                    && (choices.is_empty() || choices.iter().any(|choice| choice.is_empty()))
                {
                    return Err(format!(
                        "choice argument {} in command {} needs non-empty choices",
                        argument.name, command.name
                    )
                    .into());
                }
                if let Some(default) = &argument.default {
                    argument.validate(default).map_err(|error| {
                        format!(
                            "invalid default for {} in command {}: {error}",
                            argument.name, command.name
                        )
                    })?;
                }
            }
        }

        let help_commands = help_commands(&commands);
        let action_help_commands = help_commands
            .iter()
            .map(|command| HelpCommand {
                name: command.name.clone(),
                description: command.description.clone(),
                arguments: command.arguments.clone(),
            })
            .collect::<Vec<_>>();
        commands.push(CommandSpec {
            name: "help".to_owned(),
            description: "List commands or show command details".to_owned(),
            arguments: vec![Argument::text("command", "Command to show details for").optional()],
            action: Box::new(move |arguments| {
                let help =
                    format_help(&action_help_commands, arguments.first().map(String::as_str))?;
                print!("{help}");
                Ok(())
            }),
            is_quit: false,
            is_help: true,
        });
        commands.push(CommandSpec {
            name: "quit".to_owned(),
            description: "Exit the command interface".to_owned(),
            arguments: Vec::new(),
            action: Box::new(|_| Ok(())),
            is_quit: true,
            is_help: false,
        });

        Ok(Self {
            commands,
            help_commands,
        })
    }

    /// Returns all registered commands whose names start with `input`,
    /// ignoring ASCII letter case.
    pub fn suggestions(&self, input: &str) -> Vec<&CommandSpec> {
        self.commands
            .iter()
            .filter(|command| {
                command
                    .name
                    .get(..input.len())
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(input))
            })
            .collect()
    }

    /// Returns the shortest registered command matching the first word of
    /// `input`.
    ///
    /// Registration order breaks ties between names of equal length.
    pub fn suggestion(&self, input: &str) -> Option<&CommandSpec> {
        let command_prefix = input.split_whitespace().next().unwrap_or_default();
        if command_prefix.is_empty() {
            return None;
        }
        self.suggestions(command_prefix)
            .into_iter()
            .min_by_key(|command| command.name.chars().count())
    }
}

pub(crate) fn help_commands(commands: &[CommandSpec]) -> Vec<HelpCommand> {
    let mut help_commands = commands
        .iter()
        .filter(|command| !command.is_help && !command.is_quit)
        .map(|command| HelpCommand {
            name: command.name.clone(),
            description: command.description.clone(),
            arguments: command.arguments.clone(),
        })
        .collect::<Vec<_>>();
    help_commands.push(HelpCommand {
        name: "help".to_owned(),
        description: "List commands or show command details".to_owned(),
        arguments: vec![Argument::text("command", "Command to show details for").optional()],
    });
    help_commands.push(HelpCommand {
        name: "quit".to_owned(),
        description: "Exit the command interface".to_owned(),
        arguments: Vec::new(),
    });
    help_commands
}

pub(crate) fn format_help(
    commands: &[HelpCommand],
    requested: Option<&str>,
) -> Result<String, String> {
    let Some(requested) = requested.filter(|requested| !requested.is_empty()) else {
        let mut help = String::from("Commands:\n");
        for command in commands {
            help.push_str(&format!("  {} - {}\n", command.name, command.description));
        }
        return Ok(help);
    };

    let command = help_command_match(commands, requested)?;

    let mut help = format!("{} - {}\n", command.name, command.description);
    if command.arguments.is_empty() {
        help.push_str("No arguments.\n");
        return Ok(help);
    }

    help.push_str("Arguments:\n");
    for argument in &command.arguments {
        let requirement = if argument.required {
            "required"
        } else {
            "optional"
        };
        help.push_str(&format!(
            "  {} ({}, {requirement})\n    {}\n",
            argument.name,
            argument.kind.label(),
            argument.description
        ));
        if let Some(default) = &argument.default {
            help.push_str(&format!("    Default: {default}\n"));
        }
        if let ArgumentKind::Choice(choices) = &argument.kind {
            help.push_str(&format!("    Choices: {}\n", choices.join(", ")));
        }
    }
    Ok(help)
}

fn help_command_match<'a>(
    commands: &'a [HelpCommand],
    requested: &str,
) -> Result<&'a HelpCommand, String> {
    let exact_matches = commands
        .iter()
        .filter(|command| command.name.eq_ignore_ascii_case(requested))
        .collect::<Vec<_>>();
    let matches = if exact_matches.is_empty() {
        commands
            .iter()
            .filter(|command| {
                command
                    .name
                    .get(..requested.len())
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(requested))
            })
            .collect::<Vec<_>>()
    } else {
        exact_matches
    };
    match matches.as_slice() {
        [command] => Ok(*command),
        [] => Err(format!("no command named {requested:?}")),
        _ => Err(format!(
            "{requested:?} matches multiple commands: {}",
            matches
                .iter()
                .map(|command| command.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

pub(crate) fn resolved_help_display(
    commands: &[HelpCommand],
    requested: Option<&str>,
) -> Option<String> {
    requested
        .filter(|requested| !requested.is_empty())
        .and_then(|requested| {
            help_command_match(commands, requested)
                .ok()
                .map(|command| command.name.clone())
        })
}
