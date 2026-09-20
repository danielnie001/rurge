//! Credentials inside a spec: compared and cloned like any other field, never
//! shown by `Debug`.

use std::fmt;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    pub fn new(value: T) -> Secret<T> {
        Secret(value)
    }

    /// The value itself. Every caller is a place a credential can leave from.
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl From<&str> for Secret<String> {
    fn from(value: &str) -> Secret<String> {
        Secret(value.to_string())
    }
}

impl From<String> for Secret<String> {
    fn from(value: String) -> Secret<String> {
        Secret(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_compares_and_clones_but_never_prints() {
        let a: Secret<String> = "hunter2".into();
        assert_eq!(a, Secret::new("hunter2".to_string()));
        assert_ne!(a, Secret::from("hunter3"));
        assert_eq!(a.clone().expose(), "hunter2");
        assert_eq!(format!("{a:?}"), "Secret(***)");
        assert_eq!(
            format!("{:?}", Some(Secret::new([7u8; 16]))),
            "Some(Secret(***))"
        );
        assert_eq!(format!("{:#?}", Secret::new(7u8)), "Secret(***)");
    }
}
