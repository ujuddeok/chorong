//! The Projects, Cache and Accounts sections without any GUI: the words each row says, the
//! confirmations (which name exactly what goes), the loading states, and the masked token
//! field. `view/remote.rs` draws them and runs the background work through the adapter.
//!
//! A token only ever lives in a [`SecretField`] while it is typed and in a [`Secret`] on its
//! way to the keychain. Neither prints its value (not in `Debug`, there is no `Display`), and
//! both overwrite their bytes when they are dropped or cleared.

use crate::remote::adapter::{
    AccountInfo, Accounts, ApprovalFact, CacheUsage, Checked, ProjectInfo,
};
use cho_sources::hosts::TokenCheck;
use cho_sources::usage::{RepoUsage, WorktreeUsage};
use cho_sources::{Host, RateLimit};
use std::collections::BTreeMap;
use std::fmt;

/// The most worktrees a project may keep from this page (the backend only needs 1 or more).
pub const MAX_WORKTREE_CAP: usize = 50;

/// The sections of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Settings,
    Projects,
    Cache,
    Accounts,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Settings, Tab::Projects, Tab::Cache, Tab::Accounts];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Settings => "Settings",
            Tab::Projects => "Projects",
            Tab::Cache => "Cache",
            Tab::Accounts => "Accounts",
        }
    }

    /// Ctrl/Cmd and this digit.
    pub fn from_digit(c: char) -> Option<Tab> {
        let i = c.to_digit(10)? as usize;
        Tab::ALL.get(i.checked_sub(1)?).copied()
    }
}

/// Something read in the background.
#[derive(Debug, Clone, Default)]
pub enum Load<T> {
    #[default]
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Load<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(t) => Some(t),
            _ => None,
        }
    }

    pub fn from_result(r: Result<T, String>) -> Self {
        match r {
            Ok(t) => Load::Ready(t),
            Err(e) => Load::Failed(e),
        }
    }
}

// ---- Words ----

/// `4 KB`, `12 MB`, `3.4 GB`.
pub use cho_core::words::size_text;

/// `just now`, `5 min ago`, `3 days ago` (as everywhere else); `never` when it is not known.
pub fn ago_text(opened: Option<u64>, now: u64) -> String {
    opened.map_or_else(|| "never".into(), |t| cho_core::words::ago(now, t))
}

/// What a worktree is: `pull request 12`, `branch main`; the folder name when the cache has no
/// record of it.
pub fn worktree_label(w: &WorktreeUsage) -> String {
    w.what.clone().unwrap_or_else(|| w.slug.clone())
}

pub fn approval_text(a: &ApprovalFact) -> String {
    match a {
        ApprovalFact::Approved { commit } => {
            format!("approved at {}", cho_git::short(commit))
        }
        ApprovalFact::NotApproved => "not approved".into(),
        ApprovalFact::NoRecipe => "no recipe".into(),
    }
}

/// A worktree cap typed into the field.
pub fn parse_cap(text: &str) -> Result<usize, String> {
    let n: usize = text
        .trim()
        .parse()
        .map_err(|_| format!("Enter a whole number from 1 to {MAX_WORKTREE_CAP}"))?;
    if (1..=MAX_WORKTREE_CAP).contains(&n) {
        Ok(n)
    } else {
        Err(format!("The cap must be from 1 to {MAX_WORKTREE_CAP}"))
    }
}

// ---- Confirmations ----

/// What a confirmation is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    RemoveProject(String),
    ClearWorktree { repo: String, slug: String },
    ClearRepo(String),
    ClearAll,
    RemoveToken(String),
}

/// How much a confirmation will delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sized {
    pub bytes: u64,
    pub worktrees: usize,
}

pub fn sized_repo(u: &RepoUsage) -> Sized {
    Sized {
        bytes: u.total_bytes,
        worktrees: u.worktrees.len(),
    }
}

pub fn sized_worktree(w: &WorktreeUsage) -> Sized {
    Sized {
        bytes: w.bytes,
        worktrees: 1,
    }
}

pub fn sized_all(c: &CacheUsage) -> Sized {
    Sized {
        bytes: c.repos.iter().map(|r| r.total_bytes).sum(),
        worktrees: c.repos.iter().map(|r| r.worktrees.len()).sum(),
    }
}

