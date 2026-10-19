use std::fmt;

#[derive(Debug, Clone)]
pub struct Error(pub &'static str);
pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self("STORAGE_ERROR")
    }
}
