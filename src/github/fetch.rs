use super::{
    Client, HISTORY_PER_PAGE, MAX_HISTORY_PAGES, MAX_PAGES, Origin, PER_PAGE, classify_ctx,
    pages_for,
};
use crate::error::ApiError;
use crate::model::{RepoId, Star};
use chrono::{DateTime, Days, NaiveDate, Utc};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

const API_VERSION: &str = "2022-11-28";
const STAR_ACCEPT: &str = "application/vnd.github.star+json";
const AUTH_HEADER_ERROR: &str = "invalid Authorization header value, check the token";
const MAX_WORKERS: u64 = 10;
const BACKOFF: [Duration; 3] = [
    Duration::from_millis(500),
    Duration::from_millis(1000),
    Duration::from_millis(2000),
];
const MAX_RETRY_AFTER: u64 = 60;
const MAX_WINDOWS: u32 = 3;
const HISTORY_ACCEPT: &str = "application/vnd.github+json";
const SLEEP_SLICE: Duration = Duration::from_millis(50);
const MIN_WEEK: i64 = 1_167_609_600;
const WEEK_SLACK: i64 = 7 * 24 * 60 * 60;

pub(super) struct Reply {
    pub(super) status: u16,
    headers: ureq::http::HeaderMap,
    pub(super) body: String,
}

impl Reply {
    pub(super) fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    }

    fn link_has_next(&self) -> bool {
        self.headers
            .get_all("link")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .any(|value| value.split(',').any(|part| part.contains("rel=\"next\"")))
    }
}

struct Page {
    raw_len: u64,
    stars: Vec<Star>,
}

struct HistoryPage {
    next: bool,
    weeks: Vec<(i64, Vec<(NaiveDate, u32)>)>,
}

#[derive(Debug)]
enum Failure {
    Retry(ApiError),
    Secondary { wait: Option<u64>, err: ApiError },
    Fatal(ApiError),
}

