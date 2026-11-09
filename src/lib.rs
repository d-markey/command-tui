use std::{
    error::Error,
    fmt, fs,
    io::{self, Stdout, Write},
    path::{MAIN_SEPARATOR, Path},
};

use crossterm::{
    cursor::{MoveTo, MoveToColumn, MoveUp, Show},
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    queue,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{self, Clear, ClearType, disable_raw_mode, enable_raw_mode},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub type CommandAction = dyn Fn(&[String]) -> Result<(), String> + Send + Sync;

#[derive(Debug)]
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

pub struct CommandSpec {
    pub name: String,
    pub description: String,
    pub arguments: Vec<Argument>,
    action: Box<CommandAction>,
    is_quit: bool,
    is_help: bool,
}

impl CommandSpec {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Argument {
    pub name: String,
    pub description: String,
    pub kind: ArgumentKind,
    pub required: bool,
    pub default: Option<String>,
}

impl Argument {
    pub fn text(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(name, description, ArgumentKind::Text)
    }

    pub fn integer(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(name, description, ArgumentKind::Integer)
    }

    pub fn boolean(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(name, description, ArgumentKind::Boolean)
    }

    pub fn choice(
        name: impl Into<String>,
        description: impl Into<String>,
        choices: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self::new(
            name,
            description,
            ArgumentKind::Choice(choices.into_iter().map(Into::into).collect()),
        )
    }

    pub fn path(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(name, description, ArgumentKind::Path)
    }

    pub fn file(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(name, description, ArgumentKind::File)
    }

    pub fn directory(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(name, description, ArgumentKind::Directory)
    }

    fn new(name: impl Into<String>, description: impl Into<String>, kind: ArgumentKind) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            kind,
            required: true,
            default: None,
        }
    }

    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    pub fn with_default(mut self, default: impl Into<String>) -> Self {
        self.default = Some(default.into());
        self
    }

    fn validate(&self, value: &str) -> Result<String, String> {
        let value = value.trim();
        if value.is_empty() {
            if let Some(default) = &self.default {
                if !default.trim().is_empty() {
                    return self.validate_value(default);
                }
            }
            if self.required {
                return Err(format!("{} is required", self.name));
            }
            return Ok(String::new());
        }

        self.validate_value(value)
    }

    fn validate_value(&self, value: &str) -> Result<String, String> {
        match &self.kind {
            ArgumentKind::Text => Ok(value.to_owned()),
            ArgumentKind::Integer => value
                .parse::<i64>()
                .map(|number| number.to_string())
                .map_err(|_| format!("{} must be an integer", self.name)),
            ArgumentKind::Boolean => match value.to_ascii_lowercase().as_str() {
                "true" => Ok("true".to_owned()),
                "false" => Ok("false".to_owned()),
                _ => Err(format!("{} must be true or false", self.name)),
            },
            ArgumentKind::Choice(choices) => choices
                .iter()
                .find(|choice| choice.eq_ignore_ascii_case(value))
                .cloned()
                .ok_or_else(|| format!("{} must be one of: {}", self.name, choices.join(", "))),
            ArgumentKind::Path => Ok(value.to_owned()),
            ArgumentKind::File => validate_path_kind(self, value, fs::Metadata::is_file, "file"),
            ArgumentKind::Directory => {
                validate_path_kind(self, value, fs::Metadata::is_dir, "directory")
            }
        }
    }
}

