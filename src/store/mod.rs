pub mod config;
pub mod fs;

use crate::error::Error;
use crate::model::{RepoId, RepoState, STATE_SCHEMA};
use crate::target::dir_name;
use fs::{atomic_write, create_dir_all, io_error, tree_size, validate_file_name};
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const APP_DIR: &str = "stargauge";
const CONFIG_FILE: &str = "config.toml";
const REPOS_DIR: &str = "repos";
const ASSETS_DIR: &str = "assets";
const STATE_FILE: &str = "state.json";
const MARKER_FILE: &str = ".stargauge";
const MARKER_CONTENTS: &str = "This directory holds stargauge data.\n";
const MAX_STATE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub data: PathBuf,
    pub config_file: PathBuf,
}

impl Layout {
    pub fn discover() -> Result<Layout, Error> {
        layout_from(
            std::env::var_os("STARGAUGE_HOME"),
            dirs::data_local_dir(),
            dirs::config_local_dir(),
        )
    }

    pub fn from_root(root: &Path) -> Layout {
        Layout {
            data: root.to_path_buf(),
            config_file: root.join(CONFIG_FILE),
        }
    }

    pub fn repo_dir(&self, id: &RepoId) -> PathBuf {
        self.data
            .join(REPOS_DIR)
            .join(dir_name(&id.owner))
            .join(dir_name(&id.name))
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.data.join(ASSETS_DIR)
    }

    fn marker_path(&self) -> PathBuf {
        self.data.join(MARKER_FILE)
    }
}

fn layout_from(
    home: Option<OsString>,
    data: Option<PathBuf>,
    config: Option<PathBuf>,
) -> Result<Layout, Error> {
    if let Some(home) = home.filter(|h| !h.is_empty()) {
        return Ok(Layout::from_root(Path::new(&home)));
    }
    let data = data.ok_or_else(|| {
        Error::Config("cannot determine the data directory, set STARGAUGE_HOME".into())
    })?;
    let config = config.ok_or_else(|| {
        Error::Config("cannot determine the config directory, set STARGAUGE_HOME".into())
    })?;
    Ok(Layout {
        data: data.join(APP_DIR),
        config_file: config.join(APP_DIR).join(CONFIG_FILE),
    })
}

fn state_path(layout: &Layout, id: &RepoId) -> PathBuf {
    layout.repo_dir(id).join(STATE_FILE)
}

fn marker_exists(layout: &Layout) -> Result<bool, Error> {
    let path = layout.marker_path();
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => Ok(true),
        Ok(_) => Err(Error::Input(format!(
            "{} is not a regular file, remove it",
            path.display()
        ))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(io_error(&path, source)),
    }
}

fn data_dirs_exist(layout: &Layout) -> Result<bool, Error> {
    for name in [REPOS_DIR, ASSETS_DIR] {
        let existing = layout.data.join(name);
        match std::fs::symlink_metadata(&existing) {
            Ok(_) => return Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(io_error(&existing, source)),
        }
    }
    Ok(false)
}

fn refuse_unmarked(layout: &Layout) -> Result<(), Error> {
    if data_dirs_exist(layout)? && !marker_exists(layout)? {
        return Err(foreign_directory(layout, "use"));
    }
    Ok(())
}

pub fn check_data_dir(layout: &Layout) -> Result<(), Error> {
    if marker_exists(layout)? {
        return Ok(());
    }
    refuse_unmarked(layout)
}

fn ensure_marker(layout: &Layout) -> Result<(), Error> {
    create_dir_all(&layout.data)?;
    if marker_exists(layout)? {
        return Ok(());
    }
    refuse_unmarked(layout)?;
    let path = layout.marker_path();
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            file.write_all(MARKER_CONTENTS.as_bytes())
                .map_err(|source| io_error(&path, source))?;
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(source) => Err(io_error(&path, source)),
    }
}

fn foreign_directory(layout: &Layout, action: &str) -> Error {
    Error::Input(format!(
        "{} was not created by stargauge, refusing to {action} it",
        layout.data.display()
    ))
}

