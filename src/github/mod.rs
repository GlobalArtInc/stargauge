mod fetch;

use crate::error::ApiError;
use crate::model::{RateInfo, RepoId, RepoMeta, Star};
use crate::store::config::Token;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::Deserialize;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use ureq::config::RedirectAuthHeaders;

pub const MAX_PAGES: u64 = 400;
pub const HISTORY_PER_PAGE: u64 = 30;
pub const MAX_HISTORY_PAGES: u64 = 100;

const PER_PAGE: u64 = 100;
const SNIPPET_CHARS: usize = 200;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Client {
    agent: ureq::Agent,
    base: String,
    token: Option<Token>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Origin {
    None,
    Repo,
    Page { page: u64, total: u64 },
    History,
}

#[derive(Deserialize)]
struct RepoJson {
    full_name: String,
    description: Option<String>,
    language: Option<String>,
    size: Option<u64>,
    stargazers_count: u64,
    created_at: DateTime<Utc>,
    pushed_at: Option<DateTime<Utc>>,
}

impl Client {
    pub fn new(base_url: &str, token: Option<Token>, user_agent: &str) -> Client {
        Client::assemble(base_url, token, user_agent, true)
    }

    fn assemble(
        base_url: &str,
        token: Option<Token>,
        user_agent: &str,
        proxy_from_env: bool,
    ) -> Client {
        let user_agent = if user_agent.is_empty() {
            format!(
                "stargauge/{} (+https://github.com/GlobalArtInc/stargauge)",
                env!("CARGO_PKG_VERSION")
            )
        } else {
            user_agent.to_owned()
        };
        let builder = ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(REQUEST_TIMEOUT))
            .http_status_as_error(false)
            .redirect_auth_headers(RedirectAuthHeaders::SameHost)
            .user_agent(user_agent);
        let builder = if proxy_from_env {
            builder
        } else {
            builder.proxy(None)
        };
        Client {
            agent: ureq::Agent::new_with_config(builder.build()),
            base: base_url.trim_end_matches('/').to_owned(),
            token,
        }
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    pub fn repo(&self, id: &RepoId) -> Result<(RepoMeta, RateInfo), ApiError> {
        let url = format!(
            "{}/repos/{}/{}",
            self.base,
            fetch::encode_segment(&id.owner),
            fetch::encode_segment(&id.name)
        );
        let reply = fetch::get(self, &url, "application/vnd.github+json")?;
        if !(200..300).contains(&reply.status) {
            return Err(classify_ctx(
                reply.status,
                &|name| reply.header(name),
                &reply.body,
                Origin::Repo,
            ));
        }
        let json: RepoJson = serde_json::from_str(&reply.body).map_err(|e| ApiError::Http {
            status: reply.status,
            snippet: format!("invalid repository response: {e}"),
        })?;
        let rate = RateInfo {
            limit: reply
                .header("x-ratelimit-limit")
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0),
            remaining: reply
                .header("x-ratelimit-remaining")
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(u32::MAX),
            reset_at: reply
                .header("x-ratelimit-reset")
                .and_then(|v| parse_reset(&v)),
        };
        let meta = RepoMeta {
            full_name: json.full_name,
            description: json.description,
            language: json.language,
            size_kb: json.size,
            stargazers_count: json.stargazers_count,
            created_at: json.created_at,
            pushed_at: json.pushed_at,
        };
        Ok((meta, rate))
    }

    pub fn stargazers(
        &self,
        id: &RepoId,
        expected: u64,
        cancel: &AtomicBool,
        on_page: &(dyn Fn() + Sync),
    ) -> Result<Vec<Star>, ApiError> {
        fetch::stargazers(self, id, expected, cancel, on_page)
    }

    pub fn list_access(&self, id: &RepoId) -> Result<bool, ApiError> {
        fetch::list_access(self, id)
    }

    pub fn star_history(
        &self,
        id: &RepoId,
        cancel: &AtomicBool,
        on_page: &(dyn Fn() + Sync),
    ) -> Result<Vec<(NaiveDate, u32)>, ApiError> {
        fetch::star_history(self, id, cancel, on_page)
    }
}

pub fn pages_for(stargazers: u64) -> u64 {
    stargazers.div_ceil(PER_PAGE)
}

pub fn history_pages(created: NaiveDate, today: NaiveDate) -> u64 {
    let days = u64::try_from((week_start(today) - week_start(created)).num_days()).unwrap_or(0);
    let weeks = days / 7 + 1;
    weeks.div_ceil(HISTORY_PER_PAGE)
}

fn week_start(date: NaiveDate) -> NaiveDate {
    date - chrono::Days::new(u64::from(date.weekday().num_days_from_sunday()))
}

pub fn classify(status: u16, header: &dyn Fn(&str) -> Option<String>, body: &str) -> ApiError {
    classify_ctx(status, header, body, Origin::None)
}

pub(crate) fn classify_ctx(
    status: u16,
    header: &dyn Fn(&str) -> Option<String>,
    body: &str,
    ctx: Origin,
) -> ApiError {
    match status {
        404 => ApiError::NotFound,
        401 => ApiError::BadToken,
        403 | 429 => {
            let exhausted = header("x-ratelimit-remaining")
                .and_then(|v| v.trim().parse::<u64>().ok())
                == Some(0);
            if exhausted {
                ApiError::RateLimited {
                    reset_at: header("x-ratelimit-reset").and_then(|v| parse_reset(&v)),
                    secondary: false,
                }
            } else if header("retry-after").is_some() {
                ApiError::RateLimited {
                    reset_at: None,
                    secondary: true,
                }
            } else {
                http_error(status, body)
            }
        }
        422 => match ctx {
            Origin::None => ApiError::TooLarge {
                pages: MAX_PAGES + 1,
                max: MAX_PAGES,
            },
            Origin::Page { page, total } if page > MAX_PAGES => ApiError::TooLarge {
                pages: total.max(page),
                max: MAX_PAGES,
            },
            _ => http_error(status, body),
        },
        _ => http_error(status, body),
    }
}

