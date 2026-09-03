use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use std::fmt;
use std::path::Path;
use std::sync::Arc;

/// Location of a profile line: file plus 1-based line number.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Span {
    pub file: Arc<Path>,
    pub line: u32,
}

impl Span {
    pub fn new(file: Arc<Path>, line: u32) -> Self {
        Self { file, line }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.file.display(), self.line)
    }
}

impl Serialize for Span {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("Span", 2)?;
        st.serialize_field("file", &self.file.display().to_string())?;
        st.serialize_field("line", &self.line)?;
        st.end()
    }
}