impl Failure {
    fn into_error(self) -> ApiError {
        match self {
            Failure::Retry(err) | Failure::Fatal(err) | Failure::Secondary { err, .. } => err,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Gate {
    set_at: Instant,
    until: Instant,
    windows: u32,
}

#[derive(Debug, PartialEq, Eq)]
enum GateAction {
    Set(Gate),
    Fatal,
}

struct Control<'a> {
    cancel: &'a AtomicBool,
    stop: AtomicBool,
    gate: Mutex<Option<Gate>>,
}

impl<'a> Control<'a> {
    fn new(cancel: &'a AtomicBool) -> Control<'a> {
        Control {
            cancel,
            stop: AtomicBool::new(false),
            gate: Mutex::new(None),
        }
    }

    fn check(&self) -> Result<(), ApiError> {
        if self.stop.load(Ordering::SeqCst) || self.cancel.load(Ordering::SeqCst) {
            Err(ApiError::Interrupted)
        } else {
            Ok(())
        }
    }
}

#[derive(Deserialize)]
struct EntryJson {
    starred_at: Option<DateTime<Utc>>,
    user: Option<UserJson>,
}

#[derive(Deserialize)]
struct UserJson {
    login: String,
}

#[derive(Deserialize)]
struct WeekJson {
    week: i64,
    days: Vec<u32>,
}

pub(super) fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn transport_failure(err: ureq::Error) -> Failure {
    let message = err.to_string();
    match err {
        ureq::Error::Http(_) => {
            Failure::Fatal(ApiError::Network(format!("invalid request: {message}")))
        }
        ureq::Error::BadUri(_)
        | ureq::Error::RedirectFailed
        | ureq::Error::TooManyRedirects
        | ureq::Error::InvalidProxyUrl
        | ureq::Error::RequireHttpsOnly(_)
        | ureq::Error::BodyExceedsLimit(_)
        | ureq::Error::TlsRequired => Failure::Fatal(ApiError::Network(message)),
        _ => Failure::Retry(ApiError::Network(message)),
    }
}

fn send(client: &Client, url: &str, accept: &str) -> Result<Reply, Failure> {
    let mut request = client
        .agent
        .get(url)
        .header("Accept", accept)
        .header("X-GitHub-Api-Version", API_VERSION);
    if let Some(token) = &client.token {
        let value = format!("Bearer {}", token.0);
        if ureq::http::HeaderValue::from_str(&value).is_err() {
            return Err(Failure::Fatal(ApiError::Network(
                AUTH_HEADER_ERROR.to_owned(),
            )));
        }
        request = request.header("Authorization", value);
    }
    let mut response = request.call().map_err(transport_failure)?;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(transport_failure)?;
    Ok(Reply {
        status,
        headers,
        body,
    })
}

pub(super) fn get(client: &Client, url: &str, accept: &str) -> Result<Reply, ApiError> {
    send(client, url, accept).map_err(Failure::into_error)
}

fn pause(duration: Duration, control: &Control) -> Result<(), ApiError> {
    let deadline = Instant::now() + duration;
    loop {
        control.check()?;
        let now = Instant::now();
        if now >= deadline {
            return Ok(());
        }
        thread::sleep(SLEEP_SLICE.min(deadline - now));
    }
}

fn wait_gate(control: &Control) -> Result<Instant, ApiError> {
    loop {
        control.check()?;
        let sent_at = Instant::now();
        let until = lock(&control.gate).as_ref().map(|gate| gate.until);
        match until {
            Some(until) if until > sent_at => thread::sleep(SLEEP_SLICE.min(until - sent_at)),
            _ => return Ok(sent_at),
        }
    }
}

fn repo_url(client: &Client, id: &RepoId, tail: &str) -> String {
    format!(
        "{}/repos/{}/{}/{}",
        client.base,
        encode_segment(&id.owner),
        encode_segment(&id.name),
        tail
    )
}

fn failure_of(reply: &Reply, origin: Origin) -> Failure {
    let err = classify_ctx(
        reply.status,
        &|name| reply.header(name),
        &reply.body,
        origin,
    );
    match err {
        ApiError::RateLimited {
            secondary: true, ..
        } => Failure::Secondary {
            wait: reply
                .header("retry-after")
                .and_then(|v| v.trim().parse().ok()),
            err,
        },
        ApiError::Http { status, .. } if status >= 500 => Failure::Retry(err),
        other => Failure::Fatal(other),
    }
}

fn attempt_page(client: &Client, id: &RepoId, page: u64, total: u64) -> Result<Page, Failure> {
    let url = repo_url(
        client,
        id,
        &format!("stargazers?per_page={PER_PAGE}&page={page}"),
    );
    let reply = send(client, &url, STAR_ACCEPT)?;
    if !(200..300).contains(&reply.status) {
        return Err(failure_of(&reply, Origin::Page { page, total }));
    }
    let entries: Vec<EntryJson> = serde_json::from_str(&reply.body).map_err(|e| {
        Failure::Fatal(ApiError::Http {
            status: reply.status,
            snippet: format!("invalid stargazers response: {e}"),
        })
    })?;
    let raw_len = entries.len() as u64;
    let mut stars = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(user) = entry.user else { continue };
        let Some(starred_at) = entry.starred_at else {
            return Err(Failure::Fatal(ApiError::Http {
                status: reply.status,
                snippet: format!("stargazer {} has no starred_at", user.login),
            }));
        };
        stars.push(Star {
            login: user.login,
            starred_at,
        });
    }
    Ok(Page { raw_len, stars })
}

fn secondary_wait(wait: Option<u64>) -> Option<u64> {
    wait.filter(|secs| *secs <= MAX_RETRY_AFTER)
        .map(|secs| secs.max(1))
}

fn gate_decision(state: Option<&Gate>, sent_at: Instant, now: Instant, secs: u64) -> GateAction {
    let until = now + Duration::from_secs(secs);
    match state {
        None => GateAction::Set(Gate {
            set_at: now,
            until,
            windows: 1,
        }),
        Some(known) if sent_at < known.set_at => GateAction::Set(Gate {
            until: known.until.max(until),
            ..*known
        }),
        Some(known) if known.windows < MAX_WINDOWS => GateAction::Set(Gate {
            set_at: now,
            until: known.until.max(until),
            windows: known.windows + 1,
        }),
        Some(_) => GateAction::Fatal,
    }
}

fn retrying<T>(control: &Control, attempt: impl Fn() -> Result<T, Failure>) -> Result<T, ApiError> {
    let mut retries = 0;
    loop {
        let sent_at = wait_gate(control)?;
        match attempt() {
            Ok(result) => return Ok(result),
            Err(Failure::Fatal(err)) => return Err(err),
            Err(Failure::Retry(err)) => {
                let Some(delay) = BACKOFF.get(retries) else {
                    return Err(err);
                };
                retries += 1;
                pause(*delay, control)?;
            }
            Err(Failure::Secondary { wait, err }) => {
                let Some(secs) = secondary_wait(wait) else {
                    return Err(err);
                };
                let mut gate = lock(&control.gate);
                match gate_decision(gate.as_ref(), sent_at, Instant::now(), secs) {
                    GateAction::Set(next) => *gate = Some(next),
                    GateAction::Fatal => return Err(err),
                }
            }
        }
    }
}

fn fetch_page(
    client: &Client,
    id: &RepoId,
    page: u64,
    total: u64,
    control: &Control,
) -> Result<Page, ApiError> {
    retrying(control, || attempt_page(client, id, page, total))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(super) fn stargazers(
    client: &Client,
    id: &RepoId,
    expected: u64,
    cancel: &AtomicBool,
    on_page: &(dyn Fn() + Sync),
) -> Result<Vec<Star>, ApiError> {
    let pages = pages_for(expected);
    if pages == 0 {
        return Ok(Vec::new());
    }
    let next = AtomicUsize::new(1);
    let control = Control::new(cancel);
    let failure: Mutex<Option<ApiError>> = Mutex::new(None);
    let collected: Mutex<Vec<(u64, Page)>> = Mutex::new(Vec::new());
    let record = |err: ApiError| {
        lock(&failure).get_or_insert(err);
        control.stop.store(true, Ordering::SeqCst);
    };
    let run = || {
        loop {
            if control.stop.load(Ordering::SeqCst) {
                break;
            }
            if cancel.load(Ordering::SeqCst) {
                record(ApiError::Interrupted);
                break;
            }
            let page = next.fetch_add(1, Ordering::SeqCst) as u64;
            if page > pages {
                break;
            }
            match fetch_page(client, id, page, pages, &control) {
                Ok(result) => {
                    on_page();
                    lock(&collected).push((page, result));
                }
                Err(err) => {
                    record(err);
                    break;
                }
            }
        }
    };
    thread::scope(|scope| {
        let mut spawned = 0;
        for _ in 0..pages.min(MAX_WORKERS) {
            let started = thread::Builder::new()
                .name("stargauge-fetch".to_owned())
                .spawn_scoped(scope, run);
            if started.is_err() {
                break;
            }
            spawned += 1;
        }
        if spawned == 0 {
            run();
        }
    });
    if let Some(err) = lock(&failure).take() {
        return Err(err);
    }
    let mut all = std::mem::take(&mut *lock(&collected));
    let mut last_full = all
        .iter()
        .find(|(page, _)| *page == pages)
        .is_some_and(|(_, result)| result.raw_len == PER_PAGE);
    let mut page = pages;
    while last_full && page < MAX_PAGES {
        page += 1;
        let result = fetch_page(client, id, page, pages, &control)?;
        on_page();
        last_full = result.raw_len == PER_PAGE;
        all.push((page, result));
    }
    let mut earliest: HashMap<String, DateTime<Utc>> = HashMap::new();
    for (_, result) in all {
        for star in result.stars {
            earliest
                .entry(star.login)
                .and_modify(|at| *at = (*at).min(star.starred_at))
                .or_insert(star.starred_at);
        }
    }
    let mut stars: Vec<Star> = earliest
        .into_iter()
        .map(|(login, starred_at)| Star { login, starred_at })
        .collect();
    stars.sort_by(|a, b| (a.starred_at, &a.login).cmp(&(b.starred_at, &b.login)));
    Ok(stars)
}

fn history_error(status: u16, detail: impl std::fmt::Display) -> Failure {
    Failure::Fatal(ApiError::Http {
        status,
        snippet: format!("invalid star history response: {detail}"),
    })
}

fn week_in_range(week: i64, now: i64) -> bool {
    (MIN_WEEK..=now.saturating_add(WEEK_SLACK)).contains(&week)
}

fn attempt_history(client: &Client, id: &RepoId, page: u64) -> Result<HistoryPage, Failure> {
    let url = repo_url(
        client,
        id,
        &format!("stargazers/history?per_page={HISTORY_PER_PAGE}&page={page}"),
    );
    let reply = send(client, &url, HISTORY_ACCEPT)?;
    if !(200..300).contains(&reply.status) {
        return Err(failure_of(&reply, Origin::History));
    }
    let entries: Vec<WeekJson> =
        serde_json::from_str(&reply.body).map_err(|e| history_error(reply.status, e))?;
    let next = reply.link_has_next();
    let now = Utc::now().timestamp();
    let mut weeks = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.days.len() != 7 {
            return Err(history_error(
                reply.status,
                format!(
                    "week {} has {} days instead of 7",
                    entry.week,
                    entry.days.len()
                ),
            ));
        }
        if !week_in_range(entry.week, now) {
            return Err(history_error(
                reply.status,
                format!("week {} is out of range", entry.week),
            ));
        }
        let Some(first) = DateTime::from_timestamp(entry.week, 0).map(|at| at.date_naive()) else {
            return Err(history_error(
                reply.status,
                format!("week {} is not a valid timestamp", entry.week),
            ));
        };
        let mut days = Vec::with_capacity(7);
        for (offset, count) in (0u64..).zip(entry.days) {
            let Some(date) = first.checked_add_days(Days::new(offset)) else {
                return Err(history_error(
                    reply.status,
                    format!("week {} is out of range", entry.week),
                ));
            };
            days.push((date, count));
        }
        weeks.push((entry.week, days));
    }
    Ok(HistoryPage { next, weeks })
}

pub(super) fn star_history(
    client: &Client,
    id: &RepoId,
    cancel: &AtomicBool,
    on_page: &(dyn Fn() + Sync),
) -> Result<Vec<(NaiveDate, u32)>, ApiError> {
    let control = Control::new(cancel);
    let mut weeks: BTreeMap<i64, Vec<(NaiveDate, u32)>> = BTreeMap::new();
    let mut page = 1;
    loop {
        let result = retrying(&control, || attempt_history(client, id, page))?;
        on_page();
        let mut added = 0;
        for (week, days) in result.weeks {
            if weeks.insert(week, days).is_none() {
                added += 1;
            }
        }
        if !result.next {
            break;
        }
        if added == 0 {
            return Err(history_error(200, "pagination did not advance").into_error());
        }
        if page >= MAX_HISTORY_PAGES {
            return Err(ApiError::TooLarge {
                pages: MAX_HISTORY_PAGES + 1,
                max: MAX_HISTORY_PAGES,
            });
        }
        page += 1;
    }
    if weeks.is_empty() {
        return Err(history_error(200, "no weeks").into_error());
    }
    Ok(weeks
        .into_values()
        .flatten()
        .filter(|(_, count)| *count > 0)
        .collect())
}

pub(super) fn list_access(client: &Client, id: &RepoId) -> Result<bool, ApiError> {
    let cancel = AtomicBool::new(false);
    let control = Control::new(&cancel);
    let url = repo_url(client, id, "stargazers?per_page=1&page=1");
    let outcome = retrying(&control, || {
        let reply = send(client, &url, STAR_ACCEPT)?;
        if (200..300).contains(&reply.status) {
            Ok(())
        } else {
            Err(failure_of(&reply, Origin::Page { page: 1, total: 1 }))
        }
    });
    match outcome {
        Ok(()) => Ok(true),
        Err(ApiError::NotFound | ApiError::BadToken) => Ok(false),
        Err(ApiError::Http {
            status: 403,
            snippet,
        }) if !mentions_limit(&snippet) => Ok(false),
        Err(err) => Err(err),
    }
}

fn mentions_limit(snippet: &str) -> bool {
    let text = snippet.to_lowercase();
    text.contains("rate limit") || text.contains("abuse")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;
    use std::sync::atomic::AtomicU32;

    fn page_body(entries: &[(String, String)]) -> String {
        let items: Vec<serde_json::Value> = entries
            .iter()
            .map(|(login, at)| serde_json::json!({"starred_at": at, "user": {"login": login}}))
            .collect();
        serde_json::to_string(&items).unwrap()
    }

    fn entry(n: u32) -> (String, String) {
        (
            format!("u{n}"),
            format!("2024-01-01T00:{:02}:{:02}Z", n / 60, n % 60),
        )
    }

    fn id() -> RepoId {
        RepoId {
            owner: "o".into(),
            name: "n".into(),
        }
    }

    fn page_mock(
        server: &mut mockito::Server,
        page: u32,
        status: usize,
        body: String,
        hits: usize,
    ) -> mockito::Mock {
        server
            .mock("GET", "/repos/o/n/stargazers")
            .match_query(Matcher::UrlEncoded("page".into(), page.to_string()))
            .with_status(status)
            .with_body(body)
            .expect(hits)
            .create()
    }

    fn served_mock(server: &mut mockito::Server, page: u32, body: String) -> mockito::Mock {
        server
            .mock("GET", "/repos/o/n/stargazers")
            .match_query(Matcher::UrlEncoded("page".into(), page.to_string()))
            .with_status(200)
            .with_body(body)
            .create()
    }

    fn limited_mock(server: &mut mockito::Server, page: u32, hits: usize) -> mockito::Mock {
        server
            .mock("GET", "/repos/o/n/stargazers")
            .match_query(Matcher::UrlEncoded("page".into(), page.to_string()))
            .with_status(403)
            .with_header("retry-after", "1")
            .expect(hits)
            .create()
    }

    fn run(
        server: &mockito::Server,
        expected: u64,
        cancel: &AtomicBool,
        seen: &AtomicU32,
    ) -> Result<Vec<Star>, ApiError> {
        let client = Client::assemble(&server.url(), None, "", false);
        let on_page = || {
            seen.fetch_add(1, Ordering::SeqCst);
        };
        client.stargazers(&id(), expected, cancel, &on_page)
    }

    fn entries(range: std::ops::Range<u32>) -> String {
        page_body(&range.map(entry).collect::<Vec<_>>())
    }

    #[test]
    fn workers_collect_every_page_once() {
        let mut server = mockito::Server::new();
        let mocks = [
            page_mock(&mut server, 1, 200, entries(0..100), 1),
            page_mock(&mut server, 2, 200, entries(100..200), 1),
            page_mock(&mut server, 3, 200, entries(200..250), 1),
        ];
        let seen = AtomicU32::new(0);
        let stars = run(&server, 250, &AtomicBool::new(false), &seen).unwrap();
        mocks.iter().for_each(|mock| mock.assert());
        assert_eq!(seen.load(Ordering::SeqCst), 3);
        let logins: std::collections::HashSet<_> = stars.iter().map(|s| &s.login).collect();
        assert_eq!(stars.len(), 250);
        assert_eq!(logins.len(), 250);
    }

    #[test]
    fn fatal_error_on_one_page_is_returned() {
        let mut server = mockito::Server::new();
        let _ok1 = served_mock(&mut server, 1, entries(0..100));
        let bad = page_mock(&mut server, 2, 404, String::new(), 1);
        let _ok3 = served_mock(&mut server, 3, entries(200..250));
        let seen = AtomicU32::new(0);
        let result = run(&server, 250, &AtomicBool::new(false), &seen);
        bad.assert();
        assert_eq!(result.unwrap_err(), ApiError::NotFound);
    }

    #[test]
    fn server_error_is_retried() {
        let mut server = mockito::Server::new();
        let failing = page_mock(&mut server, 1, 500, "boom".into(), 1);
        let healthy = page_mock(&mut server, 1, 200, entries(0..5), 1);
        let seen = AtomicU32::new(0);
        let stars = run(&server, 5, &AtomicBool::new(false), &seen).unwrap();
        failing.assert();
        healthy.assert();
        assert_eq!(stars.len(), 5);
    }

    #[test]
    fn cancel_set_in_advance_sends_nothing() {
        let mut server = mockito::Server::new();
        let any = server
            .mock("GET", Matcher::Any)
            .with_status(200)
            .expect(0)
            .create();
        let seen = AtomicU32::new(0);
        let result = run(&server, 250, &AtomicBool::new(true), &seen);
        any.assert();
        assert_eq!(result.unwrap_err(), ApiError::Interrupted);
        assert_eq!(seen.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn full_last_page_pulls_the_next_empty_one() {
        let mut server = mockito::Server::new();
        let first = page_mock(&mut server, 1, 200, entries(0..100), 1);
        let second = page_mock(&mut server, 2, 200, "[]".into(), 1);
        let third = page_mock(&mut server, 3, 200, "[]".into(), 0);
        let seen = AtomicU32::new(0);
        let stars = run(&server, 100, &AtomicBool::new(false), &seen).unwrap();
        first.assert();
        second.assert();
        third.assert();
        assert_eq!(stars.len(), 100);
        assert_eq!(seen.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn secondary_limit_is_waited_and_retried() {
        let mut server = mockito::Server::new();
        let limited = limited_mock(&mut server, 1, 1);
        let healthy = page_mock(&mut server, 1, 200, entries(0..5), 1);
        let seen = AtomicU32::new(0);
        let started = Instant::now();
        let stars = run(&server, 100, &AtomicBool::new(false), &seen).unwrap();
        let elapsed = started.elapsed();
        limited.assert();
        healthy.assert();
        assert_eq!(stars.len(), 5);
        assert!(elapsed >= Duration::from_secs(1), "{elapsed:?}");
    }

    #[test]
    fn secondary_limit_after_three_windows_is_fatal() {
        let mut server = mockito::Server::new();
        let limited = limited_mock(&mut server, 1, 4);
        let seen = AtomicU32::new(0);
        let result = run(&server, 5, &AtomicBool::new(false), &seen);
        limited.assert();
        assert_eq!(
            result.unwrap_err(),
            ApiError::RateLimited {
                reset_at: None,
                secondary: true
            }
        );
    }

    fn gated(cancel: &AtomicBool) -> Control<'_> {
        let now = Instant::now();
        let control = Control::new(cancel);
        *lock(&control.gate) = Some(Gate {
            set_at: now,
            until: now + Duration::from_secs(30),
            windows: 1,
        });
        control
    }

    fn assert_woken_by_stop(sleeper: impl Fn(&Control) -> Result<(), ApiError>) {
        let cancel = AtomicBool::new(false);
        let control = gated(&cancel);
        thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(100));
                control.stop.store(true, Ordering::SeqCst);
            });
            assert_eq!(sleeper(&control).unwrap_err(), ApiError::Interrupted);
        });
    }

