use crate::model::{
    CrossUser, Diff, Gap, HistoryTotals, Last30, Marketing, Quartiles, RepoMeta, ScanEvent, Share,
    Side, Source, Star, StarCounts, Stats, Zone, format_interval,
};
use askama::Template;
use chrono::{DateTime, NaiveDate, Utc, Weekday};
use std::collections::{BTreeMap, BTreeSet};

const NOT_ENOUGH_DATA: &str = "The repository is still young, not enough data";
const EMPTY_GROUP: &str = "-------------------------";
const EVENT_TIME: &str = "%Y-%m-%d_%H:%M";
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const BACKGROUND_LANGUAGES: [&str; 30] = [
    "python",
    "c",
    "shell",
    "c++",
    "mdx",
    "javascript",
    "typescript",
    "erlang",
    "visual basic .net",
    "rust",
    "julia",
    "scala",
    "elixir",
    "lua",
    "css",
    "kotlin",
    "html",
    "perl",
    "swift",
    "go",
    "ruby",
    "php",
    "c#",
    "java",
    "dockerfile",
    "r",
    "powershell",
    "assembly",
    "pascal",
    "matlab",
];

pub enum ReportChanges<'a> {
    Logins(&'a Diff),
    Counts { counts: StarCounts, prev_stars: u64 },
}

pub struct ReportInput<'a> {
    pub repo_url: &'a str,
    pub meta: &'a RepoMeta,
    pub stats: &'a Stats,
    pub source: Source,
    pub changes: ReportChanges<'a>,
    pub prev_scanned_at: DateTime<Utc>,
    pub scanned_at: DateTime<Utc>,
    pub zone: &'a Zone,
    pub tz_label: &'a str,
    pub has_gone_graph: bool,
    pub background_url: Option<String>,
}

#[derive(Template)]
#[template(path = "report.html")]
struct ReportPage {
    repo: String,
    repo_url: String,
    background: String,
    by_counts: bool,
    new_heading: String,
    gone_heading: String,
    new_logins: Vec<String>,
    gone_logins: Vec<String>,
    has_gone_graph: bool,
    chart_count: usize,
    source: &'static str,
    list_source: bool,
    rating: String,
    size: String,
    stars: String,
    peak: String,
    trend: String,
    best_weekdays: String,
    best_months: String,
    month_label: String,
    month_lines: Vec<String>,
    year_label: String,
    year_lines: Vec<String>,
    gap: String,
    median_change: String,
    average_change: String,
    created: String,
    updated: String,
    marketing: String,
    fake: String,
    description: String,
    interval: String,
    prev: String,
    now: String,
}

#[derive(Template)]
#[template(path = "history.html")]
struct HistoryPage {
    repo: String,
    heading: String,
    blocks: Vec<HistoryBlock>,
    dups: Vec<Dup>,
}

struct HistoryBlock {
    header: String,
    users: Vec<String>,
}

struct Dup {
    login: String,
    count: u32,
}

