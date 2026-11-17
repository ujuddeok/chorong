//! The Open Remote form's state: which project, what to open, what to compare against, and the
//! hint line under the box. Pure: the hint is computed on every keystroke from the text and the
//! chosen project, with no network, and branch completion works on a list the caller loaded.

use super::adapter::{self, Request};
use super::prepare::Prefs;
use crate::picker::Ranker;
use cho_core::RepoRef;
use cho_sources::target::check_branch_name;
use cho_sources::{Target, TargetKind, parse_repo, parse_target};

/// Completions shown at most.
pub const COMPLETIONS: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Project,
    /// The Data Center clone URL (only for a new Data Center project).
    CloneUrl,
    Input,
    Base,
    /// The "Prepare and index" checkbox.
    Prepare,
}

/// The project the form will open in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chosen {
    pub repo: RepoRef,
    /// A clone URL the pasted URL gave (used only when the project is new).
    pub clone_url: Option<String>,
    /// Already a remembered project; otherwise it is added on open.
    pub known: bool,
}

/// A row of the project list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectRow {
    Known(RepoRef),
    /// The typed URL names a repository Chorong does not know yet.
    Add(RepoRef, Option<String>),
    /// The placeholder entry: type or paste a repository URL.
    AddFromUrl,
}

impl ProjectRow {
    pub fn label(&self) -> String {
        match self {
            ProjectRow::Known(r) => r.to_string(),
            ProjectRow::Add(r, _) => format!("Add {r}"),
            ProjectRow::AddFromUrl => "Add from URL\u{2026}".into(),
        }
    }
}

/// The line under the input box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub text: String,
    pub error: bool,
}

pub struct Form {
    pub known: Vec<RepoRef>,
    pub chosen: Option<Chosen>,
    pub project_query: String,
    pub project_sel: usize,
    pub input: String,
    pub base: String,
    pub field: Field,
    /// The last URL typed in the box named another repository and moved the project row.
    pub switched: bool,
    /// Branches of the chosen project's clone, when cached.
    pub branches: Vec<String>,
    /// The highlighted completion of the focused box, if the reader moved onto one.
    pub completion: Option<usize>,
    /// The clone URL of a new Bitbucket Data Center project: derived from the pasted URL,
    /// editable, and checked when the form is submitted. Empty for other projects.
    pub clone_url: String,
    /// The remote's default branch, when the remote was asked (no clone yet).
    pub remote_default: Option<String>,
    /// Stops the remote branch listing when the form closes.
    pub cancel: adapter::Cancel,
    /// Projects whose remote branches were asked for in this opening of the form (once each).
    pub asked: Vec<RepoRef>,
    /// "Prepare and index": checked by default, remembered per project in `remote.json`.
    pub prepare: bool,
    /// What `remote.json` held when the form opened (updated by the form's own choices).
    pub prefs: Prefs,
    /// The project `clone_url` was filled for.
    clone_for: Option<RepoRef>,
}

impl Form {
    pub fn new(known: Vec<RepoRef>, last: Option<&RepoRef>) -> Self {
        let pick = last
            .filter(|l| known.contains(l))
            .or_else(|| (known.len() == 1).then(|| &known[0]))
            .cloned();
        let chosen = pick.map(|repo| Chosen {
            repo,
            clone_url: None,
            known: true,
        });
        Self {
            field: if chosen.is_some() {
                Field::Input
            } else {
                Field::Project
            },
            known,
            chosen,
            project_query: String::new(),
            project_sel: 0,
            input: String::new(),
            base: String::new(),
            switched: false,
            branches: Vec::new(),
            completion: None,
            clone_url: String::new(),
            asked: Vec::new(),
            remote_default: None,
            cancel: adapter::Cancel::new(),
            prepare: false,
            prefs: Prefs::default(),
            clone_for: None,
        }
    }

    /// Takes what `remote.json` remembers (the last project).
    pub fn set_prefs(&mut self, prefs: Prefs) {
        self.prefs = prefs;
    }

