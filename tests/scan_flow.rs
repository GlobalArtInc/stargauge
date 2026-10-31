use chrono::{DateTime, Utc};
use mockito::{Matcher, Server, ServerGuard};
use serde_json::{Value, json};
use stargauge::app::{self, Ctx};
use stargauge::error::{ApiError, Error};
use stargauge::github::Client;
use stargauge::model::{RepoId, Source, StarCounts, Zone};
use stargauge::store::config::Token;
use stargauge::store::{self, Layout};
use std::sync::Once;
use tempfile::TempDir;

const T1: &str = "2026-10-01T10:00:00Z";
const T2: &str = "2026-10-02T10:00:00Z";
const HISTORY_FIRST: &str = "2026-10-31T12:00:00Z";
const HISTORY_SECOND: &str = "2026-11-07T12:00:00Z";
const PROBE_OK: &str = r#"[{"starred_at":"2026-09-30T12:00:00Z","user":{"login":"a"}}]"#;

fn clear_proxy() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for name in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            unsafe { std::env::remove_var(name) };
        }
    });
}

fn repo_id() -> RepoId {
    RepoId {
        owner: "o".into(),
        name: "r".into(),
    }
}

fn token() -> Option<Token> {
    Some(Token::new("t"))
}

fn instant(text: &str) -> DateTime<Utc> {
    text.parse().unwrap()
}

fn reset_in_future() -> String {
    (Utc::now().timestamp() + 3600).to_string()
}

fn context(layout: &Layout, base: &str, now: DateTime<Utc>, token: Option<Token>) -> Ctx {
    Ctx {
        layout: layout.clone(),
        client: Client::new(base, token, "test"),
        zone: Zone::Utc,
        tz_label: "UTC".into(),
        now,
        open_report: false,
        term_width: 100,
    }
}

fn mock_repo_with(server: &mut ServerGuard, created: &str, stars: u64) -> mockito::Mock {
    mock_repo_budget(server, created, stars, "4999")
}

fn mock_repo_budget(
    server: &mut ServerGuard,
    created: &str,
    stars: u64,
    remaining: &str,
) -> mockito::Mock {
    let body = json!({
        "full_name": "o/r",
        "description": "test repository",
        "language": "Rust",
        "size": 10,
        "stargazers_count": stars,
        "created_at": created,
        "pushed_at": "2026-01-01T00:00:00Z",
    });
    server
        .mock("GET", "/repos/o/r")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_header("x-ratelimit-limit", "5000")
        .with_header("x-ratelimit-remaining", remaining)
        .with_header("x-ratelimit-reset", &reset_in_future())
        .with_body(body.to_string())
        .create()
}

fn mock_repo(server: &mut ServerGuard) -> mockito::Mock {
    mock_repo_with(server, "2020-01-01T00:00:00Z", 3)
}

fn mock_stargazers(server: &mut ServerGuard, logins: &[&str]) -> mockito::Mock {
    let body: Vec<Value> = logins
        .iter()
        .map(|login| json!({"starred_at": "2026-09-30T12:00:00Z", "user": {"login": login}}))
        .collect();
    server
        .mock("GET", "/repos/o/r/stargazers")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("per_page".into(), "100".into()),
            Matcher::UrlEncoded("page".into(), "1".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(Value::Array(body).to_string())
        .create()
}

fn mock_probe(server: &mut ServerGuard, status: usize, body: &str) -> mockito::Mock {
    server
        .mock("GET", "/repos/o/r/stargazers")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("per_page".into(), "1".into()),
            Matcher::UrlEncoded("page".into(), "1".into()),
        ]))
        .with_status(status)
        .with_header("content-type", "application/json")
        .with_body(body)
        .create()
}

fn mock_no_list(server: &mut ServerGuard) -> mockito::Mock {
    server
        .mock("GET", "/repos/o/r/stargazers")
        .match_query(Matcher::Any)
        .expect(0)
        .create()
}

fn mock_no_history(server: &mut ServerGuard) -> mockito::Mock {
    server
        .mock("GET", "/repos/o/r/stargazers/history")
        .match_query(Matcher::Any)
        .expect(0)
        .create()
}

