use crate::diff;
use crate::error::{ApiError, Error};
use crate::github::{self, Client};
use crate::model::{
    CrossUser, Diff, RateInfo, RepoId, RepoMeta, RepoState, STATE_SCHEMA, ScanEvent, Side, Source,
    Star, StarCounts, Zone,
};
use crate::platform;
use crate::render::{charts, html, term};
use crate::stats;
use crate::store::{self, Layout};
use crate::target;
use chrono::{DateTime, NaiveDate, Utc};
use std::collections::{BTreeSet, HashSet};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

const NO_TOKEN_NOTICE: &str = "No GitHub token: using the public star history. It gives statistics and charts, but not the names of new and gone stargazers or the hour of each star.";
const TOKEN_HINT_NOTICE: &str = "A token shows names for repositories you administer or collaborate on, see the README section \"GitHub token and limits\".";
const NAMES_LATER_NOTICE: &str =
    "The previous scan used the public star history: names are compared from the next scan on.";
const LIST_GONE_NOTICE: &str = "The stargazer list is not available for this scan: the saved names are dropped, only star counts are compared from now on.";

pub struct Ctx {
    pub layout: Layout,
    pub client: Client,
    pub zone: Zone,
    pub tz_label: String,
    pub now: DateTime<Utc>,
    pub open_report: bool,
    pub term_width: u16,
}

pub fn output<T>(result: io::Result<T>) -> Result<T, Error> {
    result.map_err(|source| Error::Io {
        path: PathBuf::from("stdout"),
        source,
    })
}

enum Compared {
    Names(Diff),
    Counts(StarCounts),
}

pub fn scan(ctx: &Ctx, id: &RepoId, out: &mut dyn Write) -> Result<(), Error> {
    let started = Instant::now();
    let (meta, rate) = ctx.client.repo(id)?;
    let canonical = target::parse_repo(&meta.full_name)?;
    let name = meta.full_name.clone();
    store::check_data_dir(&ctx.layout)?;
    let today = ctx.zone.date_of(ctx.now);
    output(term::repo_info(out, &meta, &ctx.zone, &ctx.tz_label, today))?;
    let prev = store::load_state(&ctx.layout, &canonical)?;
    if !ctx.client.has_token()
        && let Some(prev) = &prev
        && prev.source == Source::List
        && !prev.logins.is_empty()
    {
        return Err(Error::Input(format!(
            "'{name}' was scanned with the stargazer list, and without a GitHub token its saved names would be dropped: set STARGAUGE_TOKEN or the token in config.toml and scan again"
        )));
    }
    let (source, remaining) = pick_source(ctx, &canonical, &meta, &rate, out)?;
    let pages = check_budget(ctx, source, &meta, &rate, remaining)?;
    let (stars, days) = match source {
        Source::List => (fetch_stars(ctx, &canonical, &meta, pages)?, Vec::new()),
        Source::History => (Vec::new(), fetch_history(ctx, &canonical, pages)?),
    };
    let created = meta.created_at.date_naive();
    let stats = match source {
        Source::List => stats::compute(&stars, created, today, meta.stargazers_count, &ctx.zone),
        Source::History => stats::from_days(&days, created, today),
    };
    output(term::stats_lines(out, &stats))?;
    if source == Source::History {
        let sum: u64 = days.iter().map(|&(_, count)| u64::from(count)).sum();
        if sum < meta.stargazers_count {
            output(term::notice(
                out,
                &format!(
                    "The public history accounts for {sum} of {} stars; GitHub updates it with a delay.",
                    meta.stargazers_count
                ),
            ))?;
        }
    }
    let logins = unique_logins(&stars);

    let Some(prev) = prev else {
        output(writeln!(
            out,
            "A new repository has been added to the tracking database: '{name}'."
        ))?;
        output(writeln!(
            out,
            "On subsequent/re-scanning of the repository, stargauge will calculate the rest of the analytics."
        ))?;
        if source == Source::List {
            store::write_page(
                &ctx.layout,
                &canonical,
                "date_users.json",
                &html::date_users_json(&stars),
            )?;
        }
        store::save_state(
            &ctx.layout,
            &canonical,
            &saved_state(ctx, &meta, source, logins, days, Vec::new()),
        )?;
        return finish(ctx, started, out);
    };

    output(writeln!(
        out,
        "Repository '{name}' was last checked ->  {} ({} days)",
        ctx.zone.format(prev.scanned_at, "%Y-%m-%d_%H:%M"),
        (ctx.now - prev.scanned_at).num_days()
    ))?;
    let (compared, logged) = compare(ctx, out, &prev, &meta, &logins, &days, source)?;
    let mut events = prev.events;
    events.extend(logged);
    let gone = diff::gone_per_day(&events, &ctx.zone);
    let users = match source {
        Source::List => Some(write_list_pages(
            ctx, &canonical, &meta, &stars, &logins, &events,
        )?),
        Source::History => None,
    };
    let changes = match &compared {
        Compared::Names(d) => html::ReportChanges::Logins(d),
        Compared::Counts(counts) => html::ReportChanges::Counts {
            counts: *counts,
            prev_stars: prev.stars_total,
        },
    };
    store::write_page(
        &ctx.layout,
        &canonical,
        "graphs.html",
        &charts::graphs_page(&name, &stats, &ctx.tz_label),
    )?;
    if !gone.is_empty() {
        store::write_page(
            &ctx.layout,
            &canonical,
            "gone_graph.html",
            &charts::gone_graph_page(&name, &gone),
        )?;
    }
    let report = html::report(&html::ReportInput {
        repo_url: &format!("https://github.com/{name}"),
        meta: &meta,
        stats: &stats,
        source,
        changes,
        prev_scanned_at: prev.scanned_at,
        scanned_at: ctx.now,
        zone: &ctx.zone,
        tz_label: &ctx.tz_label,
        has_gone_graph: !gone.is_empty(),
        background_url: html::background_url(meta.language.as_deref()),
    });
    let report_path = store::write_page(&ctx.layout, &canonical, "report.html", &report)?;
    store::ensure_assets(&ctx.layout, &[("echarts.min.js", charts::ECHARTS_JS)])?;
    store::save_state(
        &ctx.layout,
        &canonical,
        &saved_state(ctx, &meta, source, logins, days, events),
    )?;
    output(term::cli_charts(out, &stats, &gone, &name, ctx.term_width))?;
    if let Some(users) = &users {
        output(term::cross_users(out, &name, users))?;
    }
    output(writeln!(out, "Report: {}", report_path.display()))?;
    if ctx.open_report
        && let Err(msg) = platform::open_report(&report_path)
    {
        output(writeln!(
            out,
            "It is impossible to open the web browser ({msg}). Open the report manually."
        ))?;
    }
    finish(ctx, started, out)
}