    /// Space on the checkbox, or a click. Ticking it is the click that starts prepare and
    /// indexing for this open only; it is never remembered.
    pub fn toggle_prepare(&mut self) {
        self.prepare = !self.prepare;
    }

    /// Whether the form shows the clone URL box: a Data Center project Chorong does not know yet
    /// (GitHub and Bitbucket Cloud clone URLs follow from the name).
    pub fn needs_clone_url(&self) -> bool {
        self.chosen
            .as_ref()
            .is_some_and(|c| !c.known && matches!(c.repo, RepoRef::BitbucketDc { .. }))
    }

    /// The fields Tab visits, in order.
    pub fn fields(&self) -> Vec<Field> {
        let mut f = vec![Field::Project];
        if self.needs_clone_url() {
            f.push(Field::CloneUrl);
        }
        f.extend([Field::Input, Field::Base, Field::Prepare]);
        f
    }

    /// Prefills the clone URL when the chosen project changed: the URL the paste gave, else
    /// the one derived from the name. Keeps what the reader typed for the same project.
    fn sync_clone_url(&mut self) {
        if !self.needs_clone_url() {
            self.clone_url.clear();
            return;
        }
        let c = self.chosen.as_ref().expect("needs_clone_url");
        if self.clone_url.is_empty() || self.clone_for.as_ref() != Some(&c.repo) {
            self.clone_url = c
                .clone_url
                .clone()
                .or_else(|| adapter::derived_clone_url(&c.repo))
                .unwrap_or_default();
        }
        self.clone_for = Some(c.repo.clone());
    }

    /// Prefills the box with `text` when it is something `parse_target` accepts and names
    /// something (a URL), and returns whether it did. It never acts on it.
    pub fn prefill(&mut self, text: &str) -> bool {
        let t = text.trim();
        let is_url =
            t.starts_with("http://") || t.starts_with("https://") || t.starts_with("chorong://");
        if !is_url || parse_target(t).is_err() {
            return false;
        }
        self.input = t.to_string();
        self.input_changed();
        true
    }

    // ---- Project row ----

    /// Known projects matching the typed text, then the entry that adds one from a URL.
    pub fn project_rows(&self) -> Vec<ProjectRow> {
        let q = self.project_query.trim().to_lowercase();
        let mut rows: Vec<ProjectRow> = self
            .known
            .iter()
            .filter(|r| q.is_empty() || r.to_string().to_lowercase().contains(&q))
            .cloned()
            .map(ProjectRow::Known)
            .collect();
        match parse_repo(&self.project_query) {
            Ok((repo, url)) if !self.known.contains(&repo) => rows.push(ProjectRow::Add(repo, url)),
            Ok(_) => {}
            Err(_) => rows.push(ProjectRow::AddFromUrl),
        }
        rows
    }

    /// Takes the highlighted row as the project. `AddFromUrl` only asks for a URL: it changes
    /// nothing and returns false.
    pub fn choose_row(&mut self, row: &ProjectRow) -> bool {
        let chosen = match row {
            ProjectRow::Known(r) => Chosen {
                repo: r.clone(),
                clone_url: None,
                known: true,
            },
            ProjectRow::Add(r, url) => Chosen {
                repo: r.clone(),
                clone_url: url.clone(),
                known: false,
            },
            ProjectRow::AddFromUrl => return false,
        };
        self.project_query.clear();
        self.project_sel = 0;
        self.set_chosen(chosen, false);
        true
    }

    /// The project changed: everything that belongs to the previous one goes.
    fn set_chosen(&mut self, chosen: Chosen, switched: bool) {
        self.chosen = Some(chosen);
        self.switched = switched;
        self.branches.clear();
        self.remote_default = None;
        self.sync_clone_url();
    }

    // ---- The box ----

    /// Call after the input text changed: a URL that names another repository moves the
    /// project row. Returns true when it did (the caller reloads the branch list).
    pub fn input_changed(&mut self) -> bool {
        self.completion = None;
        let Ok(t) = parse_target(&self.input) else {
            return false;
        };
        let Some(repo) = t.repo else { return false };
        if self.chosen.as_ref().is_some_and(|c| c.repo == repo) {
            return false;
        }
        let chosen = Chosen {
            known: self.known.contains(&repo),
            repo,
            clone_url: t.clone_url,
        };
        self.set_chosen(chosen, true);
        true
    }