fn week_entries(later: bool) -> Vec<Value> {
    let newest = instant("2026-10-25T00:00:00Z").timestamp();
    (0i64..44)
        .map(|index| {
            let days: [u32; 7] = match (index, later) {
                (0, false) => [0, 0, 2, 0, 0, 0, 0],
                (0, true) => [0, 0, 3, 0, 0, 0, 0],
                (43, false) => [0, 0, 0, 0, 1, 0, 0],
                _ => [0; 7],
            };
            json!({
                "week": newest - 7 * 86_400 * index,
                "total": days.iter().sum::<u32>(),
                "days": days,
            })
        })
        .collect()
}

fn mock_history_page(
    server: &mut ServerGuard,
    page: &str,
    body: Vec<Value>,
    has_next: bool,
) -> mockito::Mock {
    let link = format!(
        "<{}/repos/o/r/stargazers/history?per_page=30&page=2>; rel=\"next\"",
        server.url()
    );
    let mock = server
        .mock("GET", "/repos/o/r/stargazers/history")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("per_page".into(), "30".into()),
            Matcher::UrlEncoded("page".into(), page.into()),
        ]));
    let mock = if has_next {
        mock.with_header("link", &link)
    } else {
        mock
    };
    mock.with_status(200)
        .with_header("content-type", "application/json")
        .with_body(Value::Array(body).to_string())
        .create()
}

fn mock_history(server: &mut ServerGuard, later: bool) -> (mockito::Mock, mockito::Mock) {
    let weeks = week_entries(later);
    let (newest, oldest) = weeks.split_at(30);
    let first = mock_history_page(server, "1", newest.to_vec(), true);
    let second = mock_history_page(server, "2", oldest.to_vec(), false);
    (first, second)
}

#[test]
fn scan_twice_records_event() {
    clear_proxy();
    let root = TempDir::new().unwrap();
    let layout = Layout::from_root(root.path());
    let id = repo_id();

    let mut first = Server::new();
    let _repo_first = mock_repo(&mut first);
    let probe_first = mock_probe(&mut first, 200, PROBE_OK);
    let _stars_first = mock_stargazers(&mut first, &["a", "b", "c"]);
    let mut out: Vec<u8> = Vec::new();
    app::scan(
        &context(&layout, &first.url(), instant(T1), token()),
        &id,
        &mut out,
    )
    .unwrap();

    let mut second = Server::new();
    let _repo_second = mock_repo(&mut second);
    let probe_second = mock_probe(&mut second, 200, PROBE_OK);
    let _stars_second = mock_stargazers(&mut second, &["a", "b", "d"]);
    app::scan(
        &context(&layout, &second.url(), instant(T2), token()),
        &id,
        &mut out,
    )
    .unwrap();

    probe_first.assert();
    probe_second.assert();
    let state = store::load_state(&layout, &id)
        .unwrap()
        .expect("state saved");
    assert_eq!(state.logins, ["a", "b", "d"]);
    assert_eq!(state.events.len(), 1);
    assert_eq!(state.events[0].added, ["d"]);
    assert_eq!(state.events[0].removed, ["c"]);

    let report = std::fs::read_to_string(layout.repo_dir(&id).join("report.html")).unwrap();
    assert!(report.contains("https://github.com/d"));
    assert!(!report.contains("file://"));
    assert!(layout.assets_dir().join("echarts.min.js").is_file());
}

#[test]
fn rate_limit_is_reported() {
    clear_proxy();
    let root = TempDir::new().unwrap();
    let layout = Layout::from_root(root.path());
    let id = repo_id();

    let mut server = Server::new();
    let _repo = mock_repo(&mut server);
    let probe = server
        .mock("GET", "/repos/o/r/stargazers")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("per_page".into(), "1".into()),
            Matcher::UrlEncoded("page".into(), "1".into()),
        ]))
        .with_status(403)
        .with_header("x-ratelimit-remaining", "0")
        .with_header("x-ratelimit-reset", &reset_in_future())
        .with_body(r#"{"message":"API rate limit exceeded"}"#)
        .create();
    let history = mock_no_history(&mut server);
    let mut out: Vec<u8> = Vec::new();
    let err = app::scan(
        &context(&layout, &server.url(), instant(T1), token()),
        &id,
        &mut out,
    )
    .unwrap_err();

    assert!(matches!(
        err,
        Error::Api(ApiError::RateLimited {
            reset_at: Some(_),
            ..
        })
    ));
    probe.assert();
    history.assert();
    assert!(store::load_state(&layout, &id).unwrap().is_none());
}

