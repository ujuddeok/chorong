//! Projects: remote repositories Chorong remembers, in `projects.json` in Chorong's config
//! directory (never in a repository), keyed by repository identity (`github:acme/api`).

use crate::{Error, Result};
use cho_core::RepoRef;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const PROJECTS_FILE: &str = "projects.json";
pub const DEFAULT_WORKTREE_CAP: usize = 10;

/// How clone URLs are formed when a project has no explicit URL.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    Https,
    Ssh,
}

impl Protocol {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "https" => Ok(Self::Https),
            "ssh" => Ok(Self::Ssh),
            _ => Err(Error::msg(format!(
                "unknown protocol `{s}` (expected ssh or https)"
            ))),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Https => "https",
            Self::Ssh => "ssh",
        }
    }
}

/// One remembered repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    #[serde(default)]
    pub protocol: Protocol,
    /// A clone URL that replaces the one built from the identity and protocol: a Data Center
    /// server under a context path, a mirror, or a local `file://` repository in tests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Worktrees kept before the least recently opened is removed.
    #[serde(default = "default_cap")]
    pub worktree_cap: usize,
    /// The project's own prepare recipe (highest priority).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<crate::prepare::Recipe>,
    /// The owner's approval for prepare and indexing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<crate::prepare::Approval>,
    // `auto_prepare` (prepare and index on open) was retired: nothing prepares or indexes
    // until the reader asks. An older file's `auto_prepare` is read and ignored, and dropped
    // on the next save (serde skips unknown fields).
}

fn default_cap() -> usize {
    DEFAULT_WORKTREE_CAP
}

impl Project {
    /// What `insert` insists on and `open` re-checks, so a hand-edited file can't hold a
    /// project that couldn't have been saved: a clone URL git accepts, a valid recipe, a
    /// worktree cap of at least one.
    pub fn validate(&self) -> Result<()> {
        if let Some(u) = &self.url {
            cho_core::git::check_remote_url(u).map_err(Error::msg)?;
        }
        if let Some(r) = &self.recipe {
            r.validate()?;
        }
        if self.worktree_cap == 0 {
            return Err(Error::msg("the worktree cap must be at least 1"));
        }
        Ok(())
    }
}