pub fn report(input: &ReportInput) -> String {
    let meta = input.meta;
    let stats = input.stats;
    let zone = input.zone;
    let total: u64 = stats.by_month.iter().map(|(_, count)| count).sum();
    let private = if stats.private_stars == 0 {
        String::new()
    } else {
        format!(" ({} private stars)", stats.private_stars)
    };
    let created = meta.created_at.date_naive();
    let (new_heading, gone_heading, new_logins, gone_logins) = match input.changes {
        ReportChanges::Logins(diff) => (
            format!(
                "🌟 New stars (+{}%):",
                share_percent(diff.added.len() as u64, meta.stargazers_count)
            ),
            format!(
                "💫 Gone stars (-{}%):",
                share_percent(diff.removed.len() as u64, meta.stargazers_count)
            ),
            diff.added.clone(),
            diff.removed.clone(),
        ),
        ReportChanges::Counts { counts, .. } => (
            format!(
                "🌟 New stars: +{} (+{}%)",
                counts.added,
                share_percent(counts.added, meta.stargazers_count)
            ),
            format!(
                "💫 Gone stars: -{} (-{}%)",
                counts.removed,
                share_percent(counts.removed, meta.stargazers_count)
            ),
            Vec::new(),
            Vec::new(),
        ),
    };
    let (by_counts, rating) = match input.changes {
        ReportChanges::Logins(_) => (false, String::new()),
        ReportChanges::Counts { prev_stars, .. } => {
            (true, format!("{prev_stars} → {}", meta.stargazers_count))
        }
    };
    let page = ReportPage {
        repo: meta.full_name.clone(),
        repo_url: input.repo_url.to_owned(),
        background: input.background_url.clone().unwrap_or_default(),
        by_counts,
        new_heading,
        gone_heading,
        new_logins,
        gone_logins,
        has_gone_graph: input.has_gone_graph,
        chart_count: 5 + usize::from(stats.by_hour.is_some()),
        source: match input.source {
            Source::List => "stargazer list",
            Source::History => "public star history (no names, no hours)",
        },
        list_source: matches!(input.source, Source::List),
        rating,
        size: meta.size_kb.map_or_else(
            || "~N/O MB".to_owned(),
            |kb| format!("~{:.2} MB", kb as f64 / 1024.0),
        ),
        stars: format!("{} stars", meta.stargazers_count),
        peak: peak_text(stats.peak),
        trend: format!("{} stars / day", stats.trend_per_day),
        best_weekdays: format!(
            "{} / {}",
            opt_text(stats.best_weekday.map(weekday_name)),
            opt_text(stats.worst_weekday.map(weekday_name)),
        ),
        best_months: format!(
            "{} / {}",
            opt_text(stats.best_month.map(month_name)),
            opt_text(stats.worst_month.map(month_name)),
        ),
        month_label: format!("📅 Distribution-of-stars-by-month{private} ({total} stars)"),
        month_lines: distribution_lines(stats.month_quartiles.as_ref(), true),
        year_label: format!("📅 Distribution-of-stars-by-year ({total} stars)"),
        year_lines: distribution_lines(stats.year_quartiles.as_ref(), false),
        gap: gap_text(stats.longest_gap.as_ref()),
        median_change: median_change_text(stats.last30.as_ref()),
        average_change: average_change_text(stats.last30.as_ref()),
        created: format!(
            "{created} ({} days)",
            (zone.date_of(input.scanned_at) - created).num_days()
        ),
        updated: meta.pushed_at.map_or_else(
            || "N/O".to_owned(),
            |pushed| {
                format!(
                    "{}_{}",
                    zone.format(pushed, "%Y-%m-%d__%H:%M"),
                    input.tz_label
                )
            },
        ),
        marketing: marketing_text(stats.marketing).to_owned(),
        fake: fake_text(stats.marketing).to_owned(),
        description: meta
            .description
            .clone()
            .unwrap_or_else(|| "No repository description available".to_owned()),
        interval: format_interval(input.scanned_at - input.prev_scanned_at),
        prev: zone.format(input.prev_scanned_at, EVENT_TIME),
        now: zone.format(input.scanned_at, EVENT_TIME),
    };
    page.render().expect("report template renders")
}

pub fn history_page(
    side: Side,
    repo: &str,
    events: &[ScanEvent],
    totals: &HistoryTotals,
    dups: &[(String, u32)],
    zone: &Zone,
) -> String {
    let (icon, word, arrow, sign) = match side {
        Side::Added => ("🌟", "New", "↝", "+"),
        Side::Removed => ("💫", "Gone", "⇢", "-"),
    };
    let mut ordered: Vec<&ScanEvent> = events
        .iter()
        .filter(|event| side_total(event, side) > 0)
        .collect();
    ordered.sort_by_key(|event| event.scanned_at);
    let span = if ordered.len() == 1 {
        "1 scan".to_owned()
    } else {
        format!("{} days", totals.days_since_first)
    };
    let heading = format!(
        "{icon}_________Total {word} Stars/Date {arrow} {} stars ({sign}{:.2}%)␥{span}_________",
        totals.total, totals.percent,
    );
    let blocks = ordered
        .into_iter()
        .map(|event| {
            let users = side_users(event, side);
            HistoryBlock {
                header: format!(
                    "📅 Date Range Of {word} Stars 【{}】 :: {} — {} users__{}",
                    format_interval(event.scanned_at - event.prev_scanned_at),
                    zone.format(event.prev_scanned_at, EVENT_TIME),
                    zone.format(event.scanned_at, EVENT_TIME),
                    side_total(event, side),
                ),
                users: users.to_vec(),
            }
        })
        .collect();
    let dups = dups
        .iter()
        .map(|(login, count)| Dup {
            login: login.clone(),
            count: *count,
        })
        .collect();
    HistoryPage {
        repo: repo.to_owned(),
        heading,
        blocks,
        dups,
    }
    .render()
    .expect("history template renders")
}

