use crate::error::Error;
use crate::target::is_reserved_name;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const TEMP_ATTEMPTS: usize = 16;
const MAX_FILE_NAME_CHARS: usize = 100;
const RESERVED_STATE_FILE: &str = "state.json";

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn io_error(path: &Path, source: io::Error) -> Error {
    Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

pub fn create_dir_all(path: &Path) -> Result<(), Error> {
    fs::create_dir_all(path).map_err(|source| io_error(path, source))
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let name = path
        .file_name()
        .ok_or_else(|| Error::Input(format!("{} has no file name", path.display())))?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::Input(format!("{} has no parent directory", path.display())))?;
    create_dir_all(parent)?;
    let (file, tmp) = open_temp(parent, name, || {
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    })
    .map_err(|source| io_error(path, source))?;
    let result = write_synced(file, bytes).and_then(|()| rename_replace(&tmp, path));
    if let Err(source) = result {
        let _ = fs::remove_file(&tmp);
        return Err(io_error(path, source));
    }
    Ok(())
}

fn open_temp(
    parent: &Path,
    name: &OsStr,
    mut next: impl FnMut() -> u64,
) -> io::Result<(File, PathBuf)> {
    for _ in 0..TEMP_ATTEMPTS {
        let mut tmp_name = OsString::from(".");
        tmp_name.push(name);
        tmp_name.push(format!(".tmp-{}-{}", std::process::id(), next()));
        let tmp = parent.join(tmp_name);
        match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(file) => return Ok((file, tmp)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no unused temporary file name",
    ))
}

fn write_synced(mut file: File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(not(windows))]
fn rename_replace(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

#[cfg(windows)]
fn rename_replace(from: &Path, to: &Path) -> io::Result<()> {
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const RETRIES: usize = 8;
    let mut delay_ms = 25;
    let mut retried = 0;
    loop {
        match fs::rename(from, to) {
            Err(e)
                if retried < RETRIES
                    && (e.kind() == io::ErrorKind::PermissionDenied
                        || e.raw_os_error() == Some(ERROR_SHARING_VIOLATION)) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                delay_ms = (delay_ms * 2).min(100);
                retried += 1;
            }
            other => return other,
        }
    }
}

pub fn validate_file_name(file: &str) -> Result<(), Error> {
    let stem = file.split('.').next().unwrap_or_default();
    let bad = file.is_empty()
        || file == "."
        || file.contains("..")
        || !file
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        || file.ends_with(['.', ' '])
        || file.chars().count() > MAX_FILE_NAME_CHARS
        || file.eq_ignore_ascii_case(RESERVED_STATE_FILE)
        || is_reserved_name(stem);
    if bad {
        return Err(Error::Input(format!("invalid file name {file:?}")));
    }
    Ok(())
}

pub fn tree_size(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| tree_size(&entry.path()))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_round_trip_replaces_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a").join("b").join("data.json");
        atomic_write(&path, b"first").unwrap();
        atomic_write(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![OsString::from("data.json")]);
    }

    #[test]
    fn concurrent_writes_to_one_path_do_not_collide() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.json");
        std::thread::scope(|scope| {
            for i in 0..8 {
                let path = &path;
                scope.spawn(move || {
                    for _ in 0..20 {
                        atomic_write(path, format!("writer {i}").as_bytes()).unwrap();
                    }
                });
            }
        });
        assert!(fs::read_to_string(&path).unwrap().starts_with("writer "));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn temp_file_never_follows_a_planted_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        let planted = dir
            .path()
            .join(format!(".data.json.tmp-{}-0", std::process::id()));
        std::os::unix::fs::symlink(&victim, &planted).unwrap();
        let mut counter = 0..;
        let (file, tmp) = open_temp(dir.path(), OsStr::new("data.json"), || {
            counter.next().unwrap()
        })
        .unwrap();
        drop(file);
        assert!(tmp.to_string_lossy().ends_with("-1"));
        assert!(!victim.exists());
    }

    #[test]
    fn unsafe_file_names_are_rejected() {
        let long = "a".repeat(101);
        let bad = [
            "",
            ".",
            "..",
            "a/b",
            "a\\b",
            "../x",
            "c:x",
            "state.json",
            "State.JSON",
            "con",
            "CON.html",
            "nul.txt",
            "Com1",
            "lpt9.x",
            "con .txt",
            "page.",
            "page ",
            "com0",
            "LPT0.txt",
            "a b.html",
            "é.html",
            "a*b.html",
            "a?b",
            "a\0b",
            &long,
        ];
        for name in bad {
            assert!(validate_file_name(name).is_err(), "{name:?}");
        }
        let good = [
            "new.html",
            "console.html",
            "com10",
            "a_b-c.1.html",
            "lpt",
            "state.json.bak",
        ];
        for name in good {
            assert!(validate_file_name(name).is_ok(), "{name:?}");
        }
        assert!(validate_file_name(&"a".repeat(100)).is_ok());
    }
}