    #[test]
    fn stop_wakes_gate_and_pause() {
        assert_woken_by_stop(|control| wait_gate(control).map(|_| ()));
        assert_woken_by_stop(|control| pause(Duration::from_secs(30), control));
    }

    #[test]
    fn gate_decision_covers_each_rule() {
        let base = Instant::now();
        let at = |secs: u64| base + Duration::from_secs(secs);
        let window = |set_at: u64, until: u64, windows: u32| Gate {
            set_at: at(set_at),
            until: at(until),
            windows,
        };
        assert_eq!(
            gate_decision(None, at(0), at(5), 10),
            GateAction::Set(window(5, 15, 1))
        );
        let known = window(10, 40, 2);
        assert_eq!(
            gate_decision(Some(&known), at(5), at(20), 30),
            GateAction::Set(window(10, 50, 2))
        );
        assert_eq!(
            gate_decision(Some(&known), at(5), at(20), 5),
            GateAction::Set(window(10, 40, 2))
        );
        let first = window(10, 40, 1);
        assert_eq!(
            gate_decision(Some(&first), at(40), at(45), 7),
            GateAction::Set(window(45, 52, 2))
        );
        assert_eq!(
            gate_decision(Some(&first), at(10), at(45), 7),
            GateAction::Set(window(45, 52, 2))
        );
        let last = window(10, 40, MAX_WINDOWS);
        assert_eq!(
            gate_decision(Some(&last), at(40), at(45), 7),
            GateAction::Fatal
        );
        assert_eq!(
            gate_decision(Some(&last), at(5), at(45), 7),
            GateAction::Set(window(10, 52, MAX_WINDOWS))
        );
    }