pub fn date_users_json(stars: &[Star]) -> String {
    let mut by_day: BTreeMap<NaiveDate, Vec<&Star>> = BTreeMap::new();
    for star in stars {
        by_day
            .entry(star.starred_at.date_naive())
            .or_default()
            .push(star);
    }
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (day, mut day_stars) in by_day {
        day_stars.sort_by(|a, b| {
            a.starred_at
                .cmp(&b.starred_at)
                .then_with(|| a.login.cmp(&b.login))
        });
        let key = format!("{day}  ({} stars)", day_stars.len());
        let links = day_stars
            .iter()
            .map(|star| format!("https://github.com/{}", star.login))
            .collect();
        map.insert(key, links);
    }
    serde_json::to_string_pretty(&map).expect("string map serializes")
}

pub fn cross_users_txt(repo: &str, users: &[CrossUser]) -> String {
    let mut sorted: Vec<&CrossUser> = users.iter().collect();
    sorted.sort_by(|a, b| {
        b.repos
            .len()
            .cmp(&a.repos.len())
            .then_with(|| a.login.cmp(&b.login))
    });
    let others: BTreeSet<&str> = users
        .iter()
        .flat_map(|user| user.repos.iter().map(String::as_str))
        .filter(|name| *name != repo)
        .collect();
    let mut out = String::from("<dynamically updated file>\n\n");
    out.push_str(&format!(
        "CROSS-USERS [regarding the repository: {repo}]:: {} users\n",
        users.len()
    ));
    out.push_str(&format!(
        "CROSS-USERS [found in repositories: 1 VS {}]:: ({repo}) VS ({}) repositories\n\n",
        others.len(),
        others.into_iter().collect::<Vec<_>>().join(", ")
    ));
    for user in sorted {
        out.push_str(&format!(
            "https://github.com/{} ({}):\n   {} \n\n",
            user.login,
            user.repos.len(),
            user.repos.join("\n   ")
        ));
    }
    out
}

pub fn background_url(language: Option<&str>) -> Option<String> {
    let language = language?.to_lowercase();
    if !BACKGROUND_LANGUAGES.contains(&language.as_str()) {
        return None;
    }
    let slug = match language.as_str() {
        "c#" => "csharp",
        "c++" => "cpp",
        "visual basic .net" => "vbnet",
        other => other,
    };
    Some(format!(
        "https://raw.githubusercontent.com/GlobalArtInc/stargauge/main/images/backgrounds/{slug}.jpg"
    ))
}

fn side_users(event: &ScanEvent, side: Side) -> &[String] {
    match side {
        Side::Added => &event.added,
        Side::Removed => &event.removed,
    }
}

fn side_total(event: &ScanEvent, side: Side) -> u64 {
    match side {
        Side::Added => event.added_total(),
        Side::Removed => event.removed_total(),
    }
}

fn share_percent(count: u64, stargazers: u64) -> String {
    let percent = if stargazers == 0 {
        0.0
    } else {
        count as f64 * 100.0 / stargazers as f64
    };
    format!("{percent:.2}")
}

fn peak_text(peak: Option<(NaiveDate, u32)>) -> String {
    match peak {
        Some((date, count)) => format!("{count} / {date}"),
        None => "— / —".to_owned(),
    }
}

fn opt_text<T: ToString>(value: Option<T>) -> String {
    value.map_or_else(|| "—".to_owned(), |value| value.to_string())
}

fn weekday_name(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
    }
}

fn month_name(month: u32) -> &'static str {
    month
        .checked_sub(1)
        .and_then(|index| MONTHS.get(index as usize))
        .copied()
        .unwrap_or("—")
}

fn distribution_lines(quartiles: Option<&Quartiles>, month: bool) -> Vec<String> {
    let Some(quartiles) = quartiles else {
        return vec![NOT_ENOUGH_DATA.to_owned()];
    };
    let mut top = share_lines(&quartiles.top, "_STARS___", |label| {
        if month {
            label.to_uppercase()
        } else {
            label.to_owned()
        }
    });
    if top.is_empty() {
        top.push(EMPTY_GROUP.to_owned());
    }
    let mut lines = top;
    lines.extend(share_lines(&quartiles.middle, "_Stars___", |label| {
        label.to_owned()
    }));
    let mut low = share_lines(&quartiles.low, "_stars___", |label| {
        if month {
            label.to_lowercase()
        } else {
            label.to_owned()
        }
    });
    if low.is_empty() {
        low.push(EMPTY_GROUP.to_owned());
    }
    lines.extend(low);
    lines
}

