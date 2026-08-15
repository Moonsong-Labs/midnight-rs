use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    message: String,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error {
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "normalized-ir: {}", self.message)
    }
}

impl std::error::Error for Error {}

impl From<crate::sexp::ReadError> for Error {
    fn from(e: crate::sexp::ReadError) -> Self {
        Error::new(e.to_string())
    }
}
