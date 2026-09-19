//! Why an outbound could not be built from its spec.

use std::fmt;

/// The text is shown to the user (`rurge check`): never put a secret in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildError {
    pub message: String,
}

impl BuildError {
    pub fn new(message: impl Into<String>) -> BuildError {
        BuildError {
            message: message.into(),
        }
    }
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for BuildError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_errors_are_plain_messages() {
        let e = BuildError::new("keystore item `cert1` cannot be decoded");
        assert_eq!(e.to_string(), "keystore item `cert1` cannot be decoded");
        assert_eq!(
            e,
            BuildError {
                message: "keystore item `cert1` cannot be decoded".into()
            }
        );
        let _: &dyn std::error::Error = &e;
    }
}