fn share_lines(shares: &[Share], suffix: &str, label: impl Fn(&str) -> String) -> Vec<String> {
    shares
        .iter()
        .map(|share| {
            format!(
                "{}: {}{suffix}({:.1}%)",
                label(&share.label),
                share.stars,
                share.percent
            )
        })
        .collect()
}

fn gap_text(gap: Option<&Gap>) -> String {
    match gap {
        Some(gap) => {
            let end = gap.start + chrono::Days::new(u64::from(gap.days));
            format!("{} — {end} ({} days)", gap.start, gap.days)
        }
        None => "— — — (— days)".to_owned(),
    }
}

fn median_change_text(last30: Option<&Last30>) -> String {
    match last30 {
        None => NOT_ENOUGH_DATA.to_owned(),
        Some(last30) => last30
            .median_change_percent
            .map_or_else(|| "—".to_owned(), |percent| format!("{percent:.2} %")),
    }
}

fn average_change_text(last30: Option<&Last30>) -> String {
    match last30 {
        None => format!("{NOT_ENOUGH_DATA} (-)"),
        Some(last30) => {
            let times = last30
                .mean_change_times
                .map_or_else(|| "—".to_owned(), |times| format!("{times:.2} times"));
            format!("{times} ({} stars)", last30.sum_change)
        }
    }
}

fn marketing_text(marketing: Marketing) -> &'static str {
    match marketing {
        Marketing::NotEnoughData => NOT_ENOUGH_DATA,
        Marketing::None => "—",
        Marketing::Low => "Low",
        Marketing::Medium => "Medium",
        Marketing::High => "High",
        Marketing::Hard => "Hard",
        Marketing::HardPlus => "Hard+",
    }
}