fn validate_path_kind(
    argument: &Argument,
    value: &str,
    matches_kind: fn(&fs::Metadata) -> bool,
    expected: &str,
) -> Result<String, String> {
    match fs::metadata(value) {
        Ok(metadata) if matches_kind(&metadata) => Ok(value.to_owned()),
        Ok(_) => Err(format!("{} must be a {expected}", argument.name)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(value.to_owned()),
        Err(error) => Err(format!("could not inspect {}: {error}", argument.name)),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgumentKind {
    Text,
    Integer,
    Boolean,
    Choice(Vec<String>),
    Path,
    File,
    Directory,
}

pub struct CommandTui {
    commands: Vec<CommandSpec>,
    help_commands: Vec<HelpCommand>,
}

#[derive(Default)]
struct CommandHistory {
    entries: Vec<String>,
    position: Option<usize>,
    draft: Option<String>,
}

impl CommandHistory {
    fn push(&mut self, command: String) {
        self.entries.push(command);
        self.reset_navigation();
    }

    fn previous(&mut self, current: &str) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        let position = match self.position {
            Some(0) => {
                self.position = None;
                return self.draft.clone();
            }
            Some(position) => position - 1,
            None => {
                self.draft = Some(current.to_owned());
                self.entries.len() - 1
            }
        };
        self.position = Some(position);
        Some(self.entries[position].clone())
    }

    fn next(&mut self) -> Option<String> {
        let Some(position) = self.position else {
            if self.entries.is_empty() {
                return None;
            }
            self.position = Some(0);
            return Some(self.entries[0].clone());
        };
        if position + 1 < self.entries.len() {
            let position = position + 1;
            self.position = Some(position);
            Some(self.entries[position].clone())
        } else {
            self.position = None;
            self.draft.take()
        }
    }

    fn reset_navigation(&mut self) {
        self.position = None;
        self.draft = None;
    }
}

struct HelpCommand {
    name: String,
    description: String,
    arguments: Vec<Argument>,
}

impl CommandTui {
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
                if let ArgumentKind::Choice(choices) = &argument.kind {
                    if choices.is_empty() || choices.iter().any(|choice| choice.is_empty()) {
                        return Err(format!(
                            "choice argument {} in command {} needs non-empty choices",
                            argument.name, command.name
                        )
                        .into());
                    }
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

    pub fn suggestion(&self, input: &str) -> Option<&CommandSpec> {
        let command_prefix = input.split_whitespace().next().unwrap_or_default();
        if command_prefix.is_empty() {
            return None;
        }
        self.suggestions(command_prefix)
            .into_iter()
            .min_by_key(|command| command.name.chars().count())
    }

    pub fn run(&self) -> io::Result<()> {
        let mut terminal = TerminalSession::enter()?;
        let mut query = String::new();
        let (_, mut frame_start) = crossterm::cursor::position()?;
        let mut input_row = draw_search(&mut io::stdout(), &query, None, frame_start)?;
        let mut input_error = None::<String>;
        let mut history = CommandHistory::default();

        loop {
            let suggestion = self.suggestion(&query);

            match event::read()? {
                Event::Key(key) if key.code == KeyCode::Esc => {
                    query.clear();
                    input_error = None;
                    history.reset_navigation();
                    queue!(
                        io::stdout(),
                        MoveTo(0, input_row),
                        Clear(ClearType::FromCursorDown)
                    )?;
                    input_row = draw_command_input(&mut io::stdout(), &query, None, input_row)?;
                }
                Event::Key(key) if key.code == KeyCode::Enter => {
                    let Some(command) = suggestion else {
                        input_error = Some("no matching command".to_owned());
                        input_row = draw_command_error(
                            &mut io::stdout(),
                            &query,
                            input_row,
                            input_error.as_deref(),
                        )?;
                        continue;
                    };
                    if command.is_quit {
                        queue!(io::stdout(), Print("\r\n\r\n"))?;
                        io::stdout().flush()?;
                        return Ok(());
                    }
                    let mut supplied_arguments = match parse_command_arguments(command, &query) {
                        Ok(arguments) => arguments,
                        Err(error) => {
                            query = display_command_line(&command.name, &query);
                            input_row = draw_command_input(
                                &mut io::stdout(),
                                &query,
                                self.suggestion(&query),
                                input_row,
                            )?;
                            input_error = Some(error);
                            input_row = draw_command_error(
                                &mut io::stdout(),
                                &query,
                                input_row,
                                input_error.as_deref(),
                            )?;
                            continue;
                        }
                    };
                    if command.is_help
                        && let Some(argument) = supplied_arguments.first_mut()
                        && argument.is_none()
                    {
                        *argument = Some(String::new());
                    }
                    queue!(
                        io::stdout(),
                        MoveTo(0, input_row),
                        Clear(ClearType::UntilNewLine),
                        Print(format!("> {}", display_command_line(&command.name, &query))),
                        Print("\r\n")
                    )?;
                    io::stdout().flush()?;
                    let display_arguments = if command.is_help {
                        supplied_arguments
                            .iter()
                            .map(|argument| {
                                resolved_help_display(&self.help_commands, argument.as_deref())
                            })
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    };
                    let history_entry = display_command_line(&command.name, &query);
                    let Some(arguments) = prompt_arguments(
                        command,
                        supplied_arguments,
                        &display_arguments,
                        &mut io::stdout(),
                    )?
                    else {
                        let (_, row) = crossterm::cursor::position()?;
                        frame_start = row;
                        query.clear();
                        input_error = None;
                        input_row = draw_search(&mut io::stdout(), &query, None, frame_start)?;
                        continue;
                    };
                    history.push(history_entry);
                    terminal.suspend()?;
                    let result = (command.action)(&arguments);
                    if let Err(error) = result {
                        eprintln!("{} failed: {error}", command.name);
                    }
                    println!();
                    terminal.resume()?;
                    let (_, row) = crossterm::cursor::position()?;
                    frame_start = row;
                    query.clear();
                    input_error = None;
                    input_row = draw_search(&mut io::stdout(), &query, None, frame_start)?;
                }
                Event::Key(key) if matches!(key.code, KeyCode::Up | KeyCode::Down) => {
                    let history_query = match key.code {
                        KeyCode::Up => history.previous(&query),
                        KeyCode::Down => history.next(),
                        _ => unreachable!(),
                    };
                    if let Some(history_query) = history_query {
                        query = history_query;
                        if input_error.take().is_some() {
                            queue!(
                                io::stdout(),
                                MoveTo(0, input_row.saturating_add(1)),
                                Clear(ClearType::FromCursorDown)
                            )?;
                        }
                        input_row = draw_command_input(
                            &mut io::stdout(),
                            &query,
                            self.suggestion(&query),
                            input_row,
                        )?;
                    }
                }
                Event::Key(key) => {
                    let changed = if key.code == KeyCode::Tab {
                        if let Some(completed) = complete_command_line_argument(&query, suggestion)?
                        {
                            query = completed;
                            true
                        } else {
                            handle_search_key(key, &mut query, suggestion)
                        }
                    } else {
                        handle_search_key(key, &mut query, suggestion)
                    };
                    if changed {
                        history.reset_navigation();
                        if input_error.take().is_some() {
                            queue!(
                                io::stdout(),
                                MoveTo(0, input_row.saturating_add(1)),
                                Clear(ClearType::FromCursorDown)
                            )?;
                        }
                        input_row = draw_command_input(
                            &mut io::stdout(),
                            &query,
                            self.suggestion(&query),
                            input_row,
                        )?;
                        continue;
                    }
                }
                _ => {}
            }
        }
    }
}

fn help_commands(commands: &[CommandSpec]) -> Vec<HelpCommand> {
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

fn format_help(commands: &[HelpCommand], requested: Option<&str>) -> Result<String, String> {
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
        [] => return Err(format!("no command named {requested:?}")),
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

fn resolved_help_display(commands: &[HelpCommand], requested: Option<&str>) -> Option<String> {
    requested
        .filter(|requested| !requested.is_empty())
        .and_then(|requested| {
            help_command_match(commands, requested)
                .ok()
                .map(|command| command.name.clone())
        })
}

fn handle_search_key(key: KeyEvent, query: &mut String, suggestion: Option<&CommandSpec>) -> bool {
    match key.code {
        KeyCode::Char(_)
            if key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) =>
        {
            false
        }
        KeyCode::Tab => accept_command_completion(query, suggestion),
        KeyCode::Char(' ')
            if command_name_and_arguments(query).1.is_empty() && suggestion.is_some() =>
        {
            accept_command_completion(query, suggestion)
        }
        KeyCode::Char(character) => {
            query.push(character);
            true
        }
        KeyCode::Backspace => {
            query.pop();
            true
        }
        _ => false,
    }
}

fn accept_command_completion(query: &mut String, suggestion: Option<&CommandSpec>) -> bool {
    let Some(command) = suggestion else {
        return false;
    };
    let leading_whitespace = query.len() - query.trim_start().len();
    let (typed_command, arguments) = command_name_and_arguments(query);
    if typed_command.is_empty() || !arguments.is_empty() {
        return false;
    }

    query.truncate(leading_whitespace);
    query.push_str(&command.name);
    query.push(' ');
    true
}

fn complete_command_line_argument(
    query: &str,
    command: Option<&CommandSpec>,
) -> io::Result<Option<String>> {
    let Some(command) = command else {
        return Ok(None);
    };
    let command_end = query
        .char_indices()
        .find_map(|(index, character)| character.is_whitespace().then_some(index))
        .unwrap_or(query.len());
    if command_end == query.len() {
        return Ok(None);
    }
    let arguments_start = query[command_end..]
        .char_indices()
        .find_map(|(index, character)| (!character.is_whitespace()).then_some(command_end + index))
        .unwrap_or(query.len());
    let arguments = &query[arguments_start..];
    let token_start = current_argument_token_start(arguments);
    let previous_tokens = tokenize_arguments(&arguments[..token_start])
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let mut supplied = vec![false; command.arguments.len()];
    let mut pending_named = None;
    for token in &previous_tokens {
        if let Some(index) = pending_named.take() {
            supplied[index] = true;
            continue;
        }
        if let Some((name, value)) = token.split_once(':')
            && let Some(index) = command
                .arguments
                .iter()
                .position(|argument| argument.name == name)
        {
            if value.is_empty() {
                pending_named = Some(index);
            } else {
                supplied[index] = true;
            }
        } else if let Some(index) = supplied.iter().position(|is_supplied| !is_supplied) {
            supplied[index] = true;
        }
    }

    let current_raw = &arguments[token_start..];
    let (argument_index, value_start) = if let Some(index) = pending_named {
        (index, token_start)
    } else if let Some((name, _value)) = current_raw.split_once(':')
        && let Some(index) = command
            .arguments
            .iter()
            .position(|argument| argument.name == name)
    {
        (index, token_start + name.len() + 1)
    } else if let Some(index) = supplied.iter().position(|is_supplied| !is_supplied) {
        (index, token_start)
    } else {
        return Ok(None);
    };

    let argument = &command.arguments[argument_index];
    let raw_value = &arguments[value_start..];
    let (quote_prefix, value_prefix) = raw_value
        .strip_prefix('"')
        .map_or(("", raw_value), |value| ("\"", value));
    let closed_quote = value_prefix.ends_with('"') && quote_prefix == "\"";
    let value_prefix = if closed_quote {
        &value_prefix[..value_prefix.len() - 1]
    } else {
        value_prefix
    };
    let completed = match &argument.kind {
        ArgumentKind::Path | ArgumentKind::File | ArgumentKind::Directory => {
            complete_path(&argument.kind, value_prefix)?
        }
        ArgumentKind::Choice(_) => choice_suggestion(&argument.kind, value_prefix)
            .filter(|choice| *choice != value_prefix)
            .map(str::to_owned),
        _ => None,
    };
    let Some(completed) = completed else {
        return Ok(None);
    };
    let replacement = format!(
        "{quote_prefix}{completed}{}",
        if closed_quote { "\"" } else { "" }
    );
    let replace_start = arguments_start + value_start;
    let replace_end = query.len();
    let mut result = String::with_capacity(query.len() + replacement.len());
    result.push_str(&query[..replace_start]);
    result.push_str(&replacement);
    result.push_str(&query[replace_end..]);
    Ok(Some(result))
}

fn current_argument_token_start(arguments: &str) -> usize {
    let mut in_quotes = false;
    let mut token_start = 0;
    let mut characters = arguments.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        match character {
            '"' if in_quotes && characters.peek().is_some_and(|(_, next)| *next == '"') => {
                characters.next();
            }
            '"' => in_quotes = !in_quotes,
            character if character.is_whitespace() && !in_quotes => {
                token_start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    token_start
}

fn parse_command_arguments(
    command: &CommandSpec,
    line: &str,
) -> Result<Vec<Option<String>>, String> {
    let mut supplied = vec![None; command.arguments.len()];
    let (_, argument_text) = command_name_and_arguments(line);
    let tokens = tokenize_arguments(argument_text)?;
    let mut tokens = tokens.iter().map(String::as_str);

    while let Some(token) = tokens.next() {
        if let Some((name, value)) = token.split_once(':') {
            if let Some(index) = command
                .arguments
                .iter()
                .position(|argument| argument.name == name)
            {
                if supplied[index].is_some() {
                    return Err(format!("{} was supplied more than once", name));
                }
                let value = if value.is_empty() {
                    tokens
                        .next()
                        .ok_or_else(|| format!("missing value for {name}"))?
                } else {
                    value
                };
                supplied[index] = Some(resolve_argument_value(&command.arguments[index], value)?);
                continue;
            }
            if token.ends_with(':') {
                return Err(format!("unknown argument: {name}"));
            }
        }
        let Some(index) = supplied.iter().position(Option::is_none) else {
            return Err(format!("too many arguments; unexpected value: {token}"));
        };
        supplied[index] = Some(resolve_argument_value(&command.arguments[index], token)?);
    }
    Ok(supplied)
}

fn tokenize_arguments(input: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut in_quotes = false;
    let mut token_started = false;
    let mut characters = input.chars().peekable();

    while let Some(character) = characters.next() {
        match character {
            '"' if in_quotes && characters.peek() == Some(&'"') => {
                characters.next();
                token.push('"');
                token_started = true;
            }
            '"' => {
                in_quotes = !in_quotes;
                token_started = true;
            }
            character if character.is_whitespace() && !in_quotes => {
                if token_started {
                    tokens.push(std::mem::take(&mut token));
                    token_started = false;
                }
            }
            character => {
                token.push(character);
                token_started = true;
            }
        }
    }

    if in_quotes {
        return Err("unclosed double quote in command arguments".to_owned());
    }
    if token_started {
        tokens.push(token);
    }
    Ok(tokens)
}

fn command_name_and_arguments(line: &str) -> (&str, &str) {
    let trimmed = line.trim_start();
    let end = trimmed
        .char_indices()
        .find_map(|(index, character)| character.is_whitespace().then_some(index))
        .unwrap_or(trimmed.len());
    (&trimmed[..end], trimmed[end..].trim())
}

fn display_command_line(command_name: &str, input: &str) -> String {
    let (_, arguments) = command_name_and_arguments(input);
    if arguments.is_empty() {
        command_name.to_owned()
    } else {
        format!("{command_name} {arguments}")
    }
}

fn resolve_argument_value(argument: &Argument, input: &str) -> Result<String, String> {
    if let ArgumentKind::Choice(choices) = &argument.kind {
        if let Some(exact) = choices
            .iter()
            .find(|choice| choice.eq_ignore_ascii_case(input))
        {
            return Ok(exact.clone());
        }
        let candidates = choices
            .iter()
            .filter(|choice| completion_suffix(choice, input).is_some())
            .collect::<Vec<_>>();
        return match candidates.as_slice() {
            [choice] => Ok((*choice).clone()),
            [] => Err(format!(
                "{}: no choices match {:?}; choose one of: {}",
                argument.name,
                input,
                choices.join(", ")
            )),
            _ => Err(format!(
                "{}: {:?} matches multiple choices: {}",
                argument.name,
                input,
                candidates
                    .iter()
                    .map(|choice| choice.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        };
    }
    argument.validate(input)
}

fn complete_path(kind: &ArgumentKind, input: &str) -> io::Result<Option<String>> {
    let separator_index = input.rfind(|character| character == '/' || character == MAIN_SEPARATOR);
    let (directory, prefix) = match separator_index {
        Some(index) => (&input[..=index], &input[index + 1..]),
        None => ("", input),
    };
    let directory_path = if directory.is_empty() {
        Path::new(".")
    } else {
        Path::new(directory)
    };
    let entries = match fs::read_dir(directory_path) {
        Ok(entries) => entries,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };

    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !name.starts_with(prefix) {
            continue;
        }
        let metadata = entry.metadata()?;
        let is_directory = metadata.is_dir();
        let matches_kind = match kind {
            ArgumentKind::Path => is_directory || metadata.is_file(),
            ArgumentKind::File => metadata.is_file(),
            ArgumentKind::Directory => is_directory,
            _ => return Ok(None),
        };
        if matches_kind {
            candidates.push((name, is_directory));
        }
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));

    let Some((first, _)) = candidates.first() else {
        return Ok(None);
    };
    let mut common_prefix = first.clone();
    for (name, _) in candidates.iter().skip(1) {
        let shared_length = common_prefix
            .chars()
            .zip(name.chars())
            .take_while(|(left, right)| left == right)
            .map(|(character, _)| character.len_utf8())
            .sum();
        common_prefix.truncate(shared_length);
    }
    if candidates.len() == 1 && candidates[0].1 && !common_prefix.ends_with(MAIN_SEPARATOR) {
        common_prefix.push(MAIN_SEPARATOR);
    }

    let completed = format!("{directory}{common_prefix}");
    Ok((completed != input).then_some(completed))
}

fn print_completed_argument(stdout: &mut Stdout, name: &str, value: &str) -> io::Result<()> {
    queue!(stdout, Print(format!("   * {name}: {value}\r\n")))?;
    stdout.flush()
}

fn prompt_arguments(
    command: &CommandSpec,
    supplied: Vec<Option<String>>,
    display_values: &[Option<String>],
    stdout: &mut Stdout,
) -> io::Result<Option<Vec<String>>> {
    let mut values = Vec::with_capacity(command.arguments.len());
    for (index, (argument, supplied_value)) in command.arguments.iter().zip(supplied).enumerate() {
        if let Some(value) = supplied_value {
            let display_value = display_values
                .get(index)
                .and_then(Option::as_deref)
                .unwrap_or(&value);
            print_completed_argument(stdout, &argument.name, display_value)?;
            values.push(value);
        } else {
            let initial_value = argument.default.clone().unwrap_or_default();
            let Some(value) = prompt_argument(argument, initial_value, stdout)? else {
                return Ok(None);
            };
            values.push(value);
        };
    }
    Ok(Some(values))
}

fn prompt_argument(
    argument: &Argument,
    mut input: String,
    stdout: &mut Stdout,
) -> io::Result<Option<String>> {
    let prefix = format!("   * {}: ", argument.name);
    let is_choice = matches!(argument.kind, ArgumentKind::Choice(_));
    let mut error = None;
    let mut input_row =
        draw_argument_prompt(stdout, argument, &prefix, &input, error.as_deref(), None)?;
    loop {
        match event::read()? {
            Event::Key(key) if key.code == KeyCode::Esc => {
                clear_argument_details(stdout, input_row)?;
                return Ok(None);
            }
            Event::Key(key) if key.code == KeyCode::Enter => {
                match resolve_argument_value(argument, &input) {
                    Ok(value) => {
                        finish_argument_line(stdout, input_row, &prefix, &value)?;
                        return Ok(Some(value));
                    }
                    Err(message) => {
                        error = Some(message);
                        input_row = draw_argument_prompt(
                            stdout,
                            argument,
                            &prefix,
                            &input,
                            error.as_deref(),
                            Some(input_row),
                        )?;
                    }
                }
            }
            Event::Key(key) if key.code == KeyCode::Backspace => {
                if input.pop().is_some() {
                    if error.take().is_some() {
                        input_row = draw_argument_prompt(
                            stdout,
                            argument,
                            &prefix,
                            &input,
                            None,
                            Some(input_row),
                        )?;
                    } else if is_choice {
                        draw_argument_input_line(
                            stdout,
                            &prefix,
                            &input,
                            choice_suggestion(&argument.kind, &input),
                            input_row,
                        )?;
                    } else {
                        input_row = draw_argument_prompt(
                            stdout,
                            argument,
                            &prefix,
                            &input,
                            None,
                            Some(input_row),
                        )?;
                    }
                }
            }
            Event::Key(key) if key.code == KeyCode::Tab => {
                if error.take().is_some() {
                    input_row = draw_argument_prompt(
                        stdout,
                        argument,
                        &prefix,
                        &input,
                        None,
                        Some(input_row),
                    )?;
                }
                let completion = match &argument.kind {
                    ArgumentKind::Path | ArgumentKind::File | ArgumentKind::Directory => {
                        complete_path(&argument.kind, &input)?
                    }
                    ArgumentKind::Choice(_) => choice_suggestion(&argument.kind, &input)
                        .filter(|choice| *choice != input)
                        .map(str::to_owned),
                    _ => None,
                };
                if let Some(completion) = completion {
                    input = completion;
                    input_row = draw_argument_input_line(stdout, &prefix, &input, None, input_row)?;
                }
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char(character),
                modifiers,
                ..
            }) if !modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) =>
            {
                input.push(character);
                if error.take().is_some() {
                    input_row = draw_argument_prompt(
                        stdout,
                        argument,
                        &prefix,
                        &input,
                        None,
                        Some(input_row),
                    )?;
                } else if is_choice {
                    draw_argument_input_line(
                        stdout,
                        &prefix,
                        &input,
                        choice_suggestion(&argument.kind, &input),
                        input_row,
                    )?;
                } else {
                    queue!(stdout, Print(character))?;
                    stdout.flush()?;
                    let cursor_row = crossterm::cursor::position()?.1;
                    let (terminal_width, _) = terminal::size()?;
                    let prefix_width = UnicodeWidthStr::width(
                        fit_line(
                            &prefix,
                            usize::from(terminal_width).saturating_sub(1).max(1),
                        )
                        .as_str(),
                    );
                    let typed_width = UnicodeWidthStr::width(input.as_str());
                    let wrapped_rows = prefix_width.saturating_add(typed_width).saturating_sub(1)
                        / usize::from(terminal_width.max(1));
                    input_row = cursor_row.saturating_sub(wrapped_rows as u16);
                }
            }
            _ => {}
        }
    }
}

fn draw_search(
    stdout: &mut Stdout,
    query: &str,
    suggestion: Option<&CommandSpec>,
    frame_start: u16,
) -> io::Result<u16> {
    let (width, _) = terminal::size()?;
    let width = usize::from(width).max(1);
    let lines = vec![
        "Command".to_owned(),
        "Type a command; Enter to run the suggestion; Esc to clear.".to_owned(),
        String::new(),
    ];
    draw_lines(stdout, &lines, width, frame_start)?;

    let input_row = frame_start.saturating_add(lines.len().saturating_sub(1) as u16);
    draw_command_input(stdout, query, suggestion, input_row)
}

fn draw_command_input(
    stdout: &mut Stdout,
    query: &str,
    suggestion: Option<&CommandSpec>,
    input_row: u16,
) -> io::Result<u16> {
    let (width, height) = terminal::size()?;
    let width = usize::from(width).max(1);
    let input_row = input_row.min(height.saturating_sub(1));
    let line_width = width.saturating_sub(1);
    let input_width = line_width.saturating_sub(3);
    let visible_query = take_display_width_from_end(query, input_width);
    let (command_prefix, arguments) = command_name_and_arguments(query);
    let suffix = suggestion
        .filter(|_| arguments.is_empty())
        .and_then(|command| completion_suffix(&command.name, command_prefix))
        .unwrap_or_default();
    let suffix = take_display_width(
        suffix,
        input_width.saturating_sub(UnicodeWidthStr::width(visible_query.as_str())),
    );
    let cursor_column = 2usize
        .saturating_add(UnicodeWidthStr::width(visible_query.as_str()))
        .min(width.saturating_sub(1)) as u16;

    queue!(
        stdout,
        MoveTo(0, input_row),
        Clear(ClearType::UntilNewLine),
        Print("> "),
        Print(if arguments.is_empty() {
            visible_query
        } else {
            fit_line(query, input_width)
        }),
        SetForegroundColor(Color::DarkGrey),
        Print(suffix),
        ResetColor
    )?;
    let cursor_column = if arguments.is_empty() {
        cursor_column
    } else {
        2usize
            .saturating_add(UnicodeWidthStr::width(
                fit_line(query, input_width).as_str(),
            ))
            .min(width.saturating_sub(1)) as u16
    };
    queue!(stdout, MoveTo(cursor_column, input_row))?;
    stdout.flush()?;
    Ok(input_row)
}

fn draw_command_error(
    stdout: &mut Stdout,
    query: &str,
    input_row: u16,
    error: Option<&str>,
) -> io::Result<u16> {
    let (width, _) = terminal::size()?;
    let width = usize::from(width).max(1);
    let error_lines = error
        .map(|error| wrap_text(error, width.saturating_sub(1).max(1)))
        .unwrap_or_default();
    queue!(
        stdout,
        MoveTo(0, input_row),
        Print("\r\n"),
        Clear(ClearType::FromCursorDown)
    )?;
    for (index, line) in error_lines.iter().enumerate() {
        if index > 0 {
            queue!(stdout, Print("\r\n"))?;
        }
        queue!(
            stdout,
            Print(fit_line(line, width.saturating_sub(1).max(1)))
        )?;
    }

    let expected_end_row = usize::from(input_row).saturating_add(error_lines.len());
    let actual_end_row = usize::from(crossterm::cursor::position()?.1);
    let scrolled_rows = expected_end_row.saturating_sub(actual_end_row);
    let input_row = usize::from(input_row).saturating_sub(scrolled_rows) as u16;
    let cursor_column = command_cursor_column(query, width);
    queue!(stdout, MoveTo(cursor_column, input_row))?;
    stdout.flush()?;
    Ok(input_row)
}

fn command_cursor_column(query: &str, width: usize) -> u16 {
    let input_width = width.saturating_sub(4);
    let visible = fit_line(query, input_width);
    2usize
        .saturating_add(UnicodeWidthStr::width(visible.as_str()))
        .min(width.saturating_sub(1)) as u16
}

fn completion_suffix<'a>(command_name: &'a str, query: &str) -> Option<&'a str> {
    let prefix = command_name.get(..query.len())?;
    if prefix.eq_ignore_ascii_case(query) {
        command_name.get(query.len()..)
    } else {
        None
    }
}

fn choice_suggestion<'a>(kind: &'a ArgumentKind, input: &str) -> Option<&'a str> {
    if input.is_empty() {
        return None;
    }
    let ArgumentKind::Choice(choices) = kind else {
        return None;
    };
    if let Some(choice) = choices
        .iter()
        .find(|choice| choice.eq_ignore_ascii_case(input))
    {
        return Some(choice);
    }
    let mut matches = choices
        .iter()
        .filter(|choice| completion_suffix(choice, input).is_some());
    let choice = matches.next()?;
    matches.next().is_none().then_some(choice.as_str())
}

fn draw_argument_prompt(
    stdout: &mut Stdout,
    argument: &Argument,
    prefix: &str,
    input: &str,
    error: Option<&str>,
    row: Option<u16>,
) -> io::Result<u16> {
    let (width, height) = terminal::size()?;
    let width = usize::from(width).max(1);
    let start_row = if let Some(row) = row {
        row
    } else {
        crossterm::cursor::position()?.1
    };
    if row.is_some() {
        queue!(
            stdout,
            MoveTo(0, start_row),
            Clear(ClearType::FromCursorDown)
        )?;
    }

    let input_row = draw_argument_input_line(
        stdout,
        prefix,
        input,
        choice_suggestion(&argument.kind, input),
        start_row,
    )?;

    let mut details = wrap_text(&argument.description, width.saturating_sub(1).max(1));
    details.push(format!("Type: {}", argument.kind.label()));
    if !argument.required {
        details.push("Optional; press Enter to leave empty.".to_owned());
    }
    if let Some(default) = &argument.default {
        details.extend(wrap_text(&format!("Default: {default}"), width));
    }
    if let ArgumentKind::Choice(choices) = &argument.kind {
        details.extend(wrap_text(
            &format!("Choices: {}", choices.join(", ")),
            width.saturating_sub(1).max(1),
        ));
    }
    if let Some(error) = error {
        details.extend(wrap_text(
            &format!("Error: {error}"),
            width.saturating_sub(1).max(1),
        ));
    }

    queue!(stdout, MoveTo(0, start_row), Print("\r\n"))?;
    for line in &details {
        queue!(
            stdout,
            Print(fit_line(line, width.saturating_sub(1).max(1))),
            Print("\r\n")
        )?;
    }
    let end_row = usize::from(crossterm::cursor::position()?.1);
    let expected_end_row = usize::from(start_row).saturating_add(details.len() + 1);
    let scrolled_rows = expected_end_row.saturating_sub(end_row);
    let input_row = usize::from(input_row).saturating_sub(scrolled_rows);
    let input_row = input_row.min(usize::from(height.saturating_sub(1))) as u16;
    draw_argument_input_line(
        stdout,
        prefix,
        input,
        choice_suggestion(&argument.kind, input),
        input_row,
    )?;
    stdout.flush()?;
    Ok(input_row)
}

fn draw_argument_input_line(
    stdout: &mut Stdout,
    prefix: &str,
    input: &str,
    suggestion: Option<&str>,
    row: u16,
) -> io::Result<u16> {
    let (width, _) = terminal::size()?;
    let width = usize::from(width).max(1);
    let line_width = width.saturating_sub(1).max(1);
    let visible_prefix = fit_line(prefix, line_width);
    let typed_width = UnicodeWidthStr::width(visible_prefix.as_str());
    let available = line_width.saturating_sub(typed_width);
    let visible_input = take_display_width_from_end(input, available);
    let input_width = UnicodeWidthStr::width(visible_input.as_str());
    let suffix = suggestion
        .and_then(|choice| completion_suffix(choice, input))
        .unwrap_or_default();
    let suffix = take_display_width(suffix, available.saturating_sub(input_width));

    queue!(
        stdout,
        MoveTo(0, row),
        Clear(ClearType::UntilNewLine),
        Print(&visible_prefix),
        Print(&visible_input)
    )?;
    let (cursor_column, cursor_row) = crossterm::cursor::position()?;
    if !suffix.is_empty() {
        queue!(
            stdout,
            SetForegroundColor(Color::DarkGrey),
            Print(suffix),
            ResetColor
        )?;
    }
    queue!(stdout, MoveTo(cursor_column, cursor_row))?;
    stdout.flush()?;
    Ok(row)
}

fn finish_argument_line(
    stdout: &mut Stdout,
    row: u16,
    prefix: &str,
    value: &str,
) -> io::Result<()> {
    let (width, _) = terminal::size()?;
    let width = usize::from(width).max(1);
    queue!(
        stdout,
        MoveTo(0, row),
        Clear(ClearType::UntilNewLine),
        Print(fit_line(
            &format!("{prefix}{value}"),
            width.saturating_sub(1).max(1)
        )),
        MoveTo(0, row.saturating_add(1)),
        Clear(ClearType::FromCursorDown),
        MoveTo(0, row),
        Print("\r\n")
    )?;
    stdout.flush()?;
    Ok(())
}

fn clear_argument_details(stdout: &mut Stdout, row: u16) -> io::Result<()> {
    queue!(
        stdout,
        MoveTo(0, row.saturating_add(1)),
        Clear(ClearType::FromCursorDown),
        MoveTo(0, row),
        Print("\r\n")
    )?;
    stdout.flush()
}

fn draw_lines(
    stdout: &mut Stdout,
    lines: &[String],
    width: usize,
    frame_start: u16,
) -> io::Result<()> {
    queue!(
        stdout,
        MoveTo(0, frame_start),
        Clear(ClearType::FromCursorDown)
    )?;
    for line in lines {
        queue!(
            stdout,
            Print(fit_line(line, width.saturating_sub(1).max(1))),
            Print("\r\n")
        )?;
    }
    queue!(stdout, MoveUp(1), MoveToColumn(0))?;
    stdout.flush()
}

fn fit_line(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_owned();
    }
    format!("{}…", take_display_width(text, width - 1))
}

