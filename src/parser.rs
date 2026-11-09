use crate::{
    argument::{Argument, ArgumentKind},
    command::CommandSpec,
    completion::completion_suffix,
};

pub(crate) fn parse_command_arguments(
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

pub(crate) fn tokenize_arguments(input: &str) -> Result<Vec<String>, String> {
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

pub(crate) fn command_name_and_arguments(line: &str) -> (&str, &str) {
    let trimmed = line.trim_start();
    let end = trimmed
        .char_indices()
        .find_map(|(index, character)| character.is_whitespace().then_some(index))
        .unwrap_or(trimmed.len());
    (&trimmed[..end], trimmed[end..].trim())
}

pub(crate) fn display_command_line(command_name: &str, input: &str) -> String {
    let (_, arguments) = command_name_and_arguments(input);
    if arguments.is_empty() {
        command_name.to_owned()
    } else {
        format!("{command_name} {arguments}")
    }
}

pub(crate) fn resolve_argument_value(argument: &Argument, input: &str) -> Result<String, String> {
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
