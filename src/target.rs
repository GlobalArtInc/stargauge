use crate::error::Error;
use crate::model::RepoId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Repo(RepoId),
    History,
    Clear,
    Exit,
}

const RESERVED_NAMES: [&str; 24] = [
    "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
    "com8", "com9", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

pub fn is_reserved_name(stem: &str) -> bool {
    let stem = stem.trim().to_lowercase();
    RESERVED_NAMES.contains(&stem.as_str())
}

pub fn dir_name(segment: &str) -> String {
    let mut name = segment.to_lowercase();
    let stem = name.split('.').next().unwrap_or_default();
    if is_reserved_name(stem) || name.ends_with('.') {
        name.push('~');
    }
    name
}

pub fn parse_input(s: &str) -> Result<Input, Error> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(Input::Exit);
    }
    if s.eq_ignore_ascii_case("history") || s.eq_ignore_ascii_case("his") {
        return Ok(Input::History);
    }
    if s.eq_ignore_ascii_case("clear") {
        return Ok(Input::Clear);
    }
    parse_repo(s).map(Input::Repo)
}

pub fn parse_repo(s: &str) -> Result<RepoId, Error> {
    let s = s.split(['?', '#']).next().unwrap_or_default().trim();
    if s.is_empty() {
        return Err(Error::Input("repository is empty".into()));
    }
    let path = match strip_prefix_ci(s, "git@") {
        Some(rest) => strip_prefix_ci(rest, "github.com:").ok_or_else(foreign_host)?,
        None => github_path(s)?,
    };
    let mut segments = path.trim_start_matches('/').split('/');
    let owner = segments.next().unwrap_or_default();
    let name = segments.next().unwrap_or_default();
    let name = name.strip_suffix(".git").unwrap_or(name);
    if owner.is_empty() || name.is_empty() {
        return Err(Error::Input("expected owner/repo".into()));
    }
    if !valid_owner(owner) {
        return Err(Error::Input("invalid owner name".into()));
    }
    if !valid_repo(name) {
        return Err(Error::Input("invalid repository name".into()));
    }
    Ok(RepoId {
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

fn github_path(s: &str) -> Result<&str, Error> {
    let (scheme_less, has_scheme) =
        match strip_prefix_ci(s, "https://").or_else(|| strip_prefix_ci(s, "http://")) {
            Some(rest) => (rest, true),
            None => (s, false),
        };
    if scheme_less.contains("://") {
        return Err(Error::Input("unsupported URL scheme, use https".into()));
    }
    let (host, path) = scheme_less.split_once('/').unwrap_or((scheme_less, ""));
    if !has_scheme && !host.contains('.') {
        return Ok(scheme_less);
    }
    if host.eq_ignore_ascii_case("github.com") || host.eq_ignore_ascii_case("www.github.com") {
        Ok(path)
    } else {
        Err(foreign_host())
    }
}

fn foreign_host() -> Error {
    Error::Input("only github.com repositories are supported".into())
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    let tail = s.get(prefix.len()..)?;
    head.eq_ignore_ascii_case(prefix).then_some(tail)
}

fn valid_owner(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && s.len() <= 39
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn valid_repo(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s != "."
        && s != ".."
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_repo_accepts_github_forms() {
        let cases = [
            ("https://github.com/Owner/repo", "Owner", "repo"),
            ("http://github.com/o/r", "o", "r"),
            ("https://www.github.com/o/r/", "o", "r"),
            ("https://github.com/o/r.git", "o", "r"),
            ("https://github.com/o/r/tree/main/src", "o", "r"),
            ("https://github.com/o/r?tab=readme#top", "o", "r"),
            ("github.com/o/r", "o", "r"),
            ("git@github.com:o/r.git", "o", "r"),
            ("o/r", "o", "r"),
            ("  o/r  ", "o", "r"),
        ];
        for (input, owner, name) in cases {
            let repo = parse_repo(input).unwrap_or_else(|e| panic!("{input}: {e}"));
            assert_eq!(
                (repo.owner.as_str(), repo.name.as_str()),
                (owner, name),
                "{input}"
            );
        }
    }

    #[test]
    fn parse_repo_rejects_bad_input() {
        let cases = [
            "",
            "https://gitlab.com/o/r",
            "git@gitlab.com:o/r.git",
            "ftp://github.com/o/r",
            "https://github.com/o",
            "https://github.com/",
            "o/",
            "o/r s",
            "o/..",
            "o/.",
            "-o/r",
            "o_x/r",
        ];
        for input in cases {
            assert!(matches!(parse_repo(input), Err(Error::Input(_))), "{input}");
        }
    }

    #[test]
    fn is_reserved_name_ignores_case_and_edge_spaces() {
        for name in [
            "con", "CON", " Nul ", "com0", "COM9", "lpt0", "Lpt5", "aux", "prn",
        ] {
            assert!(is_reserved_name(name), "{name:?}");
        }
        for name in ["", "console", "com10", "lpt", "com", "c on"] {
            assert!(!is_reserved_name(name), "{name:?}");
        }
    }

    #[test]
    fn dir_name_lowercases_and_suffixes_reserved_and_trailing_dot() {
        assert_eq!(dir_name("MyRepo"), "myrepo");
        assert_eq!(dir_name("CON"), "con~");
        assert_eq!(dir_name("Com0.txt"), "com0.txt~");
        assert_eq!(dir_name("com10"), "com10");
        assert_eq!(dir_name("foo."), "foo.~");
        assert_ne!(dir_name("con_"), dir_name("con"));
    }
}
