use crate::error::ApiError;
use crate::model::{
    CrossUser, Diff, Marketing, Quartiles, RepoMeta, RepoState, Share, Source, StarCounts, Stats,
    Zone, format_interval,
};
use crate::render::textchart;
use chrono::{DateTime, Days, NaiveDate, Utc, Weekday};
use comfy_table::presets::UTF8_FULL;
use comfy_table::{ContentArrangement, Table};
use console::{StyledObject, style};
use indicatif::{ProgressBar, ProgressStyle};
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

const BANNER_WIDE: &str = include_str!("../../assets/banner_wide.txt");
const BANNER_NARROW: &str = include_str!("../../assets/banner_narrow.txt");
const DAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const NO_DATA: &str = "The repository is still young, not enough data";
const CHART_HEIGHT: usize = 12;

#[derive(Clone, Copy)]
enum Tier {
    Top,
    Middle,
    Low,
}

impl Tier {
    fn word(self) -> &'static str {
        match self {
            Tier::Top => "STARS",
            Tier::Middle => "Stars",
            Tier::Low => "stars",
        }
    }

    fn paint(self, text: &str, bold: bool) -> StyledObject<&str> {
        let painted = match self {
            Tier::Top => style(text).green(),
            Tier::Middle => style(text).yellow(),
            Tier::Low => style(text).red(),
        };
        if bold { painted.bold() } else { painted }
    }
}

pub fn banner(out: &mut dyn Write, width: u16, version: &str) -> io::Result<()> {
    if width > 100 {
        writeln!(out, "{}", style(BANNER_WIDE.trim_end()).yellow())?;
    } else if width > 49 {
        writeln!(out, "{}", style(BANNER_NARROW.trim_end()).yellow())?;
    } else {
        writeln!(out, "{}", style("stargauge").yellow())?;
    }
    writeln!(
        out,
        "stargauge {version} - based on Shotstars by snooppr. GPL-3.0-or-later, NO WARRANTY; see --version."
    )?;
    writeln!(out)
}

pub fn repo_info(
    out: &mut dyn Write,
    meta: &RepoMeta,
    zone: &Zone,
    tz_label: &str,
    today: NaiveDate,
) -> io::Result<()> {
    let size = match meta.size_kb {
        Some(kb) => format!("{:.2}", kb as f64 / 1024.0),
        None => "N/O".to_string(),
    };
    let days = (today - meta.created_at.date_naive()).num_days();
    let updated = match meta.pushed_at {
        Some(t) => format!("{}_{tz_label}", zone.format(t, "%Y-%m-%d__%H:%M")),
        None => "N/O".to_string(),
    };
    let description = meta
        .description
        .as_deref()
        .unwrap_or("No repository description available");
    writeln!(out, "{} ~{size} MB", style("Size::").cyan())?;
    writeln!(
        out,
        "{} {} stars",
        style("GitHub-rating::").cyan(),
        meta.stargazers_count
    )?;
    writeln!(
        out,
        "{} {} ({days} days)",
        style("Date-of-creation::").cyan(),
        meta.created_at.format("%Y-%m-%d")
    )?;
    writeln!(
        out,
        "{} {updated}",
        style("Date-update (including hidden update)::").cyan()
    )?;
    writeln!(
        out,
        "{} {description}",
        style("Repository-description::").cyan()
    )
}

