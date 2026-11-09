use std::{fs, io};

#[derive(Clone, Debug, PartialEq, Eq)]
/// A named value accepted by a [`crate::CommandSpec`].
///
/// Use the constructors such as [`Argument::text`] and [`Argument::choice`]
/// to select a value type, then optionally mark the argument with
/// [`Argument::optional`] or provide a default with [`Argument::with_default`].
pub struct Argument {
    /// Name used for named command-line input and argument prompts.
    pub name: String,
    /// Description shown below the argument prompt and in `help <command>`.
    pub description: String,
    /// The accepted value type and its completion behavior.
    pub kind: ArgumentKind,
    /// Whether the argument must have a non-empty value if no default is set.
    pub required: bool,
    /// Value used when the user leaves this argument empty.
    pub default: Option<String>,
}

impl Argument {
    /// Creates an argument that accepts any non-empty text.
    pub fn text(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(name, description, ArgumentKind::Text)
    }

    /// Creates an argument that accepts a signed 64-bit integer.
    pub fn integer(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(name, description, ArgumentKind::Integer)
    }

    /// Creates an argument that accepts `true` or `false`, case-insensitively.
    pub fn boolean(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(name, description, ArgumentKind::Boolean)
    }

    /// Creates an argument that accepts one of `choices`.
    ///
    /// Unique prefixes autocomplete with Tab and are accepted when submitted.
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

    /// Creates an argument containing a filesystem path.
    ///
    /// Missing paths are accepted. Tab completes matching files and
    /// directories.
    pub fn path(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(name, description, ArgumentKind::Path)
    }

    /// Creates an argument for a file path.
    ///
    /// A missing path is accepted so handlers can create the file. If the path
    /// exists, it must refer to a file.
    pub fn file(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(name, description, ArgumentKind::File)
    }

    /// Creates an argument for a directory path.
    ///
    /// A missing path is accepted so handlers can create the directory. If the
    /// path exists, it must refer to a directory.
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

    /// Marks this argument as optional.
    ///
    /// If omitted, the handler receives an empty string unless a default is
    /// configured.
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    /// Sets the value used when this argument is left empty.
    ///
    /// The default is validated against this argument's type when the command
    /// registry is created.
    pub fn with_default(mut self, default: impl Into<String>) -> Self {
        self.default = Some(default.into());
        self
    }

    pub(crate) fn validate(&self, value: &str) -> Result<String, String> {
        let value = value.trim();
        if value.is_empty() {
            if let Some(default) = &self.default
                && !default.trim().is_empty()
            {
                return self.validate_value(default);
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
/// The supported argument value types.
pub enum ArgumentKind {
    /// Arbitrary text.
    Text,
    /// A signed 64-bit integer.
    Integer,
    /// A boolean value, `true` or `false`.
    Boolean,
    /// One value from the supplied list.
    Choice(Vec<String>),
    /// Any path, whether or not it currently exists.
    Path,
    /// A path that may be missing, but must be a file if it exists.
    File,
    /// A path that may be missing, but must be a directory if it exists.
    Directory,
}

impl ArgumentKind {
    pub(crate) fn label(&self) -> &'static str {
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