fn http_error(status: u16, body: &str) -> ApiError {
    ApiError::Http {
        status,
        snippet: body
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(SNIPPET_CHARS)
            .collect(),
    }
}

fn parse_reset(value: &str) -> Option<DateTime<Utc>> {
    let secs: i64 = value.trim().parse().ok()?;
    DateTime::from_timestamp(secs, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;

    fn headers<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn classify_maps_statuses() {
        let reset = DateTime::from_timestamp(1_700_000_000, 0);
        assert_eq!(classify(404, &headers(&[]), ""), ApiError::NotFound);
        assert_eq!(classify(401, &headers(&[]), ""), ApiError::BadToken);
        assert_eq!(
            classify(
                403,
                &headers(&[
                    ("X-RateLimit-Remaining", "0"),
                    ("X-RateLimit-Reset", "1700000000")
                ]),
                ""
            ),
            ApiError::RateLimited {
                reset_at: reset,
                secondary: false
            }
        );
        assert_eq!(
            classify(
                429,
                &headers(&[
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", "oops")
                ]),
                ""
            ),
            ApiError::RateLimited {
                reset_at: None,
                secondary: false
            }
        );
        assert_eq!(
            classify(403, &headers(&[("Retry-After", "30")]), ""),
            ApiError::RateLimited {
                reset_at: None,
                secondary: true
            }
        );
        assert_eq!(
            classify(500, &headers(&[]), &"x".repeat(300)),
            ApiError::Http {
                status: 500,
                snippet: "x".repeat(200)
            }
        );
        assert_eq!(
            classify(422, &headers(&[]), ""),
            ApiError::TooLarge {
                pages: 401,
                max: 400
            }
        );
    }

    #[test]
    fn http_snippet_replaces_control_characters() {
        assert_eq!(
            classify(502, &headers(&[]), "a\x1b[31mb\r\nc\u{85}d"),
            ApiError::Http {
                status: 502,
                snippet: "a [31mb  c d".to_owned()
            }
        );
    }

    #[test]
    fn pages_for_rounds_up() {
        assert_eq!(pages_for(0), 0);
        assert_eq!(pages_for(100), 1);
        assert_eq!(pages_for(101), 2);
    }

    #[test]
    fn history_pages_counts_weeks_from_creation_week() {
        let day = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        assert_eq!(history_pages(day(2024, 5, 25), day(2026, 10, 31)), 5);
        assert_eq!(history_pages(day(2026, 10, 25), day(2026, 10, 31)), 1);
        assert_eq!(history_pages(day(2026, 10, 24), day(2026, 10, 25)), 1);
        assert_eq!(history_pages(day(2026, 10, 31), day(2026, 10, 1)), 1);
        assert_eq!(history_pages(day(2026, 4, 5), day(2026, 10, 31)), 1);
        assert_eq!(history_pages(day(2026, 4, 4), day(2026, 10, 31)), 2);
    }

    #[test]
    fn repo_parses_meta_and_rate_headers() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/repos/o/n")
            .match_header("accept", "application/vnd.github+json")
            .match_header("x-github-api-version", "2022-11-28")
            .match_header("authorization", "Bearer tok")
            .match_header("user-agent", "ua-test")
            .with_status(200)
            .with_header("x-ratelimit-limit", "60")
            .with_header("x-ratelimit-remaining", "59")
            .with_header("x-ratelimit-reset", "1700000000")
            .with_body(
                r#"{"full_name":"O/N","description":null,"language":null,"size":2048,
                "stargazers_count":7,"created_at":"2020-01-02T03:04:05Z","pushed_at":null}"#,
            )
            .create();
        let client = Client::assemble(&server.url(), Some(Token("tok".into())), "ua-test", false);
        let id = RepoId {
            owner: "o".into(),
            name: "n".into(),
        };
        let (meta, rate) = client.repo(&id).unwrap();
        mock.assert();
        assert_eq!(meta.full_name, "O/N");
        assert_eq!(meta.size_kb, Some(2048));
        assert_eq!(meta.stargazers_count, 7);
        assert_eq!(meta.pushed_at, None);
        assert_eq!(rate.limit, 60);
        assert_eq!(rate.remaining, 59);
        assert_eq!(rate.reset_at, DateTime::from_timestamp(1_700_000_000, 0));
    }

    #[test]
    fn token_is_not_sent_to_another_host_on_redirect() {
        let mut other = mockito::Server::new();
        let other_port = other.socket_address().port();
        let target = other
            .mock("GET", "/final")
            .match_header("authorization", Matcher::Missing)
            .with_status(200)
            .with_body(
                r#"{"full_name":"o/n","description":null,"language":null,"size":null,
                "stargazers_count":0,"created_at":"2020-01-02T03:04:05Z","pushed_at":null}"#,
            )
            .expect(1)
            .create();
        let mut first = mockito::Server::new();
        let first_port = first.socket_address().port();
        let redirect = first
            .mock("GET", "/repos/o/n")
            .match_header("authorization", "Bearer tok")
            .with_status(302)
            .with_header("location", &format!("http://localhost:{other_port}/final"))
            .expect(1)
            .create();
        let client = Client::assemble(
            &format!("http://127.0.0.1:{first_port}"),
            Some(Token("tok".into())),
            "",
            false,
        );
        let id = RepoId {
            owner: "o".into(),
            name: "n".into(),
        };
        client.repo(&id).unwrap();
        redirect.assert();
        target.assert();
    }
}
