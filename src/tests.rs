use std::{fs, path::MAIN_SEPARATOR};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{
    argument::{Argument, ArgumentKind},
    command::{CommandSpec, CommandTui, format_help, help_commands, resolved_help_display},
    completion::{
        choice_suggestion, complete_command_line_argument, complete_path, completion_suffix,
        handle_search_key,
    },
    history::CommandHistory,
    parser::{display_command_line, parse_command_arguments},
    ui::{fit_line, wrap_text},
};

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
        parse_command_arguments(&command, r#"greet "say ""hello""" name: "Ada Lovelace""#).unwrap(),
        [
            Some(r#"say "hello""#.to_owned()),
            Some("Ada Lovelace".to_owned())
        ]
    );
    assert_eq!(
        parse_command_arguments(&command, r#"greet message:"quoted value" name:david"#).unwrap(),
        [Some("quoted value".to_owned()), Some("david".to_owned())]
    );
}

#[test]
fn command_line_arguments_reject_unclosed_quotes() {
    let command = CommandSpec::new("greet", "", vec![Argument::text("message", "")], |_| Ok(()));
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