pub fn stats_lines(out: &mut dyn Write, stats: &Stats) -> io::Result<()> {
    let peak = match stats.peak {
        Some((date, n)) => format!("{n} stars / {}", date.format("%Y-%m-%d")),
        None => "—".to_string(),
    };
    writeln!(out, "{} {peak}", style("Peak-stars-in-date::").cyan())?;
    writeln!(
        out,
        "{} {} stars / day",
        style("The-trend-of-adding-stars (forecasting)::").cyan(),
        stats.trend_per_day
    )?;
    writeln!(
        out,
        "{} {} / {}",
        style("Most-of-stars-d.w. / Smallest-of-stars-d.w.::").cyan(),
        weekday_name(stats.best_weekday),
        weekday_name(stats.worst_weekday)
    )?;
    writeln!(
        out,
        "{} {} / {}",
        style("Most-of-stars-month / Smallest-of-stars-month::").cyan(),
        month_name(stats.best_month),
        month_name(stats.worst_month)
    )?;

    let total: u64 = stats.by_month.iter().map(|&(_, s)| s).sum();
    if stats.age_days > 32 {
        let private = if stats.private_stars != 0 {
            format!(" ({} private stars)", stats.private_stars)
        } else {
            String::new()
        };
        writeln!(
            out,
            "{}",
            style(format!(
                "Distribution-of-stars-by-month{private} ({total} stars)::"
            ))
            .cyan()
        )?;
        match &stats.month_quartiles {
            Some(q) => quartile_block(out, q, true)?,
            None => writeln!(out, "{NO_DATA}")?,
        }
    }
    if stats.age_days > 365 {
        writeln!(
            out,
            "{}",
            style(format!("Distribution-of-stars-by-year ({total} stars)::")).cyan()
        )?;
        match &stats.year_quartiles {
            Some(q) => quartile_block(out, q, false)?,
            None => writeln!(out, "{NO_DATA}")?,
        }
    }

    let gap = match &stats.longest_gap {
        Some(gap) => {
            let end = gap
                .start
                .checked_add_days(Days::new(u64::from(gap.days)))
                .map_or("—".to_string(), |d| d.format("%Y-%m-%d").to_string());
            format!(
                "{} — {end} ({} days)",
                gap.start.format("%Y-%m-%d"),
                gap.days
            )
        }
        None => "— — — (— days)".to_string(),
    };
    writeln!(
        out,
        "{} {gap}",
        style("Longest-period-without-add-stars::").cyan()
    )?;

    let (median, average) = match &stats.last30 {
        Some(last30) => (
            last30
                .median_change_percent
                .map_or("—".to_string(), |p| format!("{p:.2} %")),
            format!(
                "{} ({} stars)",
                last30
                    .mean_change_times
                    .map_or("—".to_string(), |t| format!("{t:.2} times")),
                last30.sum_change
            ),
        ),
        None => (NO_DATA.to_string(), format!("{NO_DATA} (-)")),
    };
    writeln!(
        out,
        "{} {median}",
        style("Median-percentage-change (in the last 30 days)::").cyan()
    )?;
    writeln!(
        out,
        "{} {average}",
        style("Average-change-in-fact (in the last 30 days)::").cyan()
    )?;
    writeln!(
        out,
        "{} {}",
        style("Aggressive-marketing::").cyan(),
        marketing_label(stats.marketing)
    )?;
    writeln!(
        out,
        "{} {}",
        style("Fake-stars::").cyan(),
        fake_stars_label(stats.marketing)
    )?;
    writeln!(out)
}

fn weekday_name(day: Option<Weekday>) -> &'static str {
    day.map_or("—", |d| DAYS[d.num_days_from_monday() as usize])
}

fn month_name(month: Option<u32>) -> &'static str {
    month
        .and_then(|m| m.checked_sub(1))
        .and_then(|i| MONTHS.get(i as usize))
        .copied()
        .unwrap_or("—")
}

fn quartile_block(out: &mut dyn Write, quartiles: &Quartiles, months: bool) -> io::Result<()> {
    let groups = [
        (&quartiles.top, Tier::Top),
        (&quartiles.middle, Tier::Middle),
        (&quartiles.low, Tier::Low),
    ];
    for (shares, tier) in groups {
        let mut lines = quartile_lines(shares, months, tier);
        if lines.is_empty() && !matches!(tier, Tier::Middle) {
            lines.push("-".repeat(25));
        }
        for line in lines {
            writeln!(out, "{}", tier.paint(&line, months))?;
        }
    }
    Ok(())
}

fn quartile_lines(shares: &[Share], months: bool, tier: Tier) -> Vec<String> {
    shares
        .iter()
        .map(|share| {
            let label = match (months, tier) {
                (true, Tier::Top) => share.label.to_uppercase(),
                (true, Tier::Low) => share.label.to_lowercase(),
                _ => share.label.clone(),
            };
            format!(
                "{label}: {}_{}___({:.1}%)",
                share.stars,
                tier.word(),
                share.percent
            )
        })
        .collect()
}

fn marketing_label(marketing: Marketing) -> String {
    match marketing {
        Marketing::NotEnoughData => NO_DATA.to_string(),
        Marketing::None => "—".to_string(),
        Marketing::Low => style("Low").green().to_string(),
        Marketing::Medium => style("Medium").yellow().to_string(),
        Marketing::High => style("High").red().to_string(),
        Marketing::Hard => style("Hard").black().on_red().to_string(),
        Marketing::HardPlus => style("Hard+").black().on_red().to_string(),
    }
}