    fn reply(status: u16, headers: &[(&str, &str)]) -> Reply {
        let mut map = ureq::http::HeaderMap::new();
        for (name, value) in headers {
            map.insert(
                ureq::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                ureq::http::HeaderValue::from_str(value).unwrap(),
            );
        }
        Reply {
            status,
            headers: map,
            body: String::new(),
        }
    }

    #[test]
    fn retry_after_is_classified_and_secondary_wait_clamps() {
        let cases = [
            ("0", Some(0)),
            ("60", Some(60)),
            ("61", Some(61)),
            ("soon", None),
        ];
        for (value, expected) in cases {
            let failure = failure_of(&reply(403, &[("retry-after", value)]), Origin::None);
            let Failure::Secondary { wait, .. } = failure else {
                panic!("{value}: {failure:?}");
            };
            assert_eq!(wait, expected, "{value}");
        }
        assert!(matches!(
            failure_of(&reply(403, &[]), Origin::None),
            Failure::Fatal(ApiError::Http { status: 403, .. })
        ));
        assert_eq!(secondary_wait(Some(0)), Some(1));
        assert_eq!(secondary_wait(Some(1)), Some(1));
        assert_eq!(secondary_wait(Some(60)), Some(60));
        assert_eq!(secondary_wait(Some(61)), None);
        assert_eq!(secondary_wait(None), None);
    }