    /// What the box will open, in words.
    pub fn hint(&self) -> Hint {
        let ok = |text: String| Hint { text, error: false };
        let err = |text: String| Hint { text, error: true };
        let t: Target = match parse_target(&self.input) {
            Ok(t) => t,
            Err(e) => return err(e.to_string()),
        };
        let Some(c) = &self.chosen else {
            return err("Choose a project first".into());
        };
        let repo = &c.repo;
        let mut text = match &t.kind {
            TargetKind::Default => format!("Opens the default branch of {repo}"),
            TargetKind::Branch(b) => {
                if !self.branches.is_empty() && !self.branches.contains(b) {
                    format!(
                        "Opens branch {b} of {repo} (not among the branches Chorong has; \
                         it will ask the remote)"
                    )
                } else {
                    format!("Opens branch {b} of {repo}")
                }
            }
            TargetKind::PullRequest(n) => format!("Opens pull request #{n} in {repo}"),
            TargetKind::Commit(s) => format!("Opens commit {}", cho_git::short(s)),
            TargetKind::BranchOrCommit(s) => {
                if self.branches.iter().any(|b| b == s) {
                    format!(
                        "{s} is a branch and could be a commit: Chorong will ask which of \
                         {repo} you mean"
                    )
                } else {
                    format!(
                        "Opens commit {} (or a branch of that name \u{2014} Chorong will ask)",
                        cho_git::short(s)
                    )
                }
            }
            TargetKind::RefPath(p) => format!("Opens the branch or commit named in {p}"),
        };
        if let Some(path) = &t.path {
            text.push_str(&format!(", file {path}"));
            if let Some(line) = t.line {
                text.push_str(&format!(" line {line}"));
            }
        }
        if self.switched {
            text.push_str(&format!(". The project is now {repo}, from the URL"));
        }
        if !c.known {
            text.push_str(". Chorong will add the project");
        }
        ok(text)
    }

    /// What "Compare against" does when left empty.
    pub fn base_default(&self) -> String {
        match parse_target(&self.input).map(|t| t.kind) {
            Ok(TargetKind::PullRequest(_)) => "the pull request's target".into(),
            _ => match &self.remote_default {
                Some(d) => format!("default branch ({d})"),
                None => "default branch".into(),
            },
        }
    }

    // ---- Completion ----

    /// Branches matching what the focused box holds, best first.
    pub fn completions(&self, ranker: &mut Ranker) -> Vec<String> {
        let query = match self.field {
            Field::Input => {
                let q = self.input.trim();
                if q.is_empty() || q.starts_with('#') || q.contains("://") {
                    return Vec::new();
                }
                q
            }
            Field::Base => self.base.trim(),
            Field::Project | Field::CloneUrl | Field::Prepare => return Vec::new(),
        };
        ranker
            .rank(&self.branches, query, COMPLETIONS)
            .into_iter()
            .map(|m| self.branches[m.index].clone())
            .filter(|b| b != query)
            .collect()
    }

    // ---- Open ----