fn fake_stars_label(marketing: Marketing) -> &'static str {
    match marketing {
        Marketing::NotEnoughData => NO_DATA,
        Marketing::None | Marketing::Low | Marketing::Medium | Marketing::High => "—",
        Marketing::Hard => "Yes",
        Marketing::HardPlus => "Yes, multiple attempts to promote fake stars",
    }
}

pub fn diff_tables(
    out: &mut dyn Write,
    diff: &Diff,
    stars_total: u64,
    interval: chrono::Duration,
) -> io::Result<()> {
    if diff.removed.is_empty() {
        writeln!(
            out,
            "{}",
            style("GONE stars not detected").bold().black().on_white()
        )?;
    }
    if diff.added.is_empty() {
        writeln!(
            out,
            "{}",
            style("NEW stars not detected").bold().black().on_white()
        )?;
    }
    let interval = format_interval(interval);
    if !diff.removed.is_empty() {
        let percent = share_percent(diff.removed.len(), stars_total);
        writeln!(
            out,
            "{}",
            style(format!(
                "Gone stars (-{percent:.2}%)\nin the last: {interval}"
            ))
            .yellow()
        )?;
        writeln!(out, "{}", login_table("GONE STARS", &diff.removed))?;
    }
    if !diff.added.is_empty() {
        let percent = share_percent(diff.added.len(), stars_total);
        writeln!(
            out,
            "{}",
            style(format!(
                "New stars (+{percent:.2}%)\nin the last: {interval}"
            ))
            .cyan()
        )?;
        writeln!(out, "{}", login_table("NEW STARS", &diff.added))?;
    }
    Ok(())
}

pub fn count_changes(
    out: &mut dyn Write,
    prev_stars: u64,
    now_stars: u64,
    counts: Option<&StarCounts>,
    new_by_day: &[(NaiveDate, u32)],
    interval: chrono::Duration,
) -> io::Result<()> {
    writeln!(
        out,
        "{} {}",
        style("Changes in the last:").cyan(),
        format_interval(interval)
    )?;
    let net = now_stars as i64 - prev_stars as i64;
    writeln!(
        out,
        "{} {prev_stars} -> {now_stars} ({net:+})",
        style("GitHub-rating::").cyan()
    )?;
    let Some(counts) = counts else {
        return Ok(());
    };
    writeln!(
        out,
        "{} +{}",
        style("New stars (public history)::").cyan(),
        counts.added
    )?;
    writeln!(
        out,
        "{} -{}",
        style("Gone stars (public history, at least)::").cyan(),
        counts.removed
    )?;
    if !new_by_day.is_empty() {
        let mut table = base_table();
        table.set_header(vec!["DATE", "NEW STARS"]);
        for (date, n) in new_by_day {
            table.add_row(vec![date.format("%Y-%m-%d").to_string(), n.to_string()]);
        }
        writeln!(out, "{table}")?;
    }
    Ok(())
}

pub fn notice(out: &mut dyn Write, text: &str) -> io::Result<()> {
    writeln!(out, "{}", style(text).bold().yellow())
}

fn share_percent(count: usize, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        count as f64 * 100.0 / total as f64
    }
}

fn login_table(header: &str, logins: &[String]) -> Table {
    let mut table = base_table();
    table.set_header(vec!["N", header]);
    for (i, login) in logins.iter().enumerate() {
        table.add_row(vec![
            (i + 1).to_string(),
            format!("https://github.com/{login}"),
        ]);
    }
    table
}

fn base_table() -> Table {
    let mut table = Table::new();
    table
        .load_style(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table
}

pub fn history_table(out: &mut dyn Write, states: &[RepoState], zone: &Zone) -> io::Result<()> {
    writeln!(out, "{}", style("SCAN HISTORY").blue().bold())?;
    let mut table = base_table();
    table.set_header(vec!["N", "URL", "STARS", "MODE", "LAST SCAN"]);
    for (i, state) in states.iter().enumerate() {
        table.add_row(vec![
            (i + 1).to_string(),
            format!("https://github.com/{}", state.repo),
            state.stars_total.to_string(),
            source_label(state.source).to_string(),
            zone.format(state.scanned_at, "%Y-%m-%d"),
        ]);
    }
    writeln!(out, "{table}")
}

fn source_label(source: Source) -> &'static str {
    match source {
        Source::List => "list",
        Source::History => "public",
    }
}

