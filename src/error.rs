//! Recoverable qdu failures and their stable process exit codes.

use std::fmt;

/// A failure that ends a command with a documented exit code.
#[derive(Debug)]
pub enum Error {
    /// Invalid command input or configuration (exit code 2).
    Usage(String),
    /// Malformed or incompatible stored data (exit code 4).
    Format(String),
    /// Another qdu operation holds the profile lock (exit code 5).
    Busy(String),
    /// A snapshot failed integrity verification (exit code 6).
    Verification(String),
    /// Any other operating-system or database failure (exit code 1).
    Runtime(String),
}

impl Error {
    /// The process exit code that reports this failure.
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Runtime(_) => 1,
            Error::Usage(_) => 2,
            Error::Format(_) => 4,
            Error::Busy(_) => 5,
            Error::Verification(_) => 6,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Error::Usage(message)
            | Error::Format(message)
            | Error::Busy(message)
            | Error::Verification(message)
            | Error::Runtime(message) => message,
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Runtime(describe_io(&error))
    }
}

/// The system's description of an I/O error, such as "Permission denied",
/// without the "(os error 13)" suffix that Rust adds.
pub fn describe_io(error: &std::io::Error) -> String {
    let text = error.to_string();
    text.strip_suffix(')')
        .and_then(|rest| {
            rest.rfind(" (os error ")
                .map(|index| rest[..index].to_string())
        })
        .unwrap_or(text)
}

/// Attach the path an I/O operation used to its error.
pub trait PathContext<T> {
    fn at(self, path: &std::path::Path) -> Result<T>;
}

impl<T> PathContext<T> for std::io::Result<T> {
    fn at(self, path: &std::path::Path) -> Result<T> {
        self.map_err(|error| Error::Runtime(format!("{}: {}", path.display(), describe_io(&error))))
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Error::Runtime(format!("database error: {error}"))
    }
}

/// Exit statuses of commands that ran but report a finding. Errors map to
/// theirs in [`Error::exit_code`].
pub mod exit {
    /// `doctor` found a failed check.
    pub const FAILED_CHECK: i32 = 1;
    /// `profile list` met profiles it could not read.
    pub const INVALID_PROFILE: i32 = 2;
    /// `snapshot` saved an incomplete snapshot.
    pub const INCOMPLETE: i32 = 3;
    /// `check` raised an alert.
    pub const ALERT: i32 = 10;
}

/// The result type used throughout qdu.
pub type Result<T> = std::result::Result<T, Error>;

/// Build a usage error from any message.
pub fn usage(message: impl Into<String>) -> Error {
    Error::Usage(message.into())
}
