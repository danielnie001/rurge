//! What the text a line stands in may make rurge do on this machine.
//!
//! Where a text came from decides it, never what the text says about itself:
//! the user's own profile and the local files it takes in may name any local
//! file and start programs; downloaded text, the copy of a managed profile as
//! much as a remote include, names nothing on this machine and starts
//! nothing. A text never gains rights by being taken in by another.

/// What the text a line stands in may make rurge do on this machine: which
/// local files it may name, and whether a program it names is started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// The user's own text. Any local file, by any path; its `external`
    /// policies are started.
    Anywhere,
    /// The downloaded copy of a managed profile: nothing on this machine, like
    /// `Nowhere`. It is told apart for what is said of a line that asks for
    /// more: the user is told to write a local profile that takes the
    /// downloaded parts in.
    ManagedCopy,
    /// Any other downloaded text: nothing on this machine.
    Nowhere,
}

/// What is said of a line that asks for more than its text may: the warning
/// and what to do about it. Neither quotes anything of the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub(crate) message: &'static str,
    pub(crate) hint: &'static str,
}

impl Reach {
    /// What is said when text of this reach `#!include`s a local file, if it
    /// may not; the line is ignored. The one place that decides whether a
    /// text may include a local file.
    pub(crate) fn include_refusal(self) -> Option<Refusal> {
        match self {
            Reach::Anywhere => None,
            Reach::ManagedCopy => Some(Refusal {
                message: "the downloaded copy of a managed profile cannot include a local file; the line is ignored",
                hint: "to include a local file, load a profile without `#!MANAGED-CONFIG` that takes the downloaded parts in by `#!include`",
            }),
            Reach::Nowhere => Some(Refusal {
                message: "a downloaded file cannot include a local file; the line is ignored",
                hint: "include the local file from the local profile instead",
            }),
        }
    }

    /// What is said when text of this reach names a local file as a set, if it
    /// may not. The message is the reason; whoever drops the line says so.
    pub(crate) fn set_refusal(self) -> Option<Refusal> {
        match self {
            Reach::Anywhere => None,
            Reach::ManagedCopy => Some(Refusal {
                message: "the downloaded copy of a managed profile cannot name a local file as a set",
                hint: "name the set by a URL instead",
            }),
            Reach::Nowhere => Some(Refusal {
                message: "a downloaded file cannot name a local file as a set",
                hint: "write the line in the local profile to use a local set",
            }),
        }
    }

    /// What is said when the group a text of this reach defines names a local
    /// file as its `policy-path`, if it may not. The message follows the
    /// group's name.
    pub(crate) fn policy_path_refusal(self) -> Option<Refusal> {
        match self {
            Reach::Anywhere => None,
            Reach::ManagedCopy => Some(Refusal {
                message: "the downloaded copy of a managed profile cannot name a local file as `policy-path`; the parameter is ignored",
                hint: "give `policy-path` a URL instead",
            }),
            Reach::Nowhere => Some(Refusal {
                message: "a downloaded file cannot name a local file as `policy-path`; the parameter is ignored",
                hint: "define the group in the local profile to import policies from a local file",
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only the user's own text includes a local file: the copy of a managed
    /// profile and any other downloaded text do not, and say so in words of
    /// their own.
    #[test]
    fn only_the_users_own_text_includes_a_local_file() {
        assert_eq!(Reach::Anywhere.include_refusal(), None);
        assert_eq!(
            Reach::ManagedCopy.include_refusal(),
            Some(Refusal {
                message: "the downloaded copy of a managed profile cannot include a local file; the line is ignored",
                hint: "to include a local file, load a profile without `#!MANAGED-CONFIG` that takes the downloaded parts in by `#!include`",
            })
        );
        assert_eq!(
            Reach::Nowhere.include_refusal(),
            Some(Refusal {
                message: "a downloaded file cannot include a local file; the line is ignored",
                hint: "include the local file from the local profile instead",
            })
        );
    }
}