pub fn cross_users(out: &mut dyn Write, repo: &str, users: &[CrossUser]) -> io::Result<()> {
    if users.is_empty() {
        return Ok(());
    }
    let mut others: Vec<&str> = users
        .iter()
        .flat_map(|user| user.repos.iter().map(String::as_str))
        .filter(|name| *name != repo)
        .collect();
    others.sort_unstable();
    others.dedup();

    writeln!(
        out,
        "{}",
        style(format!(
            "#### cross-users ({}), regarding the repository ({repo}) ####",
            users.len()
        ))
        .bold()
        .blue()
    )?;
    writeln!(out)?;

    let mut panel = base_table();
    panel.set_header(vec![format!(
        "cross-users found in repositories (1 VS {})",
        others.len()
    )]);
    panel.add_row(vec![format!(
        "{} VS {}",
        style(format!("({repo})")).yellow(),
        style(format!("({})", others.join(", "))).yellow()
    )]);
    writeln!(out, "{panel}")?;

    let mut sorted: Vec<&CrossUser> = users.iter().collect();
    sorted.sort_by(|a, b| {
        b.repos
            .len()
            .cmp(&a.repos.len())
            .then_with(|| a.login.cmp(&b.login))
    });
    writeln!(
        out,
        "{}",
        style("CROSS-USERS (in CLI = max 5 users, in HTML = full users)")
            .red()
            .bold()
    )?;
    let mut table = base_table();
    table.set_header(vec!["N", "Q/S", "GitHub username", "GitHub repositories"]);
    for (i, user) in sorted.iter().take(5).enumerate() {
        table.add_row(vec![
            (i + 1).to_string(),
            user.repos.len().to_string(),
            format!("https://github.com/{}", user.login),
            user.repos.join(", "),
        ]);
    }
    writeln!(out, "{table}")
}

pub fn api_error(
    out: &mut dyn Write,
    err: &ApiError,
    has_token: bool,
    config_path: &Path,
    zone: &Zone,
) -> io::Result<()> {
    match err {
        ApiError::NotFound => {
            writeln!(
                out,
                "{}",
                style("Check the entered url, it seems that such a repository does not exist.")
                    .bold()
                    .red()
            )?;
            if has_token {
                writeln!(
                    out,
                    "{}",
                    style(format!(
                        "[!] Your GitHub token may have expired (default: token = None), check: {}",
                        config_path.display()
                    ))
                    .bold()
                    .red()
                )?;
            }
        }
        ApiError::BadToken => writeln!(
            out,
            "{}",
            style(format!(
                "[!] GitHub rejected the token, check: {}",
                config_path.display()
            ))
            .bold()
            .red()
        )?,
        ApiError::RateLimited { reset_at, .. } => rate_limited(out, *reset_at, has_token, zone)?,
        ApiError::TooLarge { pages, max } => notice(
            out,
            &format!(
                "[!] GitHub serves at most {max} pages of the public star history, this repository needs {pages}."
            ),
        )?,
        ApiError::BudgetTooSmall {
            pages,
            remaining,
            reset_at,
        } => {
            let mut text = format!(
                "[!] {pages} pages needed, only {remaining} requests left in the rate limit budget."
            );
            if let Some(t) = reset_at {
                text.push_str(&format!(
                    " The budget resets at {}.",
                    zone.format(*t, "%Y-%m-%d_%H:%M")
                ));
            }
            writeln!(out, "{}", style(text).bold().yellow())?;
        }
        ApiError::Network(message) => {
            writeln!(out, "{}", style("Network error!").bold().red())?;
            writeln!(out, "{message}")?;
        }
        ApiError::Http { status, snippet } => writeln!(
            out,
            "{}",
            style(format!("GitHub API returned HTTP {status}: {snippet}")).red()
        )?,
        ApiError::Interrupted => writeln!(out, "{}", style("Interrupted").yellow())?,
    }
    Ok(())
}

fn rate_limited(
    out: &mut dyn Write,
    reset_at: Option<DateTime<Utc>>,
    has_token: bool,
    zone: &Zone,
) -> io::Result<()> {
    match reset_at {
        Some(t) => {
            let minutes = (t - Utc::now()).num_seconds().max(0) / 60;
            writeln!(
                out,
                "{}",
                style(format!(
                    "Attention! The API limit has probably been exceeded, the block will presumably be lifted: {} ::: ({minutes} min.)",
                    zone.format(t, "%Y-%m-%d_%H:%M")
                ))
                .bold()
                .red()
            )?;
        }
        None => writeln!(
            out,
            "{}",
            style("Attention! The API limit has probably been exceeded.")
                .bold()
                .red()
        )?,
    }
    if has_token {
        writeln!(
            out,
            "{}",
            limit_panel(
                "GitHub API/Token Used",
                "Limitations: ~limit max '5,000 requests/hour'"
            )
        )?;
    } else {
        writeln!(
            out,
            "{}",
            limit_panel(
                "GitHub API/No Token",
                "Limitations: ~limit max '60 requests/hour'"
            )
        )?;
        notice(
            out,
            "[!] A GitHub token raises the limit to 5,000 requests per hour. See the README section \"GitHub token and limits\".",
        )?;
    }
    Ok(())
}