#[test]
fn public_scan_without_token_records_counts() {
    clear_proxy();
    let root = TempDir::new().unwrap();
    let layout = Layout::from_root(root.path());
    let id = repo_id();
    let mut out: Vec<u8> = Vec::new();

    let mut first = Server::new();
    let _repo_first = mock_repo_with(&mut first, "2026-01-01T00:00:00Z", 3);
    let _history_first = mock_history(&mut first, false);
    let no_list_first = mock_no_list(&mut first);
    app::scan(
        &context(&layout, &first.url(), instant(HISTORY_FIRST), None),
        &id,
        &mut out,
    )
    .unwrap();
    no_list_first.assert();

    let mut second = Server::new();
    let _repo_second = mock_repo_with(&mut second, "2026-01-01T00:00:00Z", 3);
    let _history_second = mock_history(&mut second, true);
    let no_list_second = mock_no_list(&mut second);
    app::scan(
        &context(&layout, &second.url(), instant(HISTORY_SECOND), None),
        &id,
        &mut out,
    )
    .unwrap();
    no_list_second.assert();

    assert!(String::from_utf8(out).unwrap().contains("No GitHub token"));
    let state = store::load_state(&layout, &id)
        .unwrap()
        .expect("state saved");
    assert_eq!(state.source, Source::History);
    assert!(state.logins.is_empty());
    assert_eq!(state.events.len(), 1);
    assert_eq!(
        state.events[0].counts,
        Some(StarCounts {
            added: 1,
            removed: 1
        })
    );

    let dir = layout.repo_dir(&id);
    assert!(!dir.join("date_users.json").exists());
    assert!(!dir.join("cross_users.txt").exists());
    assert!(!dir.join("new_stars.html").exists());
    assert!(!dir.join("gone_stars.html").exists());
    assert!(dir.join("gone_graph.html").is_file());
    assert!(dir.join("report.html").is_file());
    let report = std::fs::read_to_string(dir.join("report.html")).unwrap();
    assert!(!report.contains("href=\"new_stars.html\""));
    assert!(!report.contains("href=\"gone_stars.html\""));
    let graphs = std::fs::read_to_string(dir.join("graphs.html")).unwrap();
    assert!(!graphs.contains("id=\"hour\""));
}

#[test]
fn closed_list_falls_back_to_history() {
    clear_proxy();
    let root = TempDir::new().unwrap();
    let layout = Layout::from_root(root.path());
    let id = repo_id();
    let mut out: Vec<u8> = Vec::new();

    let mut first = Server::new();
    let _repo_first = mock_repo(&mut first);
    let probe_first = mock_probe(&mut first, 200, PROBE_OK);
    let _stars_first = mock_stargazers(&mut first, &["a", "b", "c"]);
    app::scan(
        &context(&layout, &first.url(), instant(T1), token()),
        &id,
        &mut out,
    )
    .unwrap();
    probe_first.assert();
    let first_state = store::load_state(&layout, &id)
        .unwrap()
        .expect("state saved");
    assert_eq!(first_state.source, Source::List);

    let mut second = Server::new();
    let _repo_second = mock_repo_with(&mut second, "2020-01-01T00:00:00Z", 4);
    let probe_second = mock_probe(&mut second, 404, r#"{"message":"Not Found"}"#);
    let no_pool = second
        .mock("GET", "/repos/o/r/stargazers")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("per_page".into(), "100".into()),
            Matcher::UrlEncoded("page".into(), "1".into()),
        ]))
        .expect(0)
        .create();
    let history = second
        .mock("GET", "/repos/o/r/stargazers/history")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("per_page".into(), "30".into()),
            Matcher::UrlEncoded("page".into(), "1".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!([{"week": instant("2026-09-27T00:00:00Z").timestamp(), "total": 4, "days": [0, 0, 0, 0, 1, 3, 0]}]).to_string())
        .expect(1)
        .create();
    app::scan(
        &context(&layout, &second.url(), instant(T2), token()),
        &id,
        &mut out,
    )
    .unwrap();
    probe_second.assert();
    no_pool.assert();
    history.assert();

    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("open only to its admins and collaborators"));
    assert!(text.contains("not available for this scan"));

    let state = store::load_state(&layout, &id)
        .unwrap()
        .expect("state saved");
    assert_eq!(state.source, Source::History);
    assert!(state.logins.is_empty());
    assert_eq!(state.events.len(), 1);
    assert_eq!(
        state.events[0].counts,
        Some(StarCounts {
            added: 1,
            removed: 0
        })
    );
}