fn fake_text(marketing: Marketing) -> &'static str {
    match marketing {
        Marketing::NotEnoughData => NOT_ENOUGH_DATA,
        Marketing::Hard => "Yes",
        Marketing::HardPlus => "Yes, multiple attempts to promote fake stars",
        Marketing::None | Marketing::Low | Marketing::Medium | Marketing::High => "—",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0).unwrap()
    }

    fn meta(description: Option<&str>) -> RepoMeta {
        RepoMeta {
            full_name: "octo/cat".to_owned(),
            description: description.map(str::to_owned),
            language: Some("Rust".to_owned()),
            size_kb: Some(2048),
            stargazers_count: 4,
            created_at: Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
            pushed_at: Some(at(30, 12)),
        }
    }

    fn stats() -> Stats {
        Stats {
            age_days: 100,
            daily: Vec::new(),
            cumulative: Vec::new(),
            peak: None,
            trend_per_day: 1,
            mean_per_day: 0.0,
            mean_per_year: None,
            last30: None,
            stdev: None,
            marketing: Marketing::NotEnoughData,
            by_weekday: [0; 7],
            by_month: Vec::new(),
            by_year: Vec::new(),
            by_hour: Some([0; 24]),
            best_weekday: None,
            worst_weekday: None,
            best_month: None,
            worst_month: None,
            month_quartiles: None,
            year_quartiles: None,
            fetched_stars: 4,
            private_stars: 0,
            longest_gap: None,
        }
    }

    fn render_report(description: Option<&str>) -> String {
        let diff = Diff {
            added: vec!["alice".to_owned()],
            removed: Vec::new(),
        };
        render_with(description, Source::List, ReportChanges::Logins(&diff))
    }

    fn render_with(
        description: Option<&str>,
        source: Source,
        changes: ReportChanges<'_>,
    ) -> String {
        let meta = meta(description);
        let stats = stats();
        let zone = Zone::Utc;
        report(&ReportInput {
            repo_url: "https://github.com/octo/cat",
            meta: &meta,
            stats: &stats,
            source,
            changes,
            prev_scanned_at: at(29, 9),
            scanned_at: at(30, 9),
            zone: &zone,
            tz_label: "UTC",
            has_gone_graph: false,
            background_url: None,
        })
    }

    #[test]
    fn report_escapes_description() {
        let html = render_report(Some("\"><img src=x onerror=alert(1)>"));
        assert!(!html.contains("<img src=x"));
        assert!(html.contains("&#60;img src=x"));
    }

    #[test]
    fn report_has_no_file_urls() {
        assert!(!render_report(None).contains("file://"));
    }

    #[test]
    fn report_without_names_shows_counts_and_no_user_files() {
        let html = render_with(
            None,
            Source::History,
            ReportChanges::Counts {
                counts: StarCounts {
                    added: 3,
                    removed: 1,
                },
                prev_stars: 2,
            },
        );
        assert!(html.contains("New stars: +3"));
        assert!(html.contains("public star history"));
        assert!(!html.contains("cross_users.txt"));
        assert!(!html.contains("date_users.json"));
        assert!(!html.contains("<ol>"));
        assert!(!html.contains("href=\"new_stars.html\""));
        assert!(!html.contains("href=\"gone_stars.html\""));
    }

    #[test]
    fn history_buttons_follow_the_source_not_the_kind_of_changes() {
        let counts = || ReportChanges::Counts {
            counts: StarCounts {
                added: 3,
                removed: 1,
            },
            prev_stars: 2,
        };
        let list = render_with(None, Source::List, counts());
        assert!(list.contains("href=\"new_stars.html\""));
        assert!(list.contains("href=\"gone_stars.html\""));
        let history = render_with(None, Source::History, counts());
        assert!(!history.contains("href=\"new_stars.html\""));
    }

    #[test]
    fn background_url_maps_special_language_names() {
        let base =
            "https://raw.githubusercontent.com/GlobalArtInc/stargauge/main/images/backgrounds";
        assert_eq!(
            background_url(Some("C#")),
            Some(format!("{base}/csharp.jpg"))
        );
        assert_eq!(background_url(Some("C++")), Some(format!("{base}/cpp.jpg")));
        assert_eq!(
            background_url(Some("Visual Basic .NET")),
            Some(format!("{base}/vbnet.jpg"))
        );
    }

    #[test]
    fn background_url_is_none_for_unknown_language() {
        assert_eq!(background_url(Some("Brainfuck")), None);
        assert_eq!(background_url(None), None);
    }

    #[test]
    fn date_users_json_groups_by_utc_day_in_order() {
        let stars = vec![
            Star {
                login: "bob".to_owned(),
                starred_at: at(2, 9),
            },
            Star {
                login: "alice".to_owned(),
                starred_at: at(2, 9),
            },
            Star {
                login: "carol".to_owned(),
                starred_at: at(1, 23),
            },
        ];
        let expected = "{\n  \"2026-10-01  (1 stars)\": [\n    \"https://github.com/carol\"\n  ],\n  \"2026-10-02  (2 stars)\": [\n    \"https://github.com/alice\",\n    \"https://github.com/bob\"\n  ]\n}";
        assert_eq!(date_users_json(&stars), expected);
    }

    #[test]
    fn history_page_has_total_header_and_logins() {
        let events = vec![ScanEvent {
            scanned_at: at(2, 9),
            prev_scanned_at: at(1, 9),
            stars_total: 5,
            added: vec!["alice".to_owned()],
            removed: Vec::new(),
            counts: None,
        }];
        let totals = HistoryTotals {
            total: 5,
            percent: 25.0,
            days_since_first: 3,
        };
        let html = history_page(Side::Added, "octo/cat", &events, &totals, &[], &Zone::Utc);
        assert!(html.contains("Total New Stars/Date ↝ 5 stars (+25.00%)␥1 scan_________"));
        assert!(html.contains("https://github.com/alice"));
    }

    #[test]
    fn history_page_shows_counts_without_names() {
        let events = vec![ScanEvent {
            scanned_at: at(2, 9),
            prev_scanned_at: at(1, 9),
            stars_total: 9,
            added: Vec::new(),
            removed: Vec::new(),
            counts: Some(StarCounts {
                added: 7,
                removed: 0,
            }),
        }];
        let totals = HistoryTotals {
            total: 9,
            percent: 0.0,
            days_since_first: 1,
        };
        let html = history_page(Side::Added, "octo/cat", &events, &totals, &[], &Zone::Utc);
        assert!(html.contains("users__7"));
        assert!(html.contains("names are not available (public history)"));
    }

    #[test]
    fn cross_users_txt_starts_with_headers() {
        let users = vec![CrossUser {
            login: "alice".to_owned(),
            repos: vec!["octo/cat".to_owned(), "octo/dog".to_owned()],
        }];
        let txt = cross_users_txt("octo/cat", &users);
        assert!(txt.starts_with(
            "<dynamically updated file>\n\nCROSS-USERS [regarding the repository: octo/cat]:: 1 users\nCROSS-USERS [found in repositories: 1 VS 1]:: (octo/cat) VS (octo/dog) repositories\n\n"
        ));
    }
}
