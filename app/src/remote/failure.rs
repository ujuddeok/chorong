//! What a failed open says and offers. Pure: the adapter turns the backend's error into a
//! [`Failure`]; [`Failure::shown`] decides the sentence, the command to copy and the buttons.

use cho_sources::TargetKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailKind {
    /// The reader cancelled: nothing is shown.
    Cancelled,
    Offline,
    Auth {
        host: String,
    },
    NotFound,
    /// No such commit (`commit` as it was asked for, lower case).
    CommitNotFound {
        commit: String,
    },
    /// The name is both a branch and a commit id.
    Ambiguous {
        branch: String,
        commit: String,
    },
    LockBusy,
    /// Unsupported, and anything else: the backend's sentence is all there is.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailKind,
    pub message: String,
    /// The full story (the git command and the end of its error output, already redacted),
    /// behind a "Details" disclosure.
    pub detail: Option<String>,
}

/// A "Details" disclosure: closed until the reader opens it, and closed again for a new error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Disclosure {
    pub open: bool,
}

impl Disclosure {
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// The control's label.
    pub fn label(self) -> &'static str {
        if self.open {
            "\u{25be} Details"
        } else {
            "\u{25b8} Details"
        }
    }
}

impl Failure {
    pub fn other(message: impl Into<String>) -> Self {
        Self {
            kind: FailKind::Other,
            message: message.into(),
            detail: None,
        }
    }
}

/// A button that opens again as something specific.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    pub kind: TargetKind,
}

/// What the form shows under the boxes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Shown {
    pub text: String,
    /// A command to run in a terminal, shown as it is.
    pub command: Option<String>,
    pub choices: Vec<Choice>,
    /// For the "Details" disclosure; `None` hides it.
    pub detail: Option<String>,
}

impl Shown {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }
}

impl Failure {
    /// `None` for a cancel: it is silent.
    pub fn shown(&self) -> Option<Shown> {
        let mut shown = self.shown_without_detail()?;
        shown.detail = self.detail.clone();
        Some(shown)
    }

    fn shown_without_detail(&self) -> Option<Shown> {
        let m = self.message.clone();
        Some(match &self.kind {
            FailKind::Cancelled => return None,
            FailKind::Offline => Shown::plain(format!(
                "Offline: {m}. Branches and pull requests Chorong has fetched before still open \
                 from its cache."
            )),
            FailKind::Auth { host } => Shown {
                text: format!("{host} refused access: add a token, then open again."),
                command: Some(format!("cho auth add {host}")),
                choices: Vec::new(),
                detail: None,
            },
            FailKind::NotFound | FailKind::CommitNotFound { .. } | FailKind::Other => {
                Shown::plain(m)
            }
            FailKind::LockBusy => {
                Shown::plain("Another Chorong is using this repository; try again shortly")
            }
            FailKind::Ambiguous { branch, commit } => Shown {
                text: format!("{branch} is both a branch and a commit: which do you mean?"),
                command: None,
                detail: None,
                choices: vec![
                    Choice {
                        label: format!("Open branch {branch}"),
                        kind: TargetKind::Branch(branch.clone()),
                    },
                    Choice {
                        label: format!("Open commit {}", cho_git::short(commit)),
                        kind: TargetKind::Commit(commit.clone()),
                    },
                ],
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(kind: FailKind) -> Failure {
        Failure {
            kind,
            message: "the sentence".into(),
            detail: Some("git clone -- file:///x\nfatal: refused".into()),
        }
    }

    #[test]
    fn a_cancel_says_nothing() {
        assert_eq!(f(FailKind::Cancelled).shown(), None);
    }

    #[test]
    fn auth_names_the_command_for_the_host() {
        let s = f(FailKind::Auth {
            host: "github.com".into(),
        })
        .shown()
        .unwrap();
        assert_eq!(s.command.as_deref(), Some("cho auth add github.com"));
        assert!(s.text.contains("add a token"));
    }

    #[test]
    fn ambiguous_offers_the_branch_and_the_commit() {
        let s = f(FailKind::Ambiguous {
            branch: "deadbeef".into(),
            commit: "deadbeefcafe0123".into(),
        })
        .shown()
        .unwrap();
        assert_eq!(s.choices[0].label, "Open branch deadbeef");
        assert_eq!(s.choices[0].kind, TargetKind::Branch("deadbeef".into()));
        assert_eq!(s.choices[1].label, "Open commit deadbee");
        assert_eq!(
            s.choices[1].kind,
            TargetKind::Commit("deadbeefcafe0123".into())
        );
    }

    #[test]
    fn the_other_kinds_say_their_sentence_or_the_lock_words() {
        assert_eq!(f(FailKind::NotFound).shown().unwrap().text, "the sentence");
        assert_eq!(f(FailKind::Other).shown().unwrap().text, "the sentence");
        assert_eq!(
            f(FailKind::LockBusy).shown().unwrap().text,
            "Another Chorong is using this repository; try again shortly"
        );
        assert!(
            f(FailKind::Offline)
                .shown()
                .unwrap()
                .text
                .starts_with("Offline: the sentence")
        );
    }

    #[test]
    fn the_detail_goes_with_the_error_and_is_hidden_without_one() {
        for k in [FailKind::Offline, FailKind::NotFound, FailKind::Other] {
            let shown = f(k).shown().unwrap();
            assert_eq!(
                shown.detail.as_deref(),
                Some("git clone -- file:///x\nfatal: refused")
            );
        }
        assert_eq!(Failure::other("x").shown().unwrap().detail, None);
        assert_eq!(Shown::plain("x").detail, None);
    }
}