fn scan_list_first(layout: &Layout, id: &RepoId, out: &mut Vec<u8>) {
    let mut server = Server::new();
    let _repo = mock_repo(&mut server);
    let _probe = mock_probe(&mut server, 200, PROBE_OK);
    let _stars = mock_stargazers(&mut server, &["a", "b", "c"]);
    app::scan(
        &context(layout, &server.url(), instant(T1), token()),
        id,
        out,
    )
    .unwrap();
}

fn scan_history_first(layout: &Layout, id: &RepoId, out: &mut Vec<u8>) {
    let mut server = Server::new();
    let _repo = mock_repo_with(&mut server, "2026-01-01T00:00:00Z", 3);
    let _history = mock_history(&mut server, false);
    app::scan(
        &context(layout, &server.url(), instant(HISTORY_FIRST), None),
        id,
        out,
    )
    .unwrap();
}

fn state_bytes(layout: &Layout, id: &RepoId) -> Vec<u8> {
    std::fs::read(layout.repo_dir(id).join("state.json")).unwrap()
}

#[test]
fn history_to_list_transition_writes_name_pages_and_links() {
    clear_proxy();
    let root = TempDir::new().unwrap();
    let layout = Layout::from_root(root.path());
    let id = repo_id();
    let mut out: Vec<u8> = Vec::new();
    scan_history_first(&layout, &id, &mut out);

    let mut second = Server::new();
    let _repo = mock_repo_with(&mut second, "2026-01-01T00:00:00Z", 3);
    let probe = mock_probe(&mut second, 200, PROBE_OK);
    let _stars = mock_stargazers(&mut second, &["a", "b", "c"]);
    app::scan(
        &context(&layout, &second.url(), instant(HISTORY_SECOND), token()),
        &id,
        &mut out,
    )
    .unwrap();
    probe.assert();

    let state = store::load_state(&layout, &id).unwrap().unwrap();
    assert_eq!(state.source, Source::List);
    assert_eq!(state.logins, ["a", "b", "c"]);
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("The previous scan used the public star history")
    );
    let dir = layout.repo_dir(&id);
    assert!(dir.join("new_stars.html").is_file());
    let report = std::fs::read_to_string(dir.join("report.html")).unwrap();
    assert!(report.contains("href=\"new_stars.html\""));
}

#[test]
fn more_than_40000_stars_use_the_history_without_probing() {
    clear_proxy();
    let root = TempDir::new().unwrap();
    let layout = Layout::from_root(root.path());
    let id = repo_id();
    let mut out: Vec<u8> = Vec::new();

    let mut server = Server::new();
    let _repo = mock_repo_with(&mut server, "2026-01-01T00:00:00Z", 40_001);
    let _history = mock_history(&mut server, false);
    let no_list = mock_no_list(&mut server);
    app::scan(
        &context(&layout, &server.url(), instant(HISTORY_FIRST), token()),
        &id,
        &mut out,
    )
    .unwrap();
    no_list.assert();

    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("has more than 40,000 stars")
    );
    let state = store::load_state(&layout, &id).unwrap().unwrap();
    assert_eq!(state.source, Source::History);
}

