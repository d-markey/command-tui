use std::io::{self, Stdout, Write};

use crossterm::{
    cursor::{MoveTo, MoveToColumn, MoveUp, Show},
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    queue,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{self, Clear, ClearType, disable_raw_mode, enable_raw_mode},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    argument::{Argument, ArgumentKind},
    command::{CommandSpec, CommandTui, resolved_help_display},
    completion::{
        choice_suggestion, complete_command_line_argument, complete_path, completion_suffix,
        handle_search_key,
    },
    history::CommandHistory,
    parser::{
        command_name_and_arguments, display_command_line, parse_command_arguments,
        resolve_argument_value,
    },
};

impl CommandTui {
    /// Runs the interactive command interface until the user quits.
    ///
    /// This method requires an interactive terminal. It reads keys in raw mode,
    /// restores terminal mode on exit, and suspends the interface while command
    /// handlers run so they can use standard input and output.
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

pub(crate) fn fit_line(text: &str, width: usize) -> String {
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

pub(crate) fn wrap_text(text: &str, width: usize) -> Vec<String> {
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