fn has_marker(layout: &Layout) -> Result<bool, Error> {
    let path = layout.marker_path();
    match std::fs::symlink_metadata(&path) {
        Ok(meta) => Ok(meta.is_file()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(io_error(&path, source)),
    }
}

fn read_state(path: &Path) -> Result<Option<RepoState>, Error> {
    let invalid = |message: &str| Error::State {
        path: path.to_path_buf(),
        message: message.to_string(),
    };
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(io_error(path, source)),
    };
    if !meta.is_file() {
        return Err(invalid("not a regular file"));
    }
    if meta.len() > MAX_STATE_BYTES {
        return Err(invalid("state file is too large"));
    }
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(io_error(path, source)),
    };
    let mut bytes = Vec::new();
    file.take(MAX_STATE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| io_error(path, source))?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(invalid("state file is too large"));
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| invalid(&e.to_string()))?;
    let schema = value
        .get("schema")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| invalid("missing or invalid \"schema\""))?;
    if schema == 0 {
        return Err(invalid("invalid schema"));
    }
    if schema > u64::from(STATE_SCHEMA) {
        return Err(invalid("created by a newer stargauge"));
    }
    let state = serde_json::from_value(value).map_err(|e| invalid(&e.to_string()))?;
    Ok(Some(state))
}

pub fn load_state(layout: &Layout, id: &RepoId) -> Result<Option<RepoState>, Error> {
    let path = state_path(layout, id);
    let Some(state) = read_state(&path)? else {
        return Ok(None);
    };
    if !state.repo.eq_ignore_ascii_case(&id.full_name()) {
        return Err(Error::State {
            path,
            message: format!("belongs to another repository ({})", state.repo),
        });
    }
    Ok(Some(state))
}

pub fn save_state(layout: &Layout, id: &RepoId, state: &RepoState) -> Result<(), Error> {
    save_state_limited(layout, id, state, MAX_STATE_BYTES)
}

fn save_state_limited(
    layout: &Layout,
    id: &RepoId,
    state: &RepoState,
    max: u64,
) -> Result<(), Error> {
    let path = state_path(layout, id);
    let bytes = serde_json::to_vec_pretty(state).map_err(|e| Error::State {
        path: path.clone(),
        message: e.to_string(),
    })?;
    if bytes.len() as u64 > max {
        return Err(Error::State {
            path,
            message: format!(
                "the state would exceed {max} bytes, remove old data with `stargauge clear`"
            ),
        });
    }
    ensure_marker(layout)?;
    atomic_write(&path, &bytes)
}