fn worktrees_text(n: usize) -> String {
    format!("{n} worktree{}", if n == 1 { "" } else { "s" })
}

/// The question, what goes, and the button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmText {
    pub question: String,
    pub detail: String,
    pub button: &'static str,
}

/// `size` is `None` while it is still being measured.
pub fn confirm_text(c: &Confirm, size: Option<Sized>) -> ConfirmText {
    let measured = |s: Option<Sized>| match s {
        Some(s) => format!("{} ({})", worktrees_text(s.worktrees), size_text(s.bytes)),
        None => "its worktrees (size being measured\u{2026})".to_string(),
    };
    match c {
        Confirm::RemoveProject(repo) => ConfirmText {
            question: format!("Remove the project {repo}?"),
            detail: format!(
                "This deletes the project entry, Chorong's cached clone of it and {}, and their \
                 indexes. Your own clones of the repository are not touched. Opening the \
                 project again clones it afresh.",
                measured(size)
            ),
            button: "Remove project",
        },
        Confirm::ClearWorktree { repo, slug } => ConfirmText {
            question: format!("Remove the worktree {slug} of {repo}?"),
            detail: format!(
                "This deletes Chorong's copy of it{} and its index. The project and its clone stay.",
                size.map(|s| format!(" ({})", size_text(s.bytes)))
                    .unwrap_or_default()
            ),
            button: "Remove worktree",
        },
        Confirm::ClearRepo(repo) => ConfirmText {
            question: format!("Clear the cache of {repo}?"),
            detail: format!(
                "This deletes the cached clone and {}, and their indexes. The project stays \
                 and is cloned again when opened.",
                measured(size)
            ),
            button: "Clear",
        },
        Confirm::ClearAll => ConfirmText {
            question: "Clear the whole cache?".into(),
            detail: format!(
                "This deletes every cached clone and {}, and their indexes. Projects stay \
                 and are cloned again when opened. Nothing outside Chorong's cache folder is \
                 touched.",
                measured(size)
            ),
            button: "Clear all",
        },
        Confirm::RemoveToken(host) => ConfirmText {
            question: format!("Remove the token for {host}?"),
            detail: "This deletes it (and the saved email) from the keychain. A token in the \
                     environment is not affected."
                .into(),
            button: "Remove token",
        },
    }
}

// ---- Accounts ----

/// Where the token for a host comes from.
pub fn token_text(a: &AccountInfo) -> String {
    match a.status.source {
        Some(s) => s.to_string(),
        None => "no token".into(),
    }
}

/// Bitbucket Cloud's REST calls need the account email.
pub fn email_text(a: &AccountInfo) -> Option<&'static str> {
    (matches!(a.host, Host::BitbucketCloud) && a.status.source.is_some()).then_some(
        if a.status.has_email {
            "account email set"
        } else {
            "no account email (needed for REST)"
        },
    )
}

/// The result of a Check.
pub fn check_text(r: &Result<Checked, String>) -> String {
    match r {
        Ok(c) => match &c.check {
            TokenCheck::Valid => "valid".into(),
            TokenCheck::Invalid => "refused by the host".into(),
            TokenCheck::Unknown(why) => format!("not checked ({why})"),
        },
        Err(e) => format!("not checked ({e})"),
    }
}

pub fn quota_text(r: Option<RateLimit>) -> Option<String> {
    r.map(|q| format!("REST quota {} of {}", q.remaining, q.limit))
}

/// Why a token can't be stored from here, when it can't.
pub fn keychain_problem(accounts: &Accounts, a: &AccountInfo) -> Option<String> {
    accounts
        .keychain
        .as_ref()
        .err()
        .cloned()
        .or_else(|| a.status.keychain_error.clone())
}

/// What to set when there is no keychain.
pub fn env_instruction(host: &Host) -> String {
    match host.email_env() {
        Some(e) => format!(
            "Set {} and {e} in the environment Chorong runs in.",
            host.token_env()
        ),
        None => format!(
            "Set {} in the environment Chorong runs in.",
            host.token_env()
        ),
    }
}

/// A Check in progress or done, per host name.
#[derive(Debug, Clone)]
pub enum CheckState {
    Checking,
    Done(Result<Checked, String>),
}

// ---- The masked token field ----