impl Default for Project {
    fn default() -> Self {
        Self {
            protocol: Protocol::Https,
            url: None,
            worktree_cap: DEFAULT_WORKTREE_CAP,
            recipe: None,
            approval: None,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    version: u32,
    #[serde(default)]
    projects: BTreeMap<String, Project>,
}

/// `projects.json`, loaded. Writes go through a temporary file and a rename.
#[derive(Debug)]
pub struct Projects {
    path: PathBuf,
    projects: BTreeMap<String, Project>,
}

impl Projects {
    /// `projects.json` in Chorong's config directory (`CHORONG_CONFIG_DIR` in tests).
    pub fn open_default() -> Result<Self> {
        let dir = cho_settings::config_dir().ok_or_else(|| {
            Error::msg("no config directory (set CHORONG_CONFIG_DIR, XDG_CONFIG_HOME or HOME)")
        })?;
        Self::open(dir.join(PROJECTS_FILE))
    }

    #[allow(clippy::disallowed_methods)] // Chorong's own projects.json in its config directory
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let projects = match std::fs::read_to_string(&path) {
            Ok(text) => {
                let f: File = serde_json::from_str(&text)
                    .map_err(|e| Error::msg(format!("{} is not valid: {e}", path.display())))?;
                f.projects
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(Error::msg(format!("cannot read {}: {e}", path.display()))),
        };
        // Keys are re-validated so a hand-edited file can't name a path outside the cache.
        for k in projects.keys() {
            let r = RepoRef::parse(k)
                .map_err(|e| Error::msg(format!("{}: `{k}`: {e}", path.display())))?;
            if matches!(r, RepoRef::Local { .. }) {
                return Err(Error::msg(format!(
                    "{}: `{k}` is a local folder, not a remote project",
                    path.display()
                )));
            }
        }
        for (k, p) in &projects {
            p.validate()
                .map_err(|e| Error::msg(format!("{}: `{k}`: {e}", path.display())))?;
        }
        Ok(Self { path, projects })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn list(&self) -> Vec<(RepoRef, &Project)> {
        self.projects
            .iter()
            .filter_map(|(k, p)| Some((RepoRef::parse(k).ok()?, p)))
            .collect()
    }

    pub fn get(&self, repo: &RepoRef) -> Option<&Project> {
        self.projects.get(&repo.to_string())
    }

    /// The project for `repo`, added with defaults (and `url`) when it isn't one yet: opening
    /// a repository is consent to remember it. The note to show when it was added.
    pub fn get_or_add(
        &mut self,
        repo: &RepoRef,
        url: Option<String>,
    ) -> Result<(Project, Option<String>)> {
        if let Some(p) = self.get(repo) {
            return Ok((p.clone(), None));
        }
        let p = Project {
            url,
            ..Project::default()
        };
        self.insert(repo, p.clone())?;
        Ok((p, Some(format!("Added {repo} to your projects."))))
    }

    /// Adds or replaces a project. Returns true when it was new.
    pub fn insert(&mut self, repo: &RepoRef, project: Project) -> Result<bool> {
        if matches!(repo, RepoRef::Local { .. }) {
            return Err(Error::msg(format!(
                "`{repo}` is a local folder; projects are remote repositories"
            )));
        }
        project.validate()?;
        let new = self.projects.insert(repo.to_string(), project).is_none();
        self.save()?;
        Ok(new)
    }

    pub fn remove(&mut self, repo: &RepoRef) -> Result<bool> {
        let had = self.projects.remove(&repo.to_string()).is_some();
        if had {
            self.save()?;
        }
        Ok(had)
    }

    fn save(&self) -> Result<()> {
        let f = File {
            version: 1,
            projects: self.projects.clone(),
        };
        let text = serde_json::to_string_pretty(&f).expect("json") + "\n";
        crate::write_atomic(&self.path, text.as_bytes())
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // tests read their own fixtures
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chorong").join(PROJECTS_FILE);
        let mut p = Projects::open(&path).unwrap();
        assert!(p.list().is_empty());
        let r = RepoRef::parse("github:acme/api").unwrap();
        assert!(p.insert(&r, Project::default()).unwrap());
        assert!(
            !p.insert(
                &r,
                Project {
                    protocol: Protocol::Ssh,
                    ..Project::default()
                }
            )
            .unwrap()
        );
        let p = Projects::open(&path).unwrap();
        assert_eq!(p.get(&r).unwrap().protocol, Protocol::Ssh);
        assert_eq!(p.list().len(), 1);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"github:acme/api\""), "{text}");
    }

    #[test]
    fn opening_adds_the_project_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PROJECTS_FILE);
        let mut p = Projects::open(&path).unwrap();
        let r = RepoRef::parse("github:acme/api").unwrap();
        let url = Some("https://mirror.example/acme/api.git".to_string());
        let (got, note) = p.get_or_add(&r, url.clone()).unwrap();
        assert_eq!(
            (got.url, note.as_deref()),
            (url.clone(), Some("Added github:acme/api to your projects."))
        );
        // Already a project: kept as it is, no note.
        let (got, note) = p.get_or_add(&r, None).unwrap();
        assert_eq!((got.url, note), (url, None));
        assert_eq!(Projects::open(&path).unwrap().list().len(), 1);
    }

    #[test]
    fn refuses_local_and_bad_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PROJECTS_FILE);
        let mut p = Projects::open(&path).unwrap();
        assert!(
            p.insert(&RepoRef::parse("local:/x").unwrap(), Project::default())
                .is_err()
        );
        std::fs::write(&path, r#"{"version":1,"projects":{"github:../x/y":{}}}"#).unwrap();
        assert!(Projects::open(&path).is_err());
    }

    /// `auto_prepare` was retired: an older file that has it loads (even without an
    /// approval), the value is ignored, and the next save drops it.
    #[test]
    fn retired_auto_prepare_is_read_ignored_and_dropped_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PROJECTS_FILE);
        std::fs::write(
            &path,
            r#"{"version":1,"projects":{"github:acme/api":{"auto_prepare":true}}}"#,
        )
        .unwrap();
        let mut p = Projects::open(&path).unwrap();
        let r = RepoRef::parse("github:acme/api").unwrap();
        let project = p.get(&r).cloned().unwrap();
        assert_eq!(project, Project::default());
        p.insert(&r, project).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("auto_prepare"), "{text}");
    }

    /// Found by the `persist_state` fuzz target: a hand-edited file could hold a project
    /// that `insert` refuses (an option-like clone URL, a cap of 0), and it loaded.
    #[test]
    fn open_applies_the_checks_insert_does() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PROJECTS_FILE);
        for bad in [r#"{"url":"--upload-pack=x"}"#, r#"{"worktree_cap":0}"#] {
            std::fs::write(
                &path,
                format!(r#"{{"version":1,"projects":{{"github:acme/api":{bad}}}}}"#),
            )
            .unwrap();
            let e = Projects::open(&path).unwrap_err().to_string();
            assert!(e.contains("github:acme/api"), "{e}");
        }
    }
}