    #[test]
    fn failure_of_sorts_statuses() {
        assert!(matches!(
            failure_of(&reply(503, &[]), Origin::History),
            Failure::Retry(ApiError::Http { status: 503, .. })
        ));
        assert!(matches!(
            failure_of(&reply(404, &[]), Origin::History),
            Failure::Fatal(ApiError::NotFound)
        ));
        assert!(matches!(
            failure_of(&reply(422, &[]), Origin::History),
            Failure::Fatal(ApiError::Http { status: 422, .. })
        ));
    }

    #[test]
    fn transport_failure_separates_fatal_from_retryable() {
        let bad_request = ureq::http::Request::builder()
            .uri("http://[bad")
            .body(())
            .unwrap_err();
        let fatal = [
            (ureq::Error::Http(bad_request), "invalid request: "),
            (ureq::Error::BadUri("x".into()), ""),
            (ureq::Error::RedirectFailed, ""),
            (ureq::Error::TooManyRedirects, ""),
            (ureq::Error::InvalidProxyUrl, ""),
            (ureq::Error::RequireHttpsOnly("u".into()), ""),
            (ureq::Error::BodyExceedsLimit(1), ""),
            (ureq::Error::TlsRequired, ""),
        ];
        for (err, prefix) in fatal {
            let message = format!("{prefix}{err}");
            match transport_failure(err) {
                Failure::Fatal(ApiError::Network(got)) => assert_eq!(got, message),
                other => panic!("{message}: {other:?}"),
            }
        }
        let retryable = [
            ureq::Error::Io(std::io::Error::other("reset")),
            ureq::Error::Timeout(ureq::Timeout::Global),
            ureq::Error::HostNotFound,
            ureq::Error::ConnectionFailed,
            ureq::Error::Tls("handshake"),
            ureq::Error::ConnectProxyFailed("x".into()),
        ];
        for err in retryable {
            let message = err.to_string();
            match transport_failure(err) {
                Failure::Retry(ApiError::Network(got)) => assert_eq!(got, message),
                other => panic!("{message}: {other:?}"),
            }
        }
    }

