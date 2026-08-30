//! One error type for the whole generator: where, what, and how we got
//! there.

use std::fmt;

#[derive(Debug)]
pub struct Error {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub msg: String,
    pub frames: Vec<String>,
}

impl Error {
    pub fn new(file: impl Into<String>, msg: impl Into<String>) -> Error {
        Error { file: file.into(), line: 0, col: 0, msg: msg.into(), frames: Vec::new() }
    }

    pub fn at(file: impl Into<String>, line: usize, col: usize, msg: impl Into<String>) -> Error {
        Error { file: file.into(), line, col, msg: msg.into(), frames: Vec::new() }
    }

    pub fn frame(mut self, frame: String) -> Error {
        self.frames.push(frame);
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 {
            write!(f, "{}:{}:{}: {}", self.file, self.line, self.col, self.msg)?;
        } else {
            write!(f, "{}: {}", self.file, self.msg)?;
        }
        for frame in &self.frames {
            write!(f, "\n  in {frame}")?;
        }
        Ok(())
    }
}

impl From<lace::Error> for Error {
    fn from(e: lace::Error) -> Error {
        Error { file: e.file, line: e.line, col: e.col, msg: e.msg, frames: e.frames }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