/// Overwrites a string's bytes with zeros and empties it.
pub fn wipe(s: &mut String) {
    let mut bytes = std::mem::take(s).into_bytes();
    bytes.fill(0);
    std::hint::black_box(&bytes);
}

/// The most bytes a token field takes. The buffer is allocated once at this size, so typing
/// never reallocates and leaves no copy of a half-typed token behind.
const TOKEN_CAPACITY: usize = 512;

/// A token on its way to the keychain. It prints nothing and zeroes on drop.
pub struct Secret(String);

impl Secret {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Secret(<{} bytes>)", self.0.len())
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

/// The token being typed. The window draws one bullet per character ([`Self::masked`]) and
/// never the value; the value is not in `Debug` and there is no `Display`.
pub struct SecretField {
    value: String,
}

impl Default for SecretField {
    fn default() -> Self {
        Self {
            value: String::with_capacity(TOKEN_CAPACITY),
        }
    }
}

impl fmt::Debug for SecretField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SecretField(<{} characters>)",
            self.value.chars().count()
        )
    }
}

impl Drop for SecretField {
    fn drop(&mut self) {
        wipe(&mut self.value);
    }
}

impl SecretField {
    /// Adds typed or pasted text; control characters are dropped, and so is anything past
    /// the capacity. Returns false when something was refused.
    pub fn push_str(&mut self, text: &str) -> bool {
        let mut all = true;
        for c in text.chars().filter(|c| !c.is_control()) {
            if self.value.len() + c.len_utf8() > TOKEN_CAPACITY {
                all = false;
                break;
            }
            self.value.push(c);
        }
        all
    }

    /// Empties the field, zeroing what it held; the buffer is allocated again at full size.
    pub fn clear(&mut self) {
        wipe(&mut self.value);
        self.value = String::with_capacity(TOKEN_CAPACITY);
    }

    /// Removes the last character and zeroes its bytes.
    pub fn pop(&mut self) {
        let mut bytes = std::mem::take(&mut self.value).into_bytes();
        let keep = String::from_utf8_lossy(&bytes)
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i);
        bytes[keep..].fill(0);
        bytes.truncate(keep);
        // The same allocation again: the bytes before `keep` were valid UTF-8 already.
        self.value = String::from_utf8(bytes).unwrap_or_default();
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    /// One bullet per character, for the field's text.
    pub fn masked(&self) -> String {
        "\u{2022}".repeat(self.value.chars().count())
    }