fn pick_source(
    ctx: &Ctx,
    id: &RepoId,
    meta: &RepoMeta,
    rate: &RateInfo,
    out: &mut dyn Write,
) -> Result<(Source, u32), Error> {
    if !ctx.client.has_token() {
        output(term::notice(out, NO_TOKEN_NOTICE))?;
        output(term::notice(out, TOKEN_HINT_NOTICE))?;
        return Ok((Source::History, rate.remaining));
    }
    if github::pages_for(meta.stargazers_count) > github::MAX_PAGES {
        output(term::notice(
            out,
            &format!(
                "The stargazer list of '{}' has more than 40,000 stars, GitHub does not page it further: using the public star history (no names, no hours).",
                meta.full_name
            ),
        ))?;
        return Ok((Source::History, rate.remaining));
    }
    let remaining = rate.remaining.saturating_sub(1);
    if ctx.client.list_access(id)? {
        return Ok((Source::List, remaining));
    }
    output(term::notice(
        out,
        &format!(
            "The stargazer list of '{}' is open only to its admins and collaborators: using the public star history (no names, no hours).",
            meta.full_name
        ),
    ))?;
    Ok((Source::History, remaining))
}

fn requests_for(stargazers: u64) -> u64 {
    let pages = github::pages_for(stargazers);
    let extra =
        u64::from(stargazers > 0 && stargazers.is_multiple_of(100) && pages < github::MAX_PAGES);
    pages + extra
}

fn check_budget(
    ctx: &Ctx,
    source: Source,
    meta: &RepoMeta,
    rate: &RateInfo,
    remaining: u32,
) -> Result<u64, Error> {
    let (pages, needed) = match source {
        Source::List => (
            github::pages_for(meta.stargazers_count),
            requests_for(meta.stargazers_count),
        ),
        Source::History => {
            let pages = github::history_pages(meta.created_at.date_naive(), ctx.now.date_naive());
            (pages, pages)
        }
    };
    if needed > u64::from(remaining) {
        return Err(ApiError::BudgetTooSmall {
            pages: needed,
            remaining,
            reset_at: rate.reset_at,
        }
        .into());
    }
    Ok(pages)
}

fn fetch_stars(ctx: &Ctx, id: &RepoId, meta: &RepoMeta, pages: u64) -> Result<Vec<Star>, Error> {
    let bar = term::progress(pages);
    let cancel = AtomicBool::new(false);
    let on_page = || bar.inc(1);
    let fetched = ctx
        .client
        .stargazers(id, meta.stargazers_count, &cancel, &on_page);
    bar.finish_and_clear();
    Ok(fetched?)
}

fn fetch_history(ctx: &Ctx, id: &RepoId, pages: u64) -> Result<Vec<(NaiveDate, u32)>, Error> {
    let bar = term::progress(pages);
    let cancel = AtomicBool::new(false);
    let on_page = || bar.inc(1);
    let fetched = ctx.client.star_history(id, &cancel, &on_page);
    bar.finish_and_clear();
    Ok(fetched?)
}