fn limit_panel(title: &str, body: &str) -> Table {
    let mut panel = base_table();
    panel.set_header(vec![title]);
    panel.add_row(vec![body]);
    panel
}

pub fn cli_charts(
    out: &mut dyn Write,
    stats: &Stats,
    gone: &[(NaiveDate, u32)],
    repo: &str,
    width: u16,
) -> io::Result<()> {
    let width = usize::from(width);
    textchart::columns(
        out,
        &format!("ALL_STARS, '{repo}' (real parsing)"),
        &stats.daily,
        width,
        CHART_HEIGHT,
    )?;
    writeln!(out)?;
    if !gone.is_empty() {
        textchart::columns(
            out,
            &format!("GONE_STARS, '{repo}' (calculations)"),
            gone,
            width,
            CHART_HEIGHT,
        )?;
        writeln!(out)?;
    }

    let years: Vec<(String, u64)> = stats
        .by_year
        .iter()
        .map(|&(year, s)| (year.to_string(), s))
        .collect();
    textchart::bars(out, "all stars by YEAR", &years, width)?;
    writeln!(out)?;

    let months: Vec<(String, u64)> = stats
        .by_month
        .iter()
        .map(|&(month, s)| (month_name(Some(month)).to_string(), s))
        .collect();
    textchart::bars(out, "all stars by MONTH", &months, width)?;
    writeln!(out)?;

    let weekdays: Vec<(String, u64)> = DAYS
        .iter()
        .zip(stats.by_weekday)
        .map(|(day, s)| (day.to_string(), s))
        .collect();
    textchart::bars(out, "all stars by DAYS OF THE WEEK", &weekdays, width)?;
    writeln!(out)?;

    if let Some(by_hour) = stats.by_hour {
        let hours: Vec<(String, u64)> = by_hour
            .iter()
            .enumerate()
            .map(|(hour, s)| (hour_label(hour as u32), *s))
            .collect();
        textchart::bars(out, "all stars by HOUR", &hours, width)?;
        writeln!(out)?;
    }
    Ok(())
}

fn hour_label(hour: u32) -> String {
    let hour12 = if hour.is_multiple_of(12) {
        12
    } else {
        hour % 12
    };
    let meridiem = if hour < 12 { "AM" } else { "PM" };
    format!("{hour:02}:00 ({hour12:02}:00 {meridiem})")
}

pub fn spinner(msg: &str) -> ProgressBar {
    let bar = ProgressBar::new_spinner().with_message(msg.to_string());
    bar.enable_steady_tick(Duration::from_millis(100));
    bar
}

