use std::{
    fs,
    io::{self, Write},
};

use command_tui::{Argument, CommandSpec, CommandTui};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let commands = vec![
        CommandSpec::new(
            "greet",
            "Print a greeting",
            vec![Argument::text("name", "Who should be greeted?")],
            |arguments| {
                println!("Hello, {}!", arguments[0]);
                Ok(())
            },
        ),
        CommandSpec::new("grunt", "A greeting for orks", vec![], |_| {
            println!("Oi, ork! WAAAGH!");
            Ok(())
        }),
        CommandSpec::new(
            "cat",
            "Display a file's contents",
            vec![Argument::file("file", "File to display")],
            |arguments| {
                let contents = fs::read_to_string(&arguments[0])
                    .map_err(|error| format!("could not read {}: {error}", arguments[0]))?;
                print!("{contents}");
                Ok(())
            },
        ),
        CommandSpec::new(
            "check_path",
            "Check whether a path is a file or directory",
            vec![Argument::path("path", "Path to check")],
            |arguments| match fs::metadata(&arguments[0]) {
                Ok(metadata) if metadata.is_file() => {
                    println!("{} is a file", arguments[0]);
                    Ok(())
                }
                Ok(metadata) if metadata.is_dir() => {
                    println!("{} is a directory", arguments[0]);
                    Ok(())
                }
                Ok(_) => {
                    println!(
                        "{} exists but is neither a file nor a directory",
                        arguments[0]
                    );
                    Ok(())
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    println!("{} does not exist", arguments[0]);
                    Ok(())
                }
                Err(error) => Err(format!("could not check {}: {error}", arguments[0])),
            },
        ),
        CommandSpec::new(
            "count",
            "Count to a number",
            vec![Argument::integer("limit", "The final number")],
            |arguments| {
                let limit = arguments[0]
                    .parse::<i64>()
                    .map_err(|error| error.to_string())?;
                println!("Counting to {limit}");
                Ok(())
            },
        ),
        CommandSpec::new(
            "environment",
            "Select a deployment environment",
            vec![
                Argument::choice("target", "Where to deploy?", ["development", "production"]),
                Argument::boolean("dry-run", "Only show what would happen?").with_default("true"),
            ],
            |arguments| {
                print!("Deploy to {}? [y/N] ", arguments[0]);
                io::stdout().flush().map_err(|error| error.to_string())?;

                let mut answer = String::new();
                io::stdin()
                    .read_line(&mut answer)
                    .map_err(|error| error.to_string())?;

                if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                    println!("Cancelled.");
                    return Ok(());
                }

                println!("Deploying to {} (dry run: {})", arguments[0], arguments[1]);
                Ok(())
            },
        ),
    ];

    CommandTui::new(commands)?.run()?;
    Ok(())
}
