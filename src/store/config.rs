use crate::error::Error;
use crate::store::fs::{create_dir_all, io_error};
#[cfg(unix)]
use crate::store::warn;
use std::fmt;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;

const DEFAULT_CONFIG: &str = "[github]\ntoken = \"\"\n";
const TOKEN_ENV: &str = "STARGAUGE_TOKEN";

#[derive(Clone, PartialEq, Eq)]
pub struct Token(pub(crate) String);

impl Token {
    pub fn new(value: impl Into<String>) -> Token {
        Token(value.into())
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(***)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub token: Option<Token>,
}

pub fn load_or_init(path: &Path) -> Result<Config, Error> {
    let env = match std::env::var(TOKEN_ENV) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(Error::Config(format!("{TOKEN_ENV} is not valid UTF-8")));
        }
    };
    load_with_env(path, env)
}

fn load_with_env(path: &Path, env: Option<String>) -> Result<Config, Error> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    let created = create_new(path)?;
    if !created {
        tighten_permissions(path)?;
    }
    let text = std::fs::read_to_string(path).map_err(|source| io_error(path, source))?;
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        let line = e.span().map(|span| {
            let end = span.start.min(text.len());
            text.as_bytes()[..end]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count()
                + 1
        });
        match line {
            Some(line) => Error::Config(format!("{} line {line}: {}", path.display(), e.message())),
            None => Error::Config(format!("{}: {}", path.display(), e.message())),
        }
    })?;
    let file_token = file_token(&table, path)?;
    let env = clean_token(env, TOKEN_ENV)?;
    let token = match env {
        Some(token) => Some(token),
        None => clean_token(file_token, &format!("token in {}", path.display()))?,
    };
    Ok(Config {
        token: token.map(Token),
    })
}

fn file_token(table: &toml::Table, path: &Path) -> Result<Option<String>, Error> {
    let Some(github) = table.get("github") else {
        return Ok(None);
    };
    let github = github
        .as_table()
        .ok_or_else(|| Error::Config(format!("{}: `github` must be a table", path.display())))?;
    match github.get("token") {
        None => Ok(None),
        Some(toml::Value::String(token)) => Ok(Some(token.clone())),
        Some(_) => Err(Error::Config(format!(
            "{}: `github.token` must be a string",
            path.display()
        ))),
    }
}

fn clean_token(raw: Option<String>, source: &str) -> Result<Option<String>, Error> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let value = raw.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().any(|c| c.is_control() || !c.is_ascii()) {
        return Err(Error::Config(format!(
            "{source} contains control or non-ASCII characters"
        )));
    }
    Ok(Some(value.to_string()))
}

fn create_new(path: &Path) -> Result<bool, Error> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(DEFAULT_CONFIG.as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|source| io_error(path, source))?;
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(source) => Err(io_error(path, source)),
    }
}

#[cfg(unix)]
fn tighten_permissions(path: &Path) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path).map_err(|source| io_error(path, source))?;
    if meta.permissions().mode() & 0o077 != 0 {
        match std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            Ok(()) => warn(&format!(
                "{} was readable by other users, permissions set to 0600",
                path.display()
            )),
            Err(e) => warn(&format!(
                "could not restrict permissions of {}: {e}",
                path.display()
            )),
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn tighten_permissions(_path: &Path) -> Result<(), Error> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, text: &str) -> std::path::PathBuf {
        let path = dir.join("config.toml");
        std::fs::write(&path, text).unwrap();
        path
    }

    fn config_error(result: Result<Config, Error>) -> String {
        match result.unwrap_err() {
            Error::Config(message) => message,
            other => panic!("{other}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn new_config_is_private_and_loose_permissions_are_tightened() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("cfg").join("fresh.toml");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(load_with_env(&fresh, None).unwrap().token, None);
        assert_eq!(mode(&fresh), 0o600);
        assert_eq!(std::fs::read_to_string(&fresh).unwrap(), DEFAULT_CONFIG);
        std::fs::set_permissions(&fresh, std::fs::Permissions::from_mode(0o644)).unwrap();
        load_with_env(&fresh, None).unwrap();
        assert_eq!(mode(&fresh), 0o600);
    }

    #[test]
    fn token_comes_from_env_first_then_file_and_never_prints() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "[github]\ntoken = \"  from-file \"\n");
        let config = load_with_env(&path, None).unwrap();
        assert_eq!(config.token, Some(Token("from-file".into())));
        assert_eq!(format!("{:?}", config.token.unwrap()), "Token(***)");
        let config = load_with_env(&path, Some("  from-env\n".into())).unwrap();
        assert_eq!(config.token, Some(Token("from-env".into())));
        let config = load_with_env(&path, Some("   ".into())).unwrap();
        assert_eq!(config.token, Some(Token("from-file".into())));
        for bad in ["abc\r\ndef", "abc\tdef", "abc\u{7f}def", "abcé"] {
            let message = config_error(load_with_env(&path, Some(bad.into())));
            assert!(!message.contains("abc"), "{message}");
        }
    }

    #[test]
    fn config_errors_never_contain_the_token_value() {
        let dir = tempfile::tempdir().unwrap();
        let cases = [
            ("github = \"ghp_secretvalue\"\n", "`github` must be a table"),
            (
                "[github]\ntoken = 123456789012\n",
                "`github.token` must be a string",
            ),
            (
                "[github]\ntoken = [\"ghp_secretvalue\"]\n",
                "`github.token` must be a string",
            ),
        ];
        for (text, expected) in cases {
            let path = write(dir.path(), text);
            let message = config_error(load_with_env(&path, None));
            assert!(message.contains(expected), "{message}");
            assert!(!message.contains("secretvalue"), "{message}");
            assert!(!message.contains("123456789012"), "{message}");
        }
        let path = write(dir.path(), "[other]\nx = 1\n");
        assert_eq!(load_with_env(&path, None).unwrap().token, None);
    }

    #[test]
    fn syntax_errors_name_the_file_and_line_even_after_multibyte_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "# ééé\n[github\ntoken = ");
        let message = config_error(load_with_env(&path, None));
        assert!(message.contains("config.toml line 2"), "{message}");
    }

    #[test]
    fn unquoted_token_syntax_error_names_the_line_but_not_the_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "[github]\ntoken = ghp_secretvalue\n");
        let message = config_error(load_with_env(&path, None));
        assert!(message.contains("config.toml line 2"), "{message}");
        assert!(!message.contains("secretvalue"), "{message}");
    }
}