#[test]
fn budget_too_small_for_the_history_makes_no_history_request() {
    clear_proxy();
    let root = TempDir::new().unwrap();
    let layout = Layout::from_root(root.path());
    let id = repo_id();
    let mut out: Vec<u8> = Vec::new();

    let mut server = Server::new();
    let _repo = mock_repo_budget(&mut server, "2026-01-01T00:00:00Z", 3, "1");
    let history = mock_no_history(&mut server);
    let err = app::scan(
        &context(&layout, &server.url(), instant(HISTORY_FIRST), None),
        &id,
        &mut out,
    )
    .unwrap_err();
    history.assert();
    assert!(matches!(err, Error::Api(ApiError::BudgetTooSmall { .. })));
    assert!(store::load_state(&layout, &id).unwrap().is_none());
}

#[test]
fn failed_second_scan_keeps_the_previous_state() {
    clear_proxy();
    let root = TempDir::new().unwrap();
    let layout = Layout::from_root(root.path());
    let id = repo_id();
    let mut out: Vec<u8> = Vec::new();
    scan_history_first(&layout, &id, &mut out);
    let before = state_bytes(&layout, &id);

    let mut second = Server::new();
    let _repo = mock_repo_with(&mut second, "2026-01-01T00:00:00Z", 3);
    let history = second
        .mock("GET", "/repos/o/r/stargazers/history")
        .match_query(Matcher::Any)
        .with_status(404)
        .with_body(r#"{"message":"Not Found"}"#)
        .expect(1)
        .create();
    let err = app::scan(
        &context(&layout, &second.url(), instant(HISTORY_SECOND), None),
        &id,
        &mut out,
    );
    history.assert();
    assert!(err.is_err());
    assert_eq!(state_bytes(&layout, &id), before);
}

#[test]
fn scan_without_token_keeps_saved_names() {
    clear_proxy();
    let root = TempDir::new().unwrap();
    let layout = Layout::from_root(root.path());
    let id = repo_id();
    let mut out: Vec<u8> = Vec::new();
    scan_list_first(&layout, &id, &mut out);
    let before = state_bytes(&layout, &id);

    let mut second = Server::new();
    let _repo = mock_repo(&mut second);
    let history = mock_no_history(&mut second);
    let err = app::scan(
        &context(&layout, &second.url(), instant(T2), None),
        &id,
        &mut out,
    )
    .unwrap_err();
    history.assert();
    assert!(matches!(err, Error::Input(_)), "{err}");
    assert_eq!(state_bytes(&layout, &id), before);
}

#[test]
fn history_scan_without_saved_days_does_not_invent_an_event() {
    clear_proxy();
    let root = TempDir::new().unwrap();
    let layout = Layout::from_root(root.path());
    let id = repo_id();
    let mut out: Vec<u8> = Vec::new();
    scan_history_first(&layout, &id, &mut out);
    let path = layout.repo_dir(&id).join("state.json");
    let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value.as_object_mut().unwrap().remove("star_days");
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

    let mut second = Server::new();
    let _repo = mock_repo_with(&mut second, "2026-01-01T00:00:00Z", 3);
    let _history = mock_history(&mut second, false);
    app::scan(
        &context(&layout, &second.url(), instant(HISTORY_SECOND), None),
        &id,
        &mut out,
    )
    .unwrap();
    let state = store::load_state(&layout, &id).unwrap().unwrap();
    assert!(state.events.is_empty());
    assert!(state.star_days.is_some());
}

#[test]
fn foreign_data_directory_is_refused_before_any_data_request() {
    clear_proxy();
    let root = TempDir::new().unwrap();
    let layout = Layout::from_root(root.path());
    std::fs::create_dir(root.path().join("repos")).unwrap();
    let mut out: Vec<u8> = Vec::new();

    let mut server = Server::new();
    let _repo = mock_repo_with(&mut server, "2026-01-01T00:00:00Z", 3);
    let history = mock_no_history(&mut server);
    let err = app::scan(
        &context(&layout, &server.url(), instant(HISTORY_FIRST), None),
        &repo_id(),
        &mut out,
    )
    .unwrap_err();
    history.assert();
    assert!(matches!(err, Error::Input(_)), "{err}");
}