fn take_display_width(text: &str, max_width: usize) -> String {
    let mut result = String::new();
    let mut width = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width + character_width > max_width {
            break;
        }
        result.push(character);
        width += character_width;
    }
    result
}

fn take_display_width_from_end(text: &str, max_width: usize) -> String {
    let mut result = String::new();
    let mut width = 0;
    for character in text.chars().rev() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width + character_width > max_width {
            break;
        }
        result.push(character);
        width += character_width;
    }
    result.chars().rev().collect()
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut line_width = 0;
        for word in paragraph.split_whitespace() {
            let word_width = UnicodeWidthStr::width(word);
            let separator_width = usize::from(!line.is_empty());
            if line_width + separator_width + word_width <= width {
                if separator_width > 0 {
                    line.push(' ');
                    line_width += 1;
                }
                line.push_str(word);
                line_width += word_width;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            if word_width <= width {
                line.push_str(word);
                line_width = word_width;
            } else {
                let mut fragment = String::new();
                let mut fragment_width = 0;
                for character in word.chars() {
                    let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
                    if fragment_width + character_width > width {
                        lines.push(std::mem::take(&mut fragment));
                        fragment_width = 0;
                    }
                    fragment.push(character);
                    fragment_width += character_width;
                }
                line = fragment;
                line_width = fragment_width;
            }
        }
        if !line.is_empty() || paragraph.is_empty() {
            lines.push(line);
        }
    }
    lines
}

