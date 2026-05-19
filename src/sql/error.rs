use std::fmt;
use sqlparser::parser::ParserError;
use sqlparser::tokenizer::TokenizerError;

#[derive(Debug)]
pub enum Error {
    Parse(ParserError),
    Tokenize(TokenizerError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(e) => write!(f, "Parse error: {e}"),
            Error::Tokenize(e) => write!(f, "Tokenize error: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Parse(e) => Some(e),
            Error::Tokenize(e) => Some(e),
        }
    }
}

impl From<ParserError> for Error {
    fn from(e: ParserError) -> Self {
        Error::Parse(e)
    }
}

impl From<TokenizerError> for Error {
    fn from(e: TokenizerError) -> Self {
        Error::Tokenize(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