fn read_dir_sorted(dir: &Path) -> Result<Vec<PathBuf>, Error> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(io_error(dir, source)),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| io_error(dir, source))?;
        let is_dir = entry
            .file_type()
            .map_err(|source| io_error(&entry.path(), source))?
            .is_dir();
        if is_dir {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

pub fn list_states(layout: &Layout) -> Result<Vec<RepoState>, Error> {
    let mut states = Vec::new();
    for owner_dir in read_dir_sorted(&layout.data.join(REPOS_DIR))? {
        let repo_dirs = match read_dir_sorted(&owner_dir) {
            Ok(dirs) => dirs,
            Err(e) => {
                warn(&format!("skipping {}", skip_reason(&e)));
                continue;
            }
        };
        for repo_dir in repo_dirs {
            match read_state(&repo_dir.join(STATE_FILE)) {
                Ok(Some(state)) => states.push(state),
                Ok(None) => {}
                Err(e) => warn(&format!("skipping {}", skip_reason(&e))),
            }
        }
    }
    Ok(states)
}

pub(crate) fn warn(message: &str) {
    let _ = writeln!(io::stderr(), "warning: {message}");
}

fn skip_reason(error: &Error) -> String {
    match error {
        Error::State { path, message } => format!("{}: {message}", path.display()),
        Error::Io { path, source } => format!("{}: {source}", path.display()),
        other => other.to_string(),
    }
}

pub fn write_page(
    layout: &Layout,
    id: &RepoId,
    file: &str,
    contents: &str,
) -> Result<PathBuf, Error> {
    validate_file_name(file)?;
    ensure_marker(layout)?;
    let dir = layout.repo_dir(id);
    create_dir_all(&dir)?;
    let path = dir.join(file);
    atomic_write(&path, contents.as_bytes())?;
    Ok(path)
}

pub fn ensure_assets(layout: &Layout, files: &[(&str, &[u8])]) -> Result<(), Error> {
    for (name, _) in files {
        validate_file_name(name)?;
    }
    ensure_marker(layout)?;
    let dir = layout.assets_dir();
    create_dir_all(&dir)?;
    for (name, bytes) in files {
        let path = dir.join(name);
        let up_to_date = match std::fs::metadata(&path) {
            Ok(meta) => meta.is_file() && meta.len() == bytes.len() as u64,
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(source) => return Err(io_error(&path, source)),
        };
        if !up_to_date {
            atomic_write(&path, bytes)?;
        }
    }
    Ok(())
}

pub fn clear_data(layout: &Layout) -> Result<u64, Error> {
    let mut targets = Vec::new();
    for name in [REPOS_DIR, ASSETS_DIR] {
        let path = layout.data.join(name);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) => targets.push((path, meta.is_dir())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(io_error(&path, source)),
        }
    }
    if targets.is_empty() {
        return Ok(0);
    }
    if !has_marker(layout)? {
        return Err(foreign_directory(layout, "delete"));
    }
    let mut freed = 0;
    for (path, is_dir) in targets {
        let size = tree_size(&path);
        let removed = if is_dir {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        removed.map_err(|source| io_error(&path, source))?;
        freed += size;
    }
    Ok(freed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Source;
    use chrono::{TimeZone, Utc};

    fn id(owner: &str, name: &str) -> RepoId {
        RepoId {
            owner: owner.into(),
            name: name.into(),
        }
    }

    fn state(schema: u32) -> RepoState {
        RepoState {
            schema,
            repo: "Alice/Tool".into(),
            scanned_at: Utc.with_ymd_and_hms(2026, 3, 15, 10, 0, 0).unwrap(),
            stars_total: 2,
            logins: vec!["a".into(), "b".into()],
            events: Vec::new(),
            source: Source::List,
            star_days: None,
        }
    }

    #[test]
    fn schema_1_state_without_new_fields_loads_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::from_root(dir.path());
        let target = layout.repo_dir(&id("a", "b"));
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(
            target.join(STATE_FILE),
            r#"{
                "schema": 1,
                "repo": "a/b",
                "scanned_at": "2026-03-15T10:00:00Z",
                "stars_total": 1,
                "logins": ["a"],
                "events": [{
                    "scanned_at": "2026-03-15T10:00:00Z",
                    "prev_scanned_at": "2026-03-14T10:00:00Z",
                    "stars_total": 1,
                    "added": ["a"],
                    "removed": []
                }]
            }"#,
        )
        .unwrap();
        let loaded = load_state(&layout, &id("a", "b")).unwrap().unwrap();
        assert_eq!(loaded.source, Source::List);
        assert_eq!(loaded.star_days, None);
        assert_eq!(loaded.events[0].counts, None);
    }

    #[test]
    fn state_round_trips_and_missing_state_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::from_root(dir.path());
        let repo = id("Alice", "Tool");
        assert_eq!(load_state(&layout, &repo).unwrap(), None);
        assert!(!layout.data.join(REPOS_DIR).exists());
        save_state(&layout, &repo, &state(STATE_SCHEMA)).unwrap();
        assert_eq!(
            load_state(&layout, &repo).unwrap(),
            Some(state(STATE_SCHEMA))
        );
        assert!(dir.path().join("repos/alice/tool/state.json").is_file());
    }

    #[test]
    fn newer_schema_is_rejected_and_skipped_in_listing() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::from_root(dir.path());
        save_state(&layout, &id("a", "old"), &state(STATE_SCHEMA)).unwrap();
        let newer = layout.repo_dir(&id("a", "new"));
        std::fs::create_dir_all(&newer).unwrap();
        std::fs::write(newer.join(STATE_FILE), r#"{"schema": 99, "other": true}"#).unwrap();
        let broken = layout.repo_dir(&id("b", "broken"));
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join(STATE_FILE), "{not json").unwrap();
        let err = load_state(&layout, &id("a", "new")).unwrap_err();
        assert!(
            matches!(&err, Error::State { message, .. } if message.contains("created by a newer stargauge")),
            "{err}"
        );
        let err = load_state(&layout, &id("b", "broken")).unwrap_err();
        assert!(matches!(&err, Error::State { path, .. } if path.ends_with("state.json")));
        assert_eq!(list_states(&layout).unwrap(), vec![state(STATE_SCHEMA)]);
    }

    #[test]
    fn write_page_rejects_paths_and_assets_are_rewritten_on_size_change() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::from_root(dir.path());
        let repo = id("a", "b");
        assert!(matches!(
            write_page(&layout, &repo, "../x.html", "x"),
            Err(Error::Input(_))
        ));
        assert!(!layout.data.join(REPOS_DIR).exists());
        assert!(!dir.path().join(".stargauge").exists());
        let page = write_page(&layout, &repo, "new.html", "<p>").unwrap();
        assert_eq!(std::fs::read_to_string(page).unwrap(), "<p>");
        ensure_assets(&layout, &[("a.js", b"one")]).unwrap();
        ensure_assets(&layout, &[("a.js", b"three")]).unwrap();
        assert_eq!(
            std::fs::read(layout.assets_dir().join("a.js")).unwrap(),
            b"three"
        );
    }

    #[test]
    fn clear_data_removes_only_repos_and_assets_and_keeps_config() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::from_root(dir.path());
        let repo = id("a", "b");
        save_state(&layout, &repo, &state(STATE_SCHEMA)).unwrap();
        ensure_assets(&layout, &[("a.js", b"12345")]).unwrap();
        std::fs::write(&layout.config_file, "[github]\ntoken = \"\"\n").unwrap();
        std::fs::write(dir.path().join("keep.txt"), "x").unwrap();
        let state_size = std::fs::metadata(layout.repo_dir(&repo).join(STATE_FILE))
            .unwrap()
            .len();
        assert_eq!(clear_data(&layout).unwrap(), state_size + 5);
        assert!(layout.config_file.is_file());
        assert!(dir.path().join(MARKER_FILE).is_file());
        assert!(dir.path().join("keep.txt").is_file());
        assert!(!dir.path().join(REPOS_DIR).exists());
        assert!(!layout.assets_dir().exists());
        assert_eq!(clear_data(&layout).unwrap(), 0);
    }

    #[test]
    fn foreign_directories_are_never_marked_or_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::from_root(dir.path());
        assert_eq!(clear_data(&layout).unwrap(), 0);
        let foreign = dir.path().join("repos").join("mine");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(foreign.join("work.txt"), "x").unwrap();
        let err = clear_data(&layout).unwrap_err();
        assert!(
            matches!(&err, Error::Input(m) if m.contains("was not created by stargauge")),
            "{err}"
        );
        let err = save_state(&layout, &id("a", "b"), &state(STATE_SCHEMA)).unwrap_err();
        assert!(
            matches!(&err, Error::Input(m) if m.ends_with("was not created by stargauge, refusing to use it")),
            "{err}"
        );
        assert!(write_page(&layout, &id("a", "b"), "new.html", "x").is_err());
        assert!(ensure_assets(&layout, &[("a.js", b"1")]).is_err());
        assert!(!dir.path().join(MARKER_FILE).exists());
        assert!(clear_data(&layout).is_err());
        assert!(foreign.join("work.txt").is_file());
    }

    #[test]
    fn fresh_directory_is_marked_and_clear_removes_only_its_own_data() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::from_root(dir.path());
        std::fs::write(dir.path().join("keep.txt"), "x").unwrap();
        save_state(&layout, &id("a", "b"), &state(STATE_SCHEMA)).unwrap();
        assert!(dir.path().join(MARKER_FILE).is_file());
        save_state(&layout, &id("a", "c"), &state(STATE_SCHEMA)).unwrap();
        assert!(clear_data(&layout).unwrap() >= 1);
        assert!(!dir.path().join(REPOS_DIR).exists());
        assert!(dir.path().join(MARKER_FILE).is_file());
        assert!(dir.path().join("keep.txt").is_file());
    }

    #[test]
    fn marker_must_be_a_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::from_root(dir.path());
        std::fs::create_dir(dir.path().join(MARKER_FILE)).unwrap();
        let err = save_state(&layout, &id("a", "b"), &state(STATE_SCHEMA)).unwrap_err();
        assert!(
            matches!(&err, Error::Input(m) if m.ends_with("is not a regular file, remove it")),
            "{err}"
        );
        assert!(!dir.path().join(REPOS_DIR).exists());
    }

    #[cfg(unix)]
    #[test]
    fn marker_symlink_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::from_root(dir.path());
        let target = dir.path().join("target");
        std::fs::write(&target, "x").unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join(MARKER_FILE)).unwrap();
        let err = save_state(&layout, &id("a", "b"), &state(STATE_SCHEMA)).unwrap_err();
        assert!(
            matches!(&err, Error::Input(m) if m.ends_with("is not a regular file, remove it")),
            "{err}"
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "x");
    }

    #[test]
    fn state_of_another_repository_is_refused_on_load_but_listed() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::from_root(dir.path());
        save_state(&layout, &id("alice", "tool"), &state(STATE_SCHEMA)).unwrap();
        let target = layout.repo_dir(&id("a", "b"));
        std::fs::create_dir_all(&target).unwrap();
        let mut foreign = state(STATE_SCHEMA);
        foreign.repo = "x/y".into();
        std::fs::write(
            target.join(STATE_FILE),
            serde_json::to_vec(&foreign).unwrap(),
        )
        .unwrap();
        let err = load_state(&layout, &id("a", "b")).unwrap_err();
        assert!(
            matches!(&err, Error::State { message, .. } if message == "belongs to another repository (x/y)"),
            "{err}"
        );
        assert!(load_state(&layout, &id("alice", "tool")).unwrap().is_some());
        assert!(list_states(&layout).unwrap().contains(&foreign));
    }

    #[test]
    fn state_file_must_be_regular_bounded_and_have_a_valid_schema() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::from_root(dir.path());
        let message = |name: &str| {
            let err = load_state(&layout, &id("a", name)).unwrap_err();
            match err {
                Error::State { message, .. } => message,
                other => panic!("{other}"),
            }
        };
        let make = |name: &str| {
            let repo = layout.repo_dir(&id("a", name));
            std::fs::create_dir_all(&repo).unwrap();
            repo.join(STATE_FILE)
        };
        std::fs::create_dir(make("dir")).unwrap();
        assert_eq!(message("dir"), "not a regular file");
        std::fs::write(make("zero"), r#"{"schema": 0}"#).unwrap();
        assert_eq!(message("zero"), "invalid schema");
        assert_eq!(MAX_STATE_BYTES, 32 * 1024 * 1024);
        let huge = std::fs::File::create(make("huge")).unwrap();
        huge.set_len(MAX_STATE_BYTES + 1).unwrap();
        assert_eq!(message("huge"), "state file is too large");
    }

    #[test]
    fn oversized_state_is_refused_and_the_previous_file_stays() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::from_root(dir.path());
        let repo = id("a", "b");
        save_state(&layout, &repo, &state(STATE_SCHEMA)).unwrap();
        let path = state_path(&layout, &repo);
        let before = std::fs::read(&path).unwrap();
        let limit = before.len() as u64;
        save_state_limited(&layout, &repo, &state(STATE_SCHEMA), limit).unwrap();
        let mut bigger = state(STATE_SCHEMA);
        bigger.logins.push("c".into());
        let err = save_state_limited(&layout, &repo, &bigger, limit).unwrap_err();
        assert!(
            matches!(&err, Error::State { message, .. } if message.contains("would exceed") && message.contains("stargauge clear")),
            "{err}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn check_data_dir_only_reads_and_refuses_unmarked_data() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::from_root(dir.path());
        check_data_dir(&layout).unwrap();
        assert!(!dir.path().join(MARKER_FILE).exists());
        std::fs::create_dir(dir.path().join(ASSETS_DIR)).unwrap();
        let err = check_data_dir(&layout).unwrap_err();
        assert!(
            matches!(&err, Error::Input(m) if m.ends_with("was not created by stargauge, refusing to use it")),
            "{err}"
        );
        assert!(!dir.path().join(MARKER_FILE).exists());
        std::fs::write(dir.path().join(MARKER_FILE), MARKER_CONTENTS).unwrap();
        check_data_dir(&layout).unwrap();
        std::fs::remove_file(dir.path().join(MARKER_FILE)).unwrap();
        std::fs::create_dir(dir.path().join(MARKER_FILE)).unwrap();
        assert!(
            matches!(check_data_dir(&layout), Err(Error::Input(m)) if m.ends_with("is not a regular file, remove it"))
        );
    }

    #[test]
    fn layout_prefers_home_and_fails_without_directories() {
        let home = layout_from(Some("/h".into()), None, None).unwrap();
        assert_eq!(home, Layout::from_root(Path::new("/h")));
        let normal = layout_from(None, Some("/d".into()), Some("/c".into())).unwrap();
        assert_eq!(normal.data, Path::new("/d/stargauge"));
        assert_eq!(normal.config_file, Path::new("/c/stargauge/config.toml"));
        assert!(matches!(
            layout_from(None, None, Some("/c".into())),
            Err(Error::Config(_))
        ));
    }
}
