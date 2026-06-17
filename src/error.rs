use std::error::Error as StdError;
use std::fmt::{self, Display, Formatter};
use std::io;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Config(String),
    Parse(String),
    Generate(String),
    Unsupported(String),
    Backend(String),
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::Config(msg) => write!(f, "config error: {msg}"),
            Self::Parse(msg) => write!(f, "parse error: {msg}"),
            Self::Generate(msg) => write!(f, "generation error: {msg}"),
            Self::Unsupported(msg) => write!(f, "unsupported: {msg}"),
            Self::Backend(msg) => write!(f, "backend error: {msg}"),
        }
    }
}

impl StdError for Error {}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