    /// The request Enter sends, or the reason it cannot.
    pub fn request(&self) -> Result<Request, String> {
        let t = parse_target(&self.input).map_err(|e| e.to_string())?;
        let c = self.chosen.as_ref().ok_or("Choose a project first")?;
        let base = self.base.trim();
        if !base.is_empty() {
            check_branch_name(base).map_err(|e| e.to_string())?;
        }
        let clone_url = if c.known {
            None
        } else if self.needs_clone_url() {
            let u = self.clone_url.trim();
            cho_core::git::check_remote_url(u).map_err(|e| format!("Clone URL: {e}"))?;
            Some(u.to_string())
        } else {
            t.clone_url.or(c.clone_url.clone())
        };
        Ok(Request {
            repo: c.repo.clone(),
            clone_url,
            kind: t.kind,
            path: t.path,
            line: t.line,
            base: (!base.is_empty()).then(|| base.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(s: &str) -> RepoRef {
        RepoRef::parse(s).unwrap()
    }

    fn form() -> Form {
        let mut f = Form::new(
            vec![repo("github:acme/api"), repo("github:acme/web")],
            Some(&repo("github:acme/api")),
        );
        f.branches = vec!["main".into(), "feat/login".into(), "feat/logout".into()];
        f
    }

    fn hint(f: &mut Form, input: &str) -> String {
        f.input = input.into();
        f.input_changed();
        f.hint().text
    }

    #[test]
    fn last_used_project_is_preselected() {
        let f = form();
        assert_eq!(f.chosen.as_ref().unwrap().repo, repo("github:acme/api"));
        assert_eq!(f.field, Field::Input);
        let f = Form::new(vec![repo("github:a/b"), repo("github:c/d")], None);
        assert!(f.chosen.is_none());
        assert_eq!(f.field, Field::Project);
    }

    #[test]
    fn prepare_is_unchecked_by_default_and_never_remembered() {
        let mut f = form();
        assert!(!f.prepare, "unchecked by default");
        f.toggle_prepare();
        assert!(f.prepare);
        // Choosing another project, or coming back, doesn't change the box.
        f.choose_row(&ProjectRow::Known(repo("github:acme/web")));
        assert!(f.prepare);
        // A form opened later starts unchecked whatever was ticked before.
        let mut g = Form::new(
            vec![repo("github:acme/api")],
            Some(&repo("github:acme/api")),
        );
        g.set_prefs(f.prefs.clone());
        assert!(!g.prepare);
        assert!(g.fields().ends_with(&[Field::Base, Field::Prepare]));
    }

    #[test]
    fn hint_for_each_kind_of_target() {
        let mut f = form();
        assert_eq!(
            hint(&mut f, ""),
            "Opens the default branch of github:acme/api"
        );
        assert_eq!(
            hint(&mut f, "feat/login"),
            "Opens branch feat/login of github:acme/api"
        );
        assert_eq!(
            hint(&mut f, "#123"),
            "Opens pull request #123 in github:acme/api"
        );
        assert_eq!(
            hint(&mut f, "3f9c1e2"),
            "Opens commit 3f9c1e2 (or a branch of that name \u{2014} Chorong will ask)"
        );
        assert!(hint(&mut f, "nope/missing").contains("not among the branches"));
    }

    #[test]
    fn errors_are_marked_and_say_why() {
        let mut f = form();
        f.input = "bad..name".into();
        let h = f.hint();
        assert!(h.error, "{h:?}");
        f.input = "#0".into();
        assert!(f.hint().error);
        f.chosen = None;
        f.input = "main".into();
        assert_eq!(f.hint().text, "Choose a project first");
    }

    #[test]
    fn commit_that_is_also_a_branch_says_it_will_ask() {
        let mut f = form();
        f.branches.push("deadbeef".into());
        assert!(hint(&mut f, "deadbeef").contains("will ask which"));
    }

    #[test]
    fn a_commit_url_names_the_commit() {
        let mut f = form();
        let h = hint(
            &mut f,
            "https://github.com/acme/api/commit/3f9c1e2aabbccddeeff00112233445566778899",
        );
        assert!(h.starts_with("Opens commit 3f9c1e2"), "{h}");
    }

    #[test]
    fn a_url_for_another_repository_switches_the_project() {
        let mut f = form();
        f.input = "https://github.com/acme/web/pull/7".into();
        assert!(f.input_changed());
        assert_eq!(f.chosen.as_ref().unwrap().repo, repo("github:acme/web"));
        assert!(f.chosen.as_ref().unwrap().known);
        assert!(f.switched);
        let h = f.hint().text;
        assert!(
            h.starts_with("Opens pull request #7 in github:acme/web"),
            "{h}"
        );
        assert!(h.contains("The project is now github:acme/web"));
        // The same repository again moves nothing.
        assert!(!f.input_changed());
    }

    #[test]
    fn a_url_for_an_unknown_repository_adds_a_project() {
        let mut f = form();
        f.input = "https://github.com/other/thing/tree/main".into();
        f.input_changed();
        let c = f.chosen.clone().unwrap();
        assert_eq!(c.repo, repo("github:other/thing"));
        assert!(!c.known);
        assert!(f.hint().text.ends_with("Chorong will add the project"));
        let r = f.request().unwrap();
        assert_eq!(r.repo, repo("github:other/thing"));
    }

    #[test]
    fn project_list_filters_and_offers_add_from_url() {
        let mut f = form();
        assert_eq!(f.project_rows().len(), 3);
        assert_eq!(f.project_rows().last(), Some(&ProjectRow::AddFromUrl));
        f.project_query = "web".into();
        let rows = f.project_rows();
        assert_eq!(rows[0], ProjectRow::Known(repo("github:acme/web")));
        f.project_query = "https://github.com/x/y".into();
        let rows = f.project_rows();
        assert_eq!(rows, [ProjectRow::Add(repo("github:x/y"), None)]);
        assert!(f.choose_row(&rows[0]));
        assert!(!f.chosen.as_ref().unwrap().known);
        assert!(!f.choose_row(&ProjectRow::AddFromUrl));
    }

    #[test]
    fn a_data_center_url_shows_an_editable_clone_url_checked_on_submit() {
        let mut f = form();
        assert!(!f.needs_clone_url());
        f.input = "https://git.example.com/projects/PRJ/repos/svc/browse/a.rs".into();
        f.input_changed();
        assert!(f.needs_clone_url());
        assert_eq!(f.clone_url, "https://git.example.com/scm/PRJ/svc.git");
        assert_eq!(f.fields()[1], Field::CloneUrl);
        let r = f.request().unwrap();
        assert_eq!(
            r.clone_url.as_deref(),
            Some("https://git.example.com/scm/PRJ/svc.git")
        );
        // An edit (a context path) is what is sent; a bad URL stops the submit.
        f.clone_url = "https://git.example.com/stash/scm/PRJ/svc.git".into();
        assert_eq!(
            f.request().unwrap().clone_url.as_deref(),
            Some("https://git.example.com/stash/scm/PRJ/svc.git")
        );
        f.clone_url = "--upload-pack=x".into();
        assert!(f.request().unwrap_err().starts_with("Clone URL:"));
        // GitHub keeps it hidden.
        f.input = "https://github.com/other/thing/tree/main".into();
        f.input_changed();
        assert!(!f.needs_clone_url() && f.clone_url.is_empty());
        assert!(!f.fields().contains(&Field::CloneUrl));
    }

    #[test]
    fn compare_against_defaults_and_overrides() {
        let mut f = form();
        assert_eq!(f.base_default(), "default branch");
        f.remote_default = Some("trunk".into());
        assert_eq!(f.base_default(), "default branch (trunk)");
        f.input = "#5".into();
        assert_eq!(f.base_default(), "the pull request's target");
        assert_eq!(f.request().unwrap().base, None);
        f.base = " release/1.0 ".into();
        let r = f.request().unwrap();
        assert_eq!(r.base.as_deref(), Some("release/1.0"));
        assert_eq!(r.kind, TargetKind::PullRequest(5));
        f.base = "no spaces".into();
        assert!(f.request().is_err());
    }

    #[test]
    fn branches_complete_with_fuzzy_matching() {
        let mut f = form();
        let mut r = Ranker::paths();
        f.field = Field::Input;
        f.input = "flog".into();
        let mut got = f.completions(&mut r);
        got.sort();
        assert_eq!(got, ["feat/login", "feat/logout"]);
        f.input = "#12".into();
        assert!(f.completions(&mut r).is_empty());
        f.field = Field::Base;
        f.base = "mai".into();
        assert_eq!(f.completions(&mut r), ["main"]);
    }

    #[test]
    fn clipboard_prefill_takes_urls_only() {
        let mut f = form();
        assert!(!f.prefill("feat/login"));
        assert!(!f.prefill("https://example.com/not/a/host"));
        assert!(f.prefill(" https://github.com/acme/api/pull/9 \n"));
        assert_eq!(f.input, "https://github.com/acme/api/pull/9");
        assert!(!f.switched);
    }
}