fn compare(
    ctx: &Ctx,
    out: &mut dyn Write,
    prev: &RepoState,
    meta: &RepoMeta,
    logins: &[String],
    days: &[(NaiveDate, u32)],
    source: Source,
) -> Result<(Compared, Option<ScanEvent>), Error> {
    let interval = ctx.now - prev.scanned_at;
    let event = |added: Vec<String>, removed: Vec<String>, counts: Option<StarCounts>| ScanEvent {
        scanned_at: ctx.now,
        prev_scanned_at: prev.scanned_at,
        stars_total: meta.stargazers_count,
        added,
        removed,
        counts,
    };
    match (prev.source, source, prev.star_days.as_deref()) {
        (Source::List, Source::List, _) => {
            let d = diff::diff(&prev.logins, logins);
            output(term::diff_tables(out, &d, meta.stargazers_count, interval))?;
            let logged = (!d.added.is_empty() || !d.removed.is_empty())
                .then(|| event(d.added.clone(), d.removed.clone(), None));
            Ok((Compared::Names(d), logged))
        }
        (Source::History, Source::History, Some(prev_days)) => {
            let (counts, new_by_day) = diff::day_changes(prev_days, days);
            output(term::count_changes(
                out,
                prev.stars_total,
                meta.stargazers_count,
                Some(&counts),
                &new_by_day,
                interval,
            ))?;
            let logged = (counts.added + counts.removed > 0)
                .then(|| event(Vec::new(), Vec::new(), Some(counts)));
            Ok((Compared::Counts(counts), logged))
        }
        (from, to, _) => {
            match (from, to) {
                (Source::List, Source::History) => {
                    output(term::notice(out, LIST_GONE_NOTICE))?;
                }
                (Source::History, Source::List) => {
                    output(term::notice(out, NAMES_LATER_NOTICE))?;
                }
                _ => {}
            }
            let (now, before) = (meta.stargazers_count, prev.stars_total);
            let counts = if now >= before {
                StarCounts {
                    added: now - before,
                    removed: 0,
                }
            } else {
                StarCounts {
                    added: 0,
                    removed: before - now,
                }
            };
            output(term::count_changes(
                out,
                prev.stars_total,
                meta.stargazers_count,
                None,
                &[],
                interval,
            ))?;
            let logged = (now != before).then(|| event(Vec::new(), Vec::new(), Some(counts)));
            Ok((Compared::Counts(counts), logged))
        }
    }
}

fn write_list_pages(
    ctx: &Ctx,
    id: &RepoId,
    meta: &RepoMeta,
    stars: &[Star],
    logins: &[String],
    events: &[ScanEvent],
) -> Result<Vec<CrossUser>, Error> {
    let name = &meta.full_name;
    let today = ctx.zone.date_of(ctx.now);
    for (side, file) in [
        (Side::Added, "new_stars.html"),
        (Side::Removed, "gone_stars.html"),
    ] {
        let page = html::history_page(
            side,
            name,
            events,
            &diff::history_totals(events, side, meta.stargazers_count, today, &ctx.zone),
            &diff::duplicates(events, side),
            &ctx.zone,
        );
        store::write_page(&ctx.layout, id, file, &page)?;
    }
    store::write_page(
        &ctx.layout,
        id,
        "date_users.json",
        &html::date_users_json(stars),
    )?;
    let users = cross_users(ctx, name, logins)?;
    store::write_page(
        &ctx.layout,
        id,
        "cross_users.txt",
        &html::cross_users_txt(name, &users),
    )?;
    Ok(users)
}

fn saved_state(
    ctx: &Ctx,
    meta: &RepoMeta,
    source: Source,
    logins: Vec<String>,
    days: Vec<(NaiveDate, u32)>,
    events: Vec<ScanEvent>,
) -> RepoState {
    RepoState {
        schema: STATE_SCHEMA,
        repo: meta.full_name.clone(),
        scanned_at: ctx.now,
        stars_total: meta.stargazers_count,
        logins,
        events,
        source,
        star_days: (source == Source::History).then_some(days),
    }
}

fn unique_logins(stars: &[Star]) -> Vec<String> {
    stars
        .iter()
        .map(|star| star.login.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn cross_users(ctx: &Ctx, name: &str, logins: &[String]) -> Result<Vec<CrossUser>, Error> {
    let others: Vec<(String, HashSet<String>)> = store::list_states(&ctx.layout)?
        .into_iter()
        .map(|state| (state.repo, state.logins.into_iter().collect()))
        .collect();
    let current: HashSet<String> = logins.iter().cloned().collect();
    Ok(diff::cross_users(name, &current, &others))
}

fn finish(ctx: &Ctx, started: Instant, out: &mut dyn Write) -> Result<(), Error> {
    let elapsed = started.elapsed().as_secs_f64();
    output(writeln!(
        out,
        "Finish {elapsed:.1} sec, in {}.",
        ctx.zone.format(ctx.now, "%Hh:%Mm")
    ))?;
    let token = if ctx.client.has_token() {
        "GitHub-token is used!"
    } else {
        "GitHub token not provided."
    };
    output(writeln!(out, "{token}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_multiple_of_100_needs_one_extra_request() {
        assert_eq!(requests_for(5800), 59);
        assert_eq!(requests_for(5900), 60);
    }
}