impl ArgumentKind {
    fn label(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Integer => "integer",
            Self::Boolean => "true or false",
            Self::Choice(_) => "choice",
            Self::Path => "path",
            Self::File => "file",
            Self::Directory => "directory",
        }
    }
}

struct TerminalSession {
    active: bool,
}

impl TerminalSession {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = queue!(stdout, Show) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        if let Err(error) = stdout.flush() {
            let _ = disable_raw_mode();
            return Err(error);
        }
        Ok(Self { active: true })
    }

    fn suspend(&mut self) -> io::Result<()> {
        disable_raw_mode()?;
        self.active = false;
        Ok(())
    }

    fn resume(&mut self) -> io::Result<()> {
        enable_raw_mode()?;
        self.active = true;
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
        }
        let _ = queue!(io::stdout(), Show);
        let _ = io::stdout().flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tui() -> CommandTui {
        CommandTui::new(vec![
            CommandSpec::new("deploy", "Deploy an app", vec![], |_| Ok(())),
            CommandSpec::new("delete", "Delete an app", vec![], |_| Ok(())),
            CommandSpec::new("status", "Show status", vec![], |_| Ok(())),
        ])
        .unwrap()
    }

    #[test]
    fn command_history_cycles_and_restores_the_current_draft() {
        let mut history = CommandHistory::default();
        assert_eq!(history.previous("draft"), None);

        history.push("greet Ada".to_owned());
        history.push("check_path /home".to_owned());
        assert_eq!(
            history.previous("current"),
            Some("check_path /home".to_owned())
        );
        assert_eq!(history.previous("ignored"), Some("greet Ada".to_owned()));
        assert_eq!(history.previous("ignored"), Some("current".to_owned()));
        assert_eq!(history.next(), Some("greet Ada".to_owned()));
        assert_eq!(history.next(), Some("check_path /home".to_owned()));
        assert_eq!(history.next(), Some("current".to_owned()));
        assert_eq!(history.next(), Some("greet Ada".to_owned()));
    }

    #[test]
    fn editing_after_history_navigation_starts_a_new_cycle() {
        let mut history = CommandHistory::default();
        history.push("status".to_owned());
        history.push("greet Ada".to_owned());
        assert_eq!(history.previous("draft"), Some("greet Ada".to_owned()));
        history.reset_navigation();
        assert_eq!(history.previous("edited"), Some("greet Ada".to_owned()));
        assert_eq!(history.next(), Some("edited".to_owned()));
    }

    #[test]
    fn history_resolves_the_command_name_but_preserves_typed_arguments() {
        assert_eq!(
            display_command_line("environment", "env prod true"),
            "environment prod true"
        );
        assert_eq!(
            display_command_line("greet", r#"gre message: "say ""hi""""#),
            r#"greet message: "say ""hi""""#
        );
    }

    #[test]
    fn suggestions_match_case_insensitive_prefixes() {
        let tui = sample_tui();
        let names = tui
            .suggestions("DE")
            .iter()
            .map(|command| command.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["deploy", "delete"]);
    }

    #[test]
    fn single_suggestion_is_the_shortest_matching_completion() {
        let tui = CommandTui::new(vec![
            CommandSpec::new("goodbye", "", vec![], |_| Ok(())),
            CommandSpec::new("go", "", vec![], |_| Ok(())),
            CommandSpec::new("grunt", "", vec![], |_| Ok(())),
        ])
        .unwrap();

        assert_eq!(tui.suggestion("g").unwrap().name, "go");
        assert_eq!(tui.suggestion("GRU").unwrap().name, "grunt");
        assert!(tui.suggestion("xyz").is_none());
        assert!(tui.suggestion("").is_none());
    }

    #[test]
    fn completion_suffix_contains_only_the_untyped_part() {
        assert_eq!(completion_suffix("grunt", "gru"), Some("nt"));
        assert_eq!(completion_suffix("grunt", "GRU"), Some("nt"));
        assert_eq!(completion_suffix("grunt", "grunts"), None);
        assert_eq!(completion_suffix("grunt", ""), Some("grunt"));
    }

    #[test]
    fn tab_or_space_accepts_command_completion_and_starts_arguments() {
        let tui = sample_tui();
        let command = tui.suggestion("del").unwrap();

        let mut tab_query = "del".to_owned();
        assert!(handle_search_key(
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &mut tab_query,
            Some(command),
        ));
        assert_eq!(tab_query, "delete ");

        let mut space_query = "del".to_owned();
        assert!(handle_search_key(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &mut space_query,
            Some(command),
        ));
        assert_eq!(space_query, "delete ");
        assert!(handle_search_key(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &mut space_query,
            Some(command),
        ));
        assert_eq!(space_query, "delete ");
    }

    #[test]
    fn choice_arguments_autocomplete_to_the_shortest_matching_choice() {
        let kind = ArgumentKind::Choice(vec![
            "development".to_owned(),
            "dev".to_owned(),
            "production".to_owned(),
        ]);
        assert_eq!(choice_suggestion(&kind, "d"), None);
        assert_eq!(choice_suggestion(&kind, "DEV"), Some("dev"));
        assert_eq!(choice_suggestion(&kind, "prod"), Some("production"));
        assert_eq!(choice_suggestion(&kind, "x"), None);
        assert_eq!(choice_suggestion(&kind, ""), None);
    }

    #[test]
    fn command_line_arguments_accept_positional_and_named_values() {
        let command = CommandSpec::new(
            "greet",
            "",
            vec![
                Argument::text("name", ""),
                Argument::choice(
                    "environment",
                    "",
                    ["development", "production", "preproduction"],
                ),
            ],
            |_| Ok(()),
        );

        assert_eq!(
            parse_command_arguments(&command, "greet david dev").unwrap(),
            [Some("david".to_owned()), Some("development".to_owned())]
        );
        assert_eq!(
            parse_command_arguments(&command, "greet environment: prod name: david").unwrap(),
            [Some("david".to_owned()), Some("production".to_owned())]
        );
        assert_eq!(
            parse_command_arguments(&command, "greet name:david").unwrap(),
            [Some("david".to_owned()), None]
        );
        assert!(
            parse_command_arguments(&command, "greet environment: p")
                .unwrap_err()
                .contains("multiple choices")
        );
        assert!(
            parse_command_arguments(&command, "greet unknown: value")
                .unwrap_err()
                .contains("unknown argument")
        );
    }

    #[test]
    fn command_line_arguments_support_quoted_values_and_escaped_quotes() {
        let command = CommandSpec::new(
            "greet",
            "",
            vec![Argument::text("message", ""), Argument::text("name", "")],
            |_| Ok(()),
        );

        assert_eq!(
            parse_command_arguments(&command, r#"greet "hello kind person" david"#).unwrap(),
            [
                Some("hello kind person".to_owned()),
                Some("david".to_owned())
            ]
        );
        assert_eq!(
            parse_command_arguments(&command, r#"greet "say ""hello""" name: "Ada Lovelace""#)
                .unwrap(),
            [
                Some(r#"say "hello""#.to_owned()),
                Some("Ada Lovelace".to_owned())
            ]
        );
        assert_eq!(
            parse_command_arguments(&command, r#"greet message:"quoted value" name:david"#)
                .unwrap(),
            [Some("quoted value".to_owned()), Some("david".to_owned())]
        );
    }

    #[test]
    fn command_line_arguments_reject_unclosed_quotes() {
        let command =
            CommandSpec::new("greet", "", vec![Argument::text("message", "")], |_| Ok(()));
        assert!(
            parse_command_arguments(&command, r#"greet "unfinished"#)
                .unwrap_err()
                .contains("unclosed double quote")
        );
    }

    #[test]
    fn accepted_command_echo_includes_resolved_name_and_typed_arguments() {
        assert_eq!(display_command_line("greet", "gre david"), "greet david");
        assert_eq!(
            display_command_line("greet", "greet name: david"),
            "greet name: david"
        );
        assert_eq!(display_command_line("greet", "gre"), "greet");
    }

    #[test]
    fn invalid_argument_keeps_full_command_and_supplied_arguments() {
        assert_eq!(
            display_command_line("environment", "env target: pro"),
            "environment target: pro"
        );
    }

    #[test]
    fn argument_validation_normalizes_typed_values() {
        let integer = Argument::integer("count", "How many?");
        let boolean = Argument::boolean("force", "Force operation?");
        let choice = Argument::choice("region", "Where?", ["east", "west"]);
        assert_eq!(integer.validate(" 42 ").unwrap(), "42");
        assert_eq!(boolean.validate("TRUE").unwrap(), "true");
        assert_eq!(choice.validate("WEST").unwrap(), "west");
        assert!(integer.validate("many").is_err());
        assert!(boolean.validate("yes").is_err());
        assert!(choice.validate("north").is_err());
    }

    #[test]
    fn path_arguments_allow_missing_entries_and_check_existing_kinds() {
        let test_dir = temporary_path_test_dir();
        fs::create_dir_all(&test_dir).unwrap();
        let file_path = test_dir.join("file.txt");
        let directory_path = test_dir.join("folder");
        fs::write(&file_path, "contents").unwrap();
        fs::create_dir(&directory_path).unwrap();

        assert_eq!(
            Argument::path("location", "").validate(file_path.to_str().unwrap()),
            Ok(file_path.to_str().unwrap().to_owned())
        );
        assert_eq!(
            Argument::path("location", "").validate(directory_path.to_str().unwrap()),
            Ok(directory_path.to_str().unwrap().to_owned())
        );
        assert!(
            Argument::file("location", "")
                .validate(directory_path.to_str().unwrap())
                .is_err()
        );
        assert!(
            Argument::directory("location", "")
                .validate(file_path.to_str().unwrap())
                .is_err()
        );
        let missing_path = test_dir.join("missing");
        assert_eq!(
            Argument::file("location", "").validate(missing_path.to_str().unwrap()),
            Ok(missing_path.to_str().unwrap().to_owned())
        );
        assert_eq!(
            Argument::directory("location", "").validate(missing_path.to_str().unwrap()),
            Ok(missing_path.to_str().unwrap().to_owned())
        );
        fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn path_tab_completion_filters_and_completes_directories() {
        let test_dir = temporary_path_test_dir();
        fs::create_dir_all(&test_dir).unwrap();
        fs::write(test_dir.join("notes.txt"), "contents").unwrap();
        fs::write(test_dir.join("notes.md"), "contents").unwrap();
        fs::create_dir(test_dir.join("nested")).unwrap();
        let prefix = format!("{}/", test_dir.display());

        assert_eq!(
            complete_path(&ArgumentKind::Directory, &format!("{prefix}nes")).unwrap(),
            Some(format!("{prefix}nested{MAIN_SEPARATOR}"))
        );
        assert_eq!(
            complete_path(&ArgumentKind::File, &format!("{prefix}notes.t")).unwrap(),
            Some(format!("{prefix}notes.txt"))
        );
        assert_eq!(
            complete_path(&ArgumentKind::Path, &format!("{prefix}notes")).unwrap(),
            Some(format!("{prefix}notes."))
        );
        fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn path_tab_completion_works_for_absolute_command_line_arguments() {
        let test_dir = temporary_path_test_dir();
        fs::create_dir_all(&test_dir).unwrap();
        fs::create_dir(test_dir.join("nested")).unwrap();
        let prefix = format!("{}/nes", test_dir.display());
        let tui = CommandTui::new(vec![CommandSpec::new(
            "check_path",
            "",
            vec![Argument::path("path", "")],
            |_| Ok(()),
        )])
        .unwrap();
        let command = tui.suggestion("check_path").unwrap();

        assert_eq!(
            complete_command_line_argument(&format!("check_path {prefix}"), Some(command)).unwrap(),
            Some(format!(
                "check_path {}/nested{MAIN_SEPARATOR}",
                test_dir.display()
            ))
        );
        assert_eq!(
            complete_command_line_argument(&format!("check_path path: {prefix}"), Some(command))
                .unwrap(),
            Some(format!(
                "check_path path: {}/nested{MAIN_SEPARATOR}",
                test_dir.display()
            ))
        );
        fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn choice_tab_completion_works_for_command_line_arguments() {
        let tui = CommandTui::new(vec![CommandSpec::new(
            "environment",
            "",
            vec![Argument::choice(
                "target",
                "",
                ["development", "production"],
            )],
            |_| Ok(()),
        )])
        .unwrap();
        let command = tui.suggestion("environment").unwrap();

        assert_eq!(
            complete_command_line_argument("environment pr", Some(command)).unwrap(),
            Some("environment production".to_owned())
        );
        assert_eq!(
            complete_command_line_argument("environment target: pr", Some(command)).unwrap(),
            Some("environment target: production".to_owned())
        );
        assert_eq!(
            complete_command_line_argument("environment p", Some(command)).unwrap(),
            Some("environment production".to_owned())
        );
    }

    fn temporary_path_test_dir() -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("command-tui-test-{}-{unique}", std::process::id()))
    }

    #[test]
    fn optional_and_default_arguments_are_supported() {
        let optional = Argument::text("note", "Optional note").optional();
        let defaulted = Argument::integer("count", "How many?").with_default("3");
        assert_eq!(optional.validate("").unwrap(), "");
        assert_eq!(defaulted.validate("").unwrap(), "3");
    }

    #[test]
    fn invalid_command_registries_are_rejected() {
        let duplicate = CommandTui::new(vec![
            CommandSpec::new("status", "", vec![], |_| Ok(())),
            CommandSpec::new("STATUS", "", vec![], |_| Ok(())),
        ]);
        assert!(duplicate.is_err());
        assert!(CommandTui::new(vec![]).is_err());
        assert!(CommandTui::new(vec![CommandSpec::new("quit", "", vec![], |_| Ok(()))]).is_err());
        assert!(CommandTui::new(vec![CommandSpec::new("help", "", vec![], |_| Ok(()))]).is_err());
    }

    #[test]
    fn help_is_available_as_a_builtin_command() {
        let tui = sample_tui();
        let help = tui.suggestions("hel");
        assert_eq!(help.len(), 1);
        assert_eq!(help[0].name, "help");
        assert_eq!(help[0].arguments.len(), 1);
        assert!(!help[0].arguments[0].required);
        assert!(!help[0].is_quit);
    }

    #[test]
    fn help_without_a_command_lists_names_but_not_argument_details() {
        let tui = CommandTui::new(vec![CommandSpec::new(
            "greet",
            "Print a greeting",
            vec![
                Argument::text("name", "Name to greet"),
                Argument::text("style", "Greeting style").optional(),
            ],
            |_| Ok(()),
        )])
        .unwrap();
        let help = format_help(&help_commands(&tui.commands), None).unwrap();
        assert!(help.contains("greet - Print a greeting"));
        assert!(!help.contains("name"));
        assert!(help.contains("help - List commands or show command details"));
        assert!(help.contains("quit - Exit the command interface"));
    }

    #[test]
    fn help_for_a_command_shows_argument_details() {
        let tui = CommandTui::new(vec![
            CommandSpec::new(
                "deploy",
                "Deploy an app",
                vec![
                    Argument::choice(
                        "environment",
                        "Where to deploy?",
                        ["development", "production"],
                    ),
                    Argument::boolean("dry-run", "Only show what would happen?")
                        .optional()
                        .with_default("true"),
                ],
                |_| Ok(()),
            ),
            CommandSpec::new("delete", "Delete an app", vec![], |_| Ok(())),
        ])
        .unwrap();
        let help = help_commands(&tui.commands);

        let details = format_help(&help, Some("DEPLOY")).unwrap();
        assert!(details.contains("deploy - Deploy an app"));
        assert!(details.contains("environment (choice, required)"));
        assert!(details.contains("Where to deploy?"));
        assert!(details.contains("Choices: development, production"));
        assert!(details.contains("dry-run (true or false, optional)"));
        assert!(details.contains("Default: true"));

        assert_eq!(
            resolved_help_display(&help, Some("he")).as_deref(),
            Some("help")
        );
        assert_eq!(display_command_line("help", "help he"), "help he");
        assert!(
            format_help(&help, Some("dep"))
                .unwrap()
                .contains("deploy -")
        );
        assert!(
            format_help(&help, Some("unknown"))
                .unwrap_err()
                .contains("no command named")
        );
        assert!(
            format_help(&help, Some("de"))
                .unwrap_err()
                .contains("matches multiple commands")
        );
    }

    #[test]
    fn quit_is_available_as_a_builtin_command() {
        let tui = sample_tui();
        let quit = tui.suggestions("qui");
        assert_eq!(quit.len(), 1);
        assert_eq!(quit[0].name, "quit");
        assert!(quit[0].arguments.is_empty());
        assert!(quit[0].is_quit);
    }

    #[test]
    fn layout_helpers_respect_terminal_display_width() {
        assert_eq!(fit_line("hello", 4), "hel…");
        assert_eq!(fit_line("界界界", 5), "界界…");
        assert_eq!(wrap_text("wide words wrap", 5), ["wide", "words", "wrap"]);
    }
}