    #[test]
    fn invalid_token_is_fatal_and_not_sent() {
        let mut server = mockito::Server::new();
        let any = server
            .mock("GET", Matcher::Any)
            .with_status(200)
            .expect(0)
            .create();
        let client = Client::assemble(
            &server.url(),
            Some(crate::store::config::Token("bad\x01token".into())),
            "",
            false,
        );
        let result = client.stargazers(&id(), 5, &AtomicBool::new(false), &|| {});
        any.assert();
        assert_eq!(
            result.unwrap_err(),
            ApiError::Network(AUTH_HEADER_ERROR.to_owned())
        );
    }

    #[test]
    fn stargazers_follow_past_expected_pages_and_dedupe() {
        let mut server = mockito::Server::new();
        let full1: Vec<_> = (0..100).map(entry).collect();
        let full2: Vec<_> = (100..200).map(entry).collect();
        let mut short: Vec<_> = (200..204).map(entry).collect();
        short.push(("u0".to_owned(), "2025-01-01T00:00:00Z".to_owned()));
        let mut mocks = Vec::new();
        for (n, body) in [(1, &full1), (2, &full2), (3, &short)] {
            mocks.push(
                server
                    .mock("GET", "/repos/o/n/stargazers")
                    .match_query(Matcher::AllOf(vec![
                        Matcher::UrlEncoded("per_page".into(), "100".into()),
                        Matcher::UrlEncoded("page".into(), n.to_string()),
                    ]))
                    .match_header("accept", "application/vnd.github.star+json")
                    .with_status(200)
                    .with_body(page_body(body))
                    .expect(1)
                    .create(),
            );
        }
        let client = Client::assemble(&server.url(), None, "", false);
        let id = RepoId {
            owner: "o".into(),
            name: "n".into(),
        };
        let seen = AtomicU32::new(0);
        let on_page = || {
            seen.fetch_add(1, Ordering::SeqCst);
        };
        let stars = client
            .stargazers(&id, 100, &AtomicBool::new(false), &on_page)
            .unwrap();
        for mock in &mocks {
            mock.assert();
        }
        assert_eq!(seen.load(Ordering::SeqCst), 3);
        assert_eq!(stars.len(), 204);
        assert_eq!(stars[0].login, "u0");
        assert_eq!(
            stars[0].starred_at,
            "2024-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
        assert!(stars.windows(2).all(|w| w[0].starred_at <= w[1].starred_at));
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn week_at(y: i32, m: u32, d: u32) -> i64 {
        day(y, m, d)
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp()
    }

    fn weeks_body(weeks: &[(i64, Vec<u32>)]) -> String {
        let items: Vec<serde_json::Value> = weeks
            .iter()
            .map(|(week, days)| {
                serde_json::json!({"week": week, "days": days, "total": days.iter().sum::<u32>()})
            })
            .collect();
        serde_json::to_string(&items).unwrap()
    }

    fn history_mock(
        server: &mut mockito::Server,
        page: Option<u32>,
        body: String,
        link: Option<&str>,
        hits: usize,
    ) -> mockito::Mock {
        let mut query = vec![Matcher::UrlEncoded("per_page".into(), "30".into())];
        if let Some(page) = page {
            query.push(Matcher::UrlEncoded("page".into(), page.to_string()));
        }
        let mock = server
            .mock("GET", "/repos/o/n/stargazers/history")
            .match_query(Matcher::AllOf(query))
            .match_header("accept", "application/vnd.github+json")
            .with_status(200)
            .with_body(body);
        let mock = match link {
            Some(link) => mock.with_header("link", link),
            None => mock,
        };
        mock.expect(hits).create()
    }

    fn history(
        server: &mockito::Server,
        seen: &AtomicU32,
    ) -> Result<Vec<(NaiveDate, u32)>, ApiError> {
        let client = Client::assemble(&server.url(), None, "", false);
        let on_page = || {
            seen.fetch_add(1, Ordering::SeqCst);
        };
        client.star_history(&id(), &AtomicBool::new(false), &on_page)
    }

    const NEXT: &str = "<https://api.github.com/repositories/1/stargazers/history?per_page=30&page=2>; rel=\"next\", <https://api.github.com/repositories/1/stargazers/history?per_page=30&page=2>; rel=\"last\"";

    #[test]
    fn star_history_follows_next_and_spreads_days_over_utc_dates() {
        let mut server = mockito::Server::new();
        let newer = history_mock(
            &mut server,
            Some(1),
            weeks_body(&[(week_at(2024, 5, 26), vec![2, 0, 0, 0, 0, 0, 5])]),
            Some(NEXT),
            1,
        );
        let older = history_mock(
            &mut server,
            Some(2),
            weeks_body(&[(week_at(2024, 5, 19), vec![0, 0, 0, 0, 0, 0, 1])]),
            None,
            1,
        );
        let seen = AtomicU32::new(0);
        let days = history(&server, &seen).unwrap();
        newer.assert();
        older.assert();
        assert_eq!(seen.load(Ordering::SeqCst), 2);
        assert_eq!(
            days,
            vec![
                (day(2024, 5, 25), 1),
                (day(2024, 5, 26), 2),
                (day(2024, 6, 1), 5)
            ]
        );
    }

    #[test]
    fn star_history_counts_a_repeated_week_once() {
        let mut server = mockito::Server::new();
        let shared = (week_at(2024, 5, 26), vec![0, 3, 0, 0, 0, 0, 0]);
        let first = history_mock(
            &mut server,
            Some(1),
            weeks_body(&[
                (week_at(2024, 6, 2), vec![1, 0, 0, 0, 0, 0, 0]),
                shared.clone(),
            ]),
            Some(NEXT),
            1,
        );
        let second = history_mock(
            &mut server,
            Some(2),
            weeks_body(&[shared, (week_at(2024, 5, 19), vec![0, 0, 0, 0, 0, 0, 4])]),
            None,
            1,
        );
        let seen = AtomicU32::new(0);
        let days = history(&server, &seen).unwrap();
        first.assert();
        second.assert();
        assert_eq!(
            days,
            vec![
                (day(2024, 5, 25), 4),
                (day(2024, 5, 27), 3),
                (day(2024, 6, 2), 1)
            ]
        );
    }

    #[test]
    fn star_history_rejects_a_week_without_seven_days() {
        let mut server = mockito::Server::new();
        let bad = history_mock(
            &mut server,
            Some(1),
            weeks_body(&[(week_at(2024, 5, 19), vec![0, 0, 0, 0, 0, 1])]),
            None,
            1,
        );
        let seen = AtomicU32::new(0);
        let err = history(&server, &seen).unwrap_err();
        bad.assert();
        assert_eq!(seen.load(Ordering::SeqCst), 0);
        let ApiError::Http { status, snippet } = err else {
            panic!("{err:?}");
        };
        assert_eq!(status, 200);
        assert!(
            snippet.starts_with("invalid star history response: "),
            "{snippet}"
        );
    }

    #[test]
    fn star_history_stops_at_the_page_limit() {
        let mut server = mockito::Server::new();
        let endless = server
            .mock("GET", "/repos/o/n/stargazers/history")
            .match_query(Matcher::Any)
            .with_status(200)
            .with_header("link", NEXT)
            .with_body_from_request(|request| {
                let page: i64 = request
                    .path_and_query()
                    .split("&page=")
                    .nth(1)
                    .and_then(|tail| tail.split('&').next())
                    .and_then(|value| value.parse().ok())
                    .unwrap();
                weeks_body(&[(
                    week_at(2024, 5, 19) - page * 7 * 24 * 60 * 60,
                    vec![0, 0, 0, 0, 0, 0, 1],
                )])
                .into_bytes()
            })
            .expect(100)
            .create();
        let seen = AtomicU32::new(0);
        let err = history(&server, &seen).unwrap_err();
        endless.assert();
        assert_eq!(
            err,
            ApiError::TooLarge {
                pages: 101,
                max: 100
            }
        );
    }

    fn access_mock(
        server: &mut mockito::Server,
        status: usize,
        header: Option<(&str, &str)>,
        body: &str,
    ) -> mockito::Mock {
        let mock = server
            .mock("GET", "/repos/o/n/stargazers")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("per_page".into(), "1".into()),
                Matcher::UrlEncoded("page".into(), "1".into()),
            ]))
            .match_header("accept", "application/vnd.github.star+json")
            .with_status(status)
            .with_body(body);
        let mock = match header {
            Some((name, value)) => mock.with_header(name, value),
            None => mock,
        };
        mock.expect(1).create()
    }

    #[test]
    fn list_access_reads_the_answer_to_one_probe() {
        for (status, expected) in [(200, true), (404, false), (401, false), (403, false)] {
            let mut server = mockito::Server::new();
            let probe = access_mock(&mut server, status, None, "");
            let client = Client::assemble(&server.url(), None, "", false);
            assert_eq!(client.list_access(&id()), Ok(expected), "{status}");
            probe.assert();
        }
        let mut server = mockito::Server::new();
        let probe = access_mock(&mut server, 403, Some(("x-ratelimit-remaining", "0")), "");
        let client = Client::assemble(&server.url(), None, "", false);
        assert_eq!(
            client.list_access(&id()),
            Err(ApiError::RateLimited {
                reset_at: None,
                secondary: false
            })
        );
        probe.assert();
    }

    #[test]
    fn list_access_tells_a_closed_list_from_a_temporary_failure() {
        let mut server = mockito::Server::new();
        let probe = access_mock(
            &mut server,
            403,
            None,
            r#"{"message":"You have exceeded a secondary rate limit..."}"#,
        );
        let client = Client::assemble(&server.url(), None, "", false);
        let err = client.list_access(&id()).unwrap_err();
        probe.assert();
        assert!(matches!(err, ApiError::Http { status: 403, .. }), "{err:?}");

        let mut server = mockito::Server::new();
        let probe = access_mock(
            &mut server,
            403,
            None,
            r#"{"message":"Resource not accessible by personal access token"}"#,
        );
        let client = Client::assemble(&server.url(), None, "", false);
        assert_eq!(client.list_access(&id()), Ok(false));
        probe.assert();

        let mut server = mockito::Server::new();
        let probe = access_mock(&mut server, 429, None, "");
        let client = Client::assemble(&server.url(), None, "", false);
        let err = client.list_access(&id()).unwrap_err();
        probe.assert();
        assert!(matches!(err, ApiError::Http { status: 429, .. }), "{err:?}");
    }

    fn history_failure(err: ApiError, detail: &str) {
        let ApiError::Http { status, snippet } = err else {
            panic!("{err:?}");
        };
        assert_eq!(status, 200);
        assert_eq!(snippet, format!("invalid star history response: {detail}"));
    }

    #[test]
    fn star_history_without_any_week_is_invalid() {
        let mut server = mockito::Server::new();
        let empty = history_mock(&mut server, Some(1), "[]".into(), None, 1);
        let seen = AtomicU32::new(0);
        let err = history(&server, &seen).unwrap_err();
        empty.assert();
        history_failure(err, "no weeks");
    }

    #[test]
    fn star_history_finds_next_in_any_link_value() {
        let mut server = mockito::Server::new();
        let first = server
            .mock("GET", "/repos/o/n/stargazers/history")
            .match_query(Matcher::UrlEncoded("page".into(), "1".into()))
            .with_status(200)
            .with_header("link", "<https://api.github.com/x?page=1>; rel=\"prev\"")
            .with_header("link", NEXT)
            .with_body(weeks_body(&[(
                week_at(2024, 5, 26),
                vec![1, 0, 0, 0, 0, 0, 0],
            )]))
            .expect(1)
            .create();
        let second = history_mock(
            &mut server,
            Some(2),
            weeks_body(&[(week_at(2024, 5, 19), vec![0, 0, 0, 0, 0, 0, 1])]),
            None,
            1,
        );
        let seen = AtomicU32::new(0);
        let days = history(&server, &seen).unwrap();
        first.assert();
        second.assert();
        assert_eq!(days, vec![(day(2024, 5, 25), 1), (day(2024, 5, 26), 1)]);
    }

    #[test]
    fn star_history_stops_when_next_adds_no_week() {
        let mut server = mockito::Server::new();
        let same = history_mock(
            &mut server,
            None,
            weeks_body(&[(week_at(2024, 5, 19), vec![0, 0, 0, 0, 0, 0, 1])]),
            Some(NEXT),
            2,
        );
        let seen = AtomicU32::new(0);
        let err = history(&server, &seen).unwrap_err();
        same.assert();
        history_failure(err, "pagination did not advance");
    }

    #[test]
    fn week_in_range_has_both_edges() {
        let now = 1_800_000_000;
        assert!(week_in_range(MIN_WEEK, now));
        assert!(!week_in_range(MIN_WEEK - 1, now));
        assert!(week_in_range(now + WEEK_SLACK, now));
        assert!(!week_in_range(now + WEEK_SLACK + 1, now));
    }

    #[test]
    fn star_history_rejects_an_extreme_week() {
        for week in [-8_000_000_000_000_i64, 8_000_000_000_000] {
            let mut server = mockito::Server::new();
            let bad = history_mock(
                &mut server,
                Some(1),
                weeks_body(&[(week, vec![0, 0, 0, 0, 0, 0, 1])]),
                None,
                1,
            );
            let seen = AtomicU32::new(0);
            let err = history(&server, &seen).unwrap_err();
            bad.assert();
            assert_eq!(seen.load(Ordering::SeqCst), 0);
            history_failure(err, &format!("week {week} is out of range"));
        }
    }

    #[test]
    fn star_history_reports_not_found() {
        let mut server = mockito::Server::new();
        let missing = server
            .mock("GET", "/repos/o/n/stargazers/history")
            .match_query(Matcher::Any)
            .with_status(404)
            .expect(1)
            .create();
        let seen = AtomicU32::new(0);
        let err = history(&server, &seen).unwrap_err();
        missing.assert();
        assert_eq!(err, ApiError::NotFound);
    }

    #[test]
    fn star_history_retries_a_server_error_once() {
        let mut server = mockito::Server::new();
        let failing = server
            .mock("GET", "/repos/o/n/stargazers/history")
            .match_query(Matcher::Any)
            .with_status(500)
            .expect(1)
            .create();
        let healthy = history_mock(
            &mut server,
            Some(1),
            weeks_body(&[(week_at(2024, 5, 19), vec![0, 0, 0, 0, 0, 0, 1])]),
            None,
            1,
        );
        let seen = AtomicU32::new(0);
        let days = history(&server, &seen).unwrap();
        failing.assert();
        healthy.assert();
        assert_eq!(days, vec![(day(2024, 5, 25), 1)]);
    }

    #[test]
    fn star_history_with_cancel_sends_nothing() {
        let mut server = mockito::Server::new();
        let any = server
            .mock("GET", Matcher::Any)
            .with_status(200)
            .expect(0)
            .create();
        let client = Client::assemble(&server.url(), None, "", false);
        let result = client.star_history(&id(), &AtomicBool::new(true), &|| {});
        any.assert();
        assert_eq!(result.unwrap_err(), ApiError::Interrupted);
    }
}
