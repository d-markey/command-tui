use std::{
    fs, io,
    path::{MAIN_SEPARATOR, Path},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{
    argument::ArgumentKind,
    command::CommandSpec,
    parser::{command_name_and_arguments, tokenize_arguments},
};

pub(crate) fn handle_search_key(
    key: KeyEvent,
    query: &mut String,
    suggestion: Option<&CommandSpec>,
) -> bool {
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

pub(crate) fn complete_command_line_argument(
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

pub(crate) fn complete_path(kind: &ArgumentKind, input: &str) -> io::Result<Option<String>> {
    let separator_index = input.rfind(['/', MAIN_SEPARATOR]);
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

pub(crate) fn completion_suffix<'a>(command_name: &'a str, query: &str) -> Option<&'a str> {
    let prefix = command_name.get(..query.len())?;
    if prefix.eq_ignore_ascii_case(query) {
        command_name.get(query.len()..)
    } else {
        None
    }
}

pub(crate) fn choice_suggestion<'a>(kind: &'a ArgumentKind, input: &str) -> Option<&'a str> {
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