pub fn progress(pages: u64) -> ProgressBar {
    let style = ProgressStyle::with_template("{bar:40} {pos}/{len} pages")
        .expect("progress template is valid");
    ProgressBar::new(pages).with_style(style)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(buf: Vec<u8>) -> String {
        console::strip_ansi_codes(&String::from_utf8(buf).unwrap()).to_string()
    }

    fn young_stats() -> Stats {
        Stats {
            age_days: 10,
            daily: vec![],
            cumulative: vec![],
            peak: Some((NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(), 5)),
            trend_per_day: 2,
            mean_per_day: 1.0,
            mean_per_year: None,
            last30: None,
            stdev: None,
            marketing: Marketing::NotEnoughData,
            by_weekday: [0; 7],
            by_month: vec![],
            by_year: vec![],
            by_hour: Some([0; 24]),
            best_weekday: None,
            worst_weekday: None,
            best_month: None,
            worst_month: None,
            month_quartiles: None,
            year_quartiles: None,
            fetched_stars: 5,
            private_stars: 0,
            longest_gap: None,
        }
    }

    #[test]
    fn diff_tables_report_new_and_missing_gone() {
        let diff = Diff {
            added: vec!["alice".to_string()],
            removed: vec![],
        };
        let mut buf = Vec::new();
        diff_tables(&mut buf, &diff, 4, chrono::Duration::minutes(1565)).unwrap();
        let text = plain(buf);
        assert!(text.contains("GONE stars not detected"));
        assert!(text.contains("New stars (+25.00%)\nin the last: 1d. 02h. 05m."));
        assert!(text.contains("https://github.com/alice"));
        assert!(!text.contains("Gone stars"));
    }

    #[test]
    fn stats_lines_for_young_repository() {
        let mut buf = Vec::new();
        stats_lines(&mut buf, &young_stats()).unwrap();
        let text = plain(buf);
        assert!(text.contains("Peak-stars-in-date:: 5 stars / 2026-03-01"));
        assert!(text.contains("The-trend-of-adding-stars (forecasting):: 2 stars / day"));
        assert!(text.contains(
            "Average-change-in-fact (in the last 30 days):: The repository is still young, not enough data (-)"
        ));
        assert!(text.contains("Fake-stars:: The repository is still young, not enough data"));
        assert!(!text.contains("Distribution-of-stars-by-month"));
    }

    #[test]
    fn api_error_rate_limited_shows_minutes_until_reset() {
        let err = ApiError::RateLimited {
            reset_at: Some(
                Utc::now() + chrono::Duration::minutes(90) + chrono::Duration::seconds(30),
            ),
            secondary: false,
        };
        let mut buf = Vec::new();
        api_error(
            &mut buf,
            &err,
            false,
            Path::new("/cfg/config.toml"),
            &Zone::Utc,
        )
        .unwrap();
        let text = plain(buf);
        assert!(text.contains("the block will presumably be lifted:"));
        assert!(text.contains("(90 min.)"));
        assert!(text.contains("GitHub API/No Token"));
    }

    #[test]
    fn count_changes_prints_rating_counts_and_day_table() {
        let counts = StarCounts {
            added: 7,
            removed: 2,
        };
        let day = NaiveDate::from_ymd_opt(2026, 10, 30).unwrap();
        let mut buf = Vec::new();
        count_changes(
            &mut buf,
            100,
            105,
            Some(&counts),
            &[(day, 7)],
            chrono::Duration::minutes(1565),
        )
        .unwrap();
        let text = plain(buf);
        assert!(text.contains("Changes in the last: 1d. 02h. 05m."));
        assert!(text.contains("GitHub-rating:: 100 -> 105 (+5)"));
        assert!(text.contains("New stars (public history):: +7"));
        assert!(text.contains("Gone stars (public history, at least):: -2"));
        assert!(text.contains("DATE"));
        assert!(text.contains("2026-10-30"));
    }

    #[test]
    fn count_changes_without_counts_prints_only_rating() {
        let mut buf = Vec::new();
        count_changes(
            &mut buf,
            100,
            105,
            None,
            &[],
            chrono::Duration::minutes(1565),
        )
        .unwrap();
        let text = plain(buf);
        assert!(text.contains("GitHub-rating:: 100 -> 105 (+5)"));
        assert!(!text.contains("public history"));
        assert!(!text.contains("DATE"));
    }

    fn repo_state(repo: &str, source: Source) -> RepoState {
        RepoState {
            schema: crate::model::STATE_SCHEMA,
            repo: repo.to_string(),
            scanned_at: Utc::now(),
            stars_total: 42,
            logins: vec![],
            events: vec![],
            source,
            star_days: None,
        }
    }

    #[test]
    fn history_table_shows_mode_per_repository() {
        let states = [
            repo_state("owner/a", Source::List),
            repo_state("owner/b", Source::History),
        ];
        let mut buf = Vec::new();
        history_table(&mut buf, &states, &Zone::Utc).unwrap();
        let text = plain(buf);
        let row = |repo: &str| {
            text.lines()
                .find(|line| line.contains(repo))
                .unwrap_or_default()
        };
        assert!(row("owner/a").contains("list"));
        assert!(row("owner/b").contains("public"));
    }

    #[test]
    fn cli_charts_shows_hour_chart_only_with_hours() {
        let mut stats = young_stats();
        stats.by_hour = None;
        let mut buf = Vec::new();
        cli_charts(&mut buf, &stats, &[], "owner/name", 80).unwrap();
        let text = plain(buf);
        assert!(text.contains("all stars by DAYS OF THE WEEK"));
        assert!(!text.contains("all stars by HOUR"));

        let mut buf = Vec::new();
        cli_charts(&mut buf, &young_stats(), &[], "owner/name", 80).unwrap();
        assert!(plain(buf).contains("all stars by HOUR"));
    }
}