    /// Moves the value out (the field is left empty); the caller owns the only copy.
    pub fn take(&mut self) -> Secret {
        let out = std::mem::replace(&mut self.value, String::with_capacity(TOKEN_CAPACITY));
        Secret(out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormFocus {
    Token,
    Email,
}

/// The inline "Add token" form of one host.
#[derive(Debug)]
pub struct TokenForm {
    pub host: String,
    pub needs_email: bool,
    pub token: SecretField,
    pub email: String,
    pub focus: FormFocus,
    /// The keychain is being written.
    pub busy: bool,
    pub error: Option<String>,
}

/// A validated form, ready to send to the keychain.
#[derive(Debug)]
pub struct Submit {
    pub host: String,
    pub token: Secret,
    pub email: Option<String>,
}

impl TokenForm {
    pub fn new(host: &Host) -> Self {
        Self {
            host: host.name().to_string(),
            needs_email: matches!(host, Host::BitbucketCloud),
            token: SecretField::default(),
            email: String::new(),
            focus: FormFocus::Token,
            busy: false,
            error: None,
        }
    }

    /// A keystroke for the focused field, through the shared field keys (Cmd-A, paste, word
    /// and line delete). The token is never copied out, and whatever leaves it is zeroed.
    pub fn field(&mut self, k: &gpui::Keystroke, paste: Option<&str>, all: &mut bool) {
        use cho_layout_ui::field::{FieldEdit, FieldKey, edit_field, field_key};
        if self.busy {
            return;
        }
        match self.focus {
            FormFocus::Email => {
                if edit_field(k, &mut self.email, all, paste) == FieldEdit::Text {
                    self.email.retain(|c| !c.is_control() && c != ' ');
                    self.error = None;
                }
            }
            FormFocus::Token => match field_key(k, paste) {
                FieldKey::Insert(text) => {
                    if std::mem::take(all) {
                        self.token.clear();
                    }
                    self.error = None;
                    if !self.token.push_str(text.trim()) {
                        self.error = Some("That is longer than a token can be".into());
                    }
                }
                FieldKey::Backspace => {
                    if std::mem::take(all) {
                        self.token.clear();
                    } else {
                        self.token.pop();
                    }
                }
                FieldKey::DeleteWord | FieldKey::Clear => {
                    *all = false;
                    self.token.clear();
                }
                FieldKey::SelectAll => *all = !self.token.is_empty(),
                FieldKey::Deselect => *all = false,
                FieldKey::Copy | FieldKey::Other => {}
            },
        }
    }

    /// Tab: the other field (only Bitbucket Cloud has an email).
    pub fn next_focus(&mut self) {
        if self.needs_email {
            self.focus = match self.focus {
                FormFocus::Token => FormFocus::Email,
                FormFocus::Email => FormFocus::Token,
            };
        }
    }

    /// Enter: checks the form. The token leaves the field here, so the form holds no copy
    /// while the keychain is written.
    pub fn submit(&mut self) -> Option<Submit> {
        if self.busy {
            return None;
        }
        if self.token.is_empty() {
            self.error = Some("Paste or type the token first".into());
            return None;
        }
        let email = self.email.trim().to_string();
        if self.needs_email && email.is_empty() {
            self.error = Some("Bitbucket Cloud needs the account email with the token".into());
            self.focus = FormFocus::Email;
            return None;
        }
        self.busy = true;
        self.error = None;
        Some(Submit {
            host: self.host.clone(),
            token: self.token.take(),
            email: (!email.is_empty()).then_some(email),
        })
    }
}

// ---- Everything the three sections keep ----

/// The state of the Projects, Cache and Accounts sections.
#[derive(Debug, Default)]
pub struct RemoteUi {
    pub projects: Load<Vec<ProjectInfo>>,
    pub usage: Load<CacheUsage>,
    pub accounts: Load<Accounts>,
    pub checks: BTreeMap<String, CheckState>,
    /// What the reader is in the middle of: at most one at a time.
    pub editing: Editing,
    /// Cmd-A selected the whole text of the field being edited (a cap, a token, an email).
    pub field_all: bool,
    /// What the last action said.
    pub notice: Option<Notice>,
    /// A change is being made in the background.
    pub busy: Option<String>,
}

/// The one thing the three sections can be in the middle of. Opening one replaces the other.
#[derive(Debug, Default)]
pub enum Editing {
    #[default]
    None,
    /// A worktree cap being typed: the project and the text.
    Cap { repo: String, text: String },
    /// A confirmation on screen.
    Confirm(ConfirmState),
    /// The inline "Add token" form of one host.
    Token(TokenForm),
}

impl Editing {
    pub fn confirm(&self) -> Option<&ConfirmState> {
        match self {
            Editing::Confirm(c) => Some(c),
            _ => None,
        }
    }

    pub fn confirm_mut(&mut self) -> Option<&mut ConfirmState> {
        match self {
            Editing::Confirm(c) => Some(c),
            _ => None,
        }
    }

    /// Takes the confirmation off the screen, if one is there.
    pub fn take_confirm(&mut self) -> Option<ConfirmState> {
        match std::mem::take(self) {
            Editing::Confirm(c) => Some(c),
            other => {
                *self = other;
                None
            }
        }
    }

    pub fn token(&self) -> Option<&TokenForm> {
        match self {
            Editing::Token(f) => Some(f),
            _ => None,
        }
    }

    pub fn token_mut(&mut self) -> Option<&mut TokenForm> {
        match self {
            Editing::Token(f) => Some(f),
            _ => None,
        }
    }

    /// The cap text being typed for `repo`.
    pub fn cap_for(&self, repo: &str) -> Option<&str> {
        match self {
            Editing::Cap { repo: r, text } if r == repo => Some(text),
            _ => None,
        }
    }
}

/// What the last action said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Error(String),
    Done(String),
}

/// A confirmation on screen and what it will delete, once known.
#[derive(Debug, Clone)]
pub struct ConfirmState {
    pub what: Confirm,
    pub size: Option<Sized>,
}

impl ConfirmState {
    pub fn text(&self) -> ConfirmText {
        confirm_text(&self.what, self.size)
    }

    /// The button works once the size is known (tokens have none).
    pub fn can_confirm(&self) -> bool {
        matches!(self.what, Confirm::RemoveToken(_)) || self.size.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cho_sources::TokenSource;
    use cho_sources::auth::AuthStatus;
    use std::path::PathBuf;

    fn wt(slug: &str, what: Option<&str>, bytes: u64) -> WorktreeUsage {
        WorktreeUsage {
            slug: slug.into(),
            path: PathBuf::from(slug),
            bytes,
            what: what.map(str::to_string),
            opened: None,
        }
    }

    fn account(host: Host, source: Option<TokenSource>, email: bool) -> AccountInfo {
        AccountInfo {
            status: AuthStatus {
                host: host.clone(),
                source,
                has_email: email,
                keychain_error: None,
            },
            host,
        }
    }

    #[test]
    fn only_a_confirmation_is_taken_off_the_screen() {
        let mut e = Editing::Cap {
            repo: "github:a/b".into(),
            text: "3".into(),
        };
        assert!(e.take_confirm().is_none());
        assert_eq!(
            e.cap_for("github:a/b"),
            Some("3"),
            "a cap being typed stays"
        );
        assert_eq!(e.cap_for("github:c/d"), None);
        e = Editing::Confirm(ConfirmState {
            what: Confirm::ClearAll,
            size: None,
        });
        assert!(e.take_confirm().is_some());
        assert!(matches!(e, Editing::None));
    }

    #[test]
    fn approval_states_read_plainly() {
        let commit = "3f9c1e2a77b0d4c5e6f708192a3b4c5d6e7f8091".to_string();
        assert_eq!(
            approval_text(&ApprovalFact::Approved { commit }),
            "approved at 3f9c1e2"
        );
        assert_eq!(approval_text(&ApprovalFact::NotApproved), "not approved");
        assert_eq!(approval_text(&ApprovalFact::NoRecipe), "no recipe");
    }

    #[test]
    fn the_cap_is_one_to_fifty() {
        assert_eq!(parse_cap(" 7 "), Ok(7));
        assert_eq!(parse_cap("1"), Ok(1));
        assert_eq!(parse_cap("50"), Ok(50));
        for bad in ["0", "51", "", "x", "-3", "2.5"] {
            assert!(parse_cap(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn ages_read_plainly() {
        let now = 1_000_000_000;
        assert_eq!(ago_text(None, now), "never");
        assert_eq!(ago_text(Some(now - 5 * 60), now), "5 min ago");
    }

    #[test]
    fn worktrees_are_named_by_what_they_are() {
        assert_eq!(
            worktree_label(&wt("pr-12", Some("pull request 12"), 1)),
            "pull request 12"
        );
        assert_eq!(worktree_label(&wt("main", None, 1)), "main");
    }

    #[test]
    fn confirmations_name_what_goes_and_its_size() {
        let size = Some(Sized {
            bytes: 12 * 1024 * 1024,
            worktrees: 2,
        });
        let t = confirm_text(&Confirm::RemoveProject("github:acme/api".into()), size);
        assert_eq!(t.question, "Remove the project github:acme/api?");
        for part in [
            "project entry",
            "cached clone",
            "2 worktrees (12 MB)",
            "indexes",
            "Your own clones",
        ] {
            assert!(t.detail.contains(part), "{part}: {}", t.detail);
        }
        let t = confirm_text(&Confirm::RemoveProject("github:acme/api".into()), None);
        assert!(t.detail.contains("being measured"));
        let t = confirm_text(
            &Confirm::ClearWorktree {
                repo: "github:acme/api".into(),
                slug: "pr-12".into(),
            },
            Some(Sized {
                bytes: 2048,
                worktrees: 1,
            }),
        );
        assert!(t.detail.contains("(2 KB)") && t.detail.contains("index"));
        let t = confirm_text(&Confirm::ClearAll, size);
        assert!(t.detail.contains("2 worktrees (12 MB)") && t.button == "Clear all");
        let t = confirm_text(&Confirm::ClearRepo("github:acme/api".into()), size);
        assert!(t.detail.contains("The project stays"));
        let one = Some(Sized {
            bytes: 1,
            worktrees: 1,
        });
        assert!(
            confirm_text(&Confirm::ClearAll, one)
                .detail
                .contains("1 worktree (1 B)")
        );
        let t = confirm_text(&Confirm::RemoveToken("github.com".into()), None);
        assert!(t.detail.contains("keychain"));
        assert!(
            !ConfirmState {
                what: Confirm::ClearAll,
                size: None
            }
            .can_confirm()
        );
        assert!(
            ConfirmState {
                what: Confirm::RemoveToken("x".into()),
                size: None
            }
            .can_confirm()
        );
    }

    #[test]
    fn sizes_add_up_over_the_usage() {
        let repo = |bytes, n| RepoUsage {
            repo: "github:a/b".into(),
            bare: PathBuf::new(),
            clone_bytes: 0,
            worktrees: (0..n).map(|i| wt(&format!("w{i}"), None, 1)).collect(),
            total_bytes: bytes,
        };
        let c = CacheUsage {
            root: PathBuf::new(),
            repos: vec![repo(100, 2), repo(50, 1)],
        };
        assert_eq!(
            sized_all(&c),
            Sized {
                bytes: 150,
                worktrees: 3
            }
        );
        assert_eq!(sized_repo(&c.repos[0]).worktrees, 2);
        assert_eq!(sized_worktree(&wt("a", None, 9)).bytes, 9);
    }

    #[test]
    fn account_rows_say_where_the_token_is_and_what_is_missing() {
        let gh = account(Host::GitHub, Some(TokenSource::Keychain), false);
        assert_eq!(token_text(&gh), "keychain");
        assert_eq!(email_text(&gh), None);
        let env = account(
            Host::BitbucketCloud,
            Some(TokenSource::Env("CHORONG_BITBUCKET_TOKEN")),
            false,
        );
        assert_eq!(token_text(&env), "environment (CHORONG_BITBUCKET_TOKEN)");
        assert_eq!(email_text(&env), Some("no account email (needed for REST)"));
        let none = account(Host::BitbucketCloud, None, false);
        assert_eq!(token_text(&none), "no token");
        assert_eq!(email_text(&none), None);
        assert_eq!(
            email_text(&account(
                Host::BitbucketCloud,
                Some(TokenSource::Keychain),
                true
            )),
            Some("account email set")
        );
        assert_eq!(
            check_text(&Ok(Checked {
                check: TokenCheck::Valid,
                rate: None
            })),
            "valid"
        );
        assert_eq!(
            check_text(&Ok(Checked {
                check: TokenCheck::Invalid,
                rate: None
            })),
            "refused by the host"
        );
        assert_eq!(
            check_text(&Ok(Checked {
                check: TokenCheck::Unknown("not checked for Data Center".into()),
                rate: None
            })),
            "not checked (not checked for Data Center)"
        );
        assert_eq!(
            check_text(&Err("no token for github.com".into())),
            "not checked (no token for github.com)"
        );
        assert_eq!(
            quota_text(Some(RateLimit {
                remaining: 4980,
                limit: 5000
            })),
            Some("REST quota 4980 of 5000".into())
        );
        assert_eq!(
            env_instruction(&Host::GitHub),
            "Set CHORONG_GITHUB_TOKEN in the environment Chorong runs in."
        );
        assert!(env_instruction(&Host::BitbucketCloud).contains("CHORONG_BITBUCKET_EMAIL"));
    }

    #[test]
    fn a_missing_keychain_is_reported_for_every_host() {
        let gh = account(Host::GitHub, None, false);
        let accounts = Accounts {
            hosts: vec![gh.clone()],
            keychain: Err("no keychain is available".into()),
        };
        assert!(keychain_problem(&accounts, &gh).is_some());
        let ok = Accounts {
            hosts: vec![],
            keychain: Ok(()),
        };
        assert_eq!(keychain_problem(&ok, &gh), None);
        let mut broken = gh;
        broken.status.keychain_error = Some("locked".into());
        assert_eq!(keychain_problem(&ok, &broken).as_deref(), Some("locked"));
    }

    /// A keystroke as typed: `cmd` is the platform's command key (Ctrl off macOS).
    fn key(spec: &str) -> gpui::Keystroke {
        let spec = if cfg!(target_os = "macos") {
            spec.to_string()
        } else {
            spec.replace("cmd-", "ctrl-")
        };
        gpui::Keystroke::parse(&spec).unwrap()
    }

    /// Cmd-V of `text` into the focused field.
    fn paste(form: &mut TokenForm, text: &str) {
        form.field(&key("cmd-v"), Some(text), &mut false);
    }

    #[test]
    fn the_token_field_takes_the_shared_field_keys_and_never_copies_out() {
        let mut form = TokenForm::new(&Host::GitHub);
        let mut all = false;
        paste(&mut form, "ghp_one");
        form.field(&key("cmd-a"), None, &mut all);
        assert!(all, "Cmd-A selects the token");
        // Cmd-C does nothing to a secret, and leaves the selection.
        form.field(&key("cmd-c"), None, &mut all);
        assert!(all);
        // Typing replaces the selection (the old token is wiped).
        form.field(&key("cmd-v"), Some("ghp_two"), &mut all);
        assert_eq!(form.token.masked().chars().count(), "ghp_two".len());
        form.field(&key("cmd-a"), None, &mut all);
        form.field(&key("backspace"), None, &mut all);
        assert!(form.token.is_empty() && !all);
    }

    #[test]
    fn the_token_never_appears_in_debug_or_the_drawn_text() {
        let token = "ghp_SuperSecretValue123";
        let mut form = TokenForm::new(&Host::GitHub);
        paste(&mut form, token);
        let shown = format!("{form:?} {:?} {}", form.token, form.token.masked());
        assert!(!shown.contains(token), "{shown}");
        assert!(!shown.contains("SuperSecret"));
        assert_eq!(form.token.masked().chars().count(), token.len());
        assert!(form.token.masked().chars().all(|c| c == '\u{2022}'));
        let s = form.submit().unwrap();
        assert!(!format!("{s:?}").contains("ghp_"));
        assert_eq!(s.token.as_str(), token);
        // The field is empty again as soon as the token has left it.
        assert!(form.token.is_empty() && form.busy);
        assert_eq!(form.token.masked(), "");
    }

    #[test]
    fn wiping_overwrites_before_releasing() {
        let mut s = String::from("hunter2");
        wipe(&mut s);
        assert!(s.is_empty());
        let mut f = SecretField::default();
        f.push_str("abc\n\tdef");
        assert_eq!(
            f.masked().chars().count(),
            6,
            "control characters are dropped"
        );
        f.pop();
        assert_eq!(f.masked().chars().count(), 5);
        drop(f.take());
        assert!(f.is_empty());
        // Bounded: a runaway paste is refused rather than growing the buffer.
        assert!(!f.push_str(&"x".repeat(TOKEN_CAPACITY + 10)));
        assert_eq!(f.value.len(), TOKEN_CAPACITY);
        assert_eq!(f.value.capacity(), TOKEN_CAPACITY);
    }

    #[test]
    fn the_form_checks_what_the_host_needs() {
        let mut gh = TokenForm::new(&Host::GitHub);
        assert!(gh.submit().is_none() && gh.error.is_some());
        gh.next_focus();
        assert_eq!(gh.focus, FormFocus::Token, "GitHub has no email");
        let mut bb = TokenForm::new(&Host::BitbucketCloud);
        paste(&mut bb, " tok \n");
        assert!(bb.submit().is_none());
        assert!(bb.error.as_deref().unwrap().contains("email"));
        assert_eq!(bb.focus, FormFocus::Email);
        paste(&mut bb, "me@example.com");
        bb.field(&key("backspace"), None, &mut false);
        let s = bb.submit().unwrap();
        assert_eq!(s.email.as_deref(), Some("me@example.co"));
        assert_eq!(s.token.as_str(), "tok");
        // Nothing is typed into a form that is writing to the keychain.
        bb.focus = FormFocus::Token;
        paste(&mut bb, "more");
        assert!(bb.token.is_empty());
        let dc = TokenForm::new(&Host::BitbucketDc("git.example.com".into()));
        assert!(!dc.needs_email);
    }

    #[test]
    fn tabs_switch_by_digit() {
        assert_eq!(Tab::from_digit('1'), Some(Tab::Settings));
        assert_eq!(Tab::from_digit('4'), Some(Tab::Accounts));
        assert_eq!(Tab::from_digit('0'), None);
        assert_eq!(Tab::from_digit('5'), None);
    }
}
