use crate::model::{CrossUser, Diff, HistoryTotals, ScanEvent, Side, StarCounts, Zone};
use chrono::NaiveDate;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub fn diff(old: &[String], new: &[String]) -> Diff {
    let old_set: BTreeSet<&str> = old.iter().map(String::as_str).collect();
    let new_set: BTreeSet<&str> = new.iter().map(String::as_str).collect();
    Diff {
        added: new_set
            .difference(&old_set)
            .map(|s| s.to_string())
            .collect(),
        removed: old_set
            .difference(&new_set)
            .map(|s| s.to_string())
            .collect(),
    }
}

pub fn cross_users(
    current: &str,
    current_logins: &HashSet<String>,
    others: &[(String, HashSet<String>)],
) -> Vec<CrossUser> {
    let mut found: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (repo, logins) in others {
        if repo.eq_ignore_ascii_case(current) {
            continue;
        }
        for login in logins {
            if current_logins.contains(login) {
                found
                    .entry(login.as_str())
                    .or_default()
                    .insert(repo.as_str());
            }
        }
    }
    let mut result: Vec<CrossUser> = found
        .into_iter()
        .map(|(login, other_repos)| {
            let mut repos = vec![current.to_string()];
            repos.extend(other_repos.into_iter().map(str::to_string));
            repos.sort();
            CrossUser {
                login: login.to_string(),
                repos,
            }
        })
        .collect();
    result.sort_by(|a, b| {
        b.repos
            .len()
            .cmp(&a.repos.len())
            .then_with(|| a.login.cmp(&b.login))
    });
    result
}

pub fn gone_per_day(events: &[ScanEvent], zone: &Zone) -> Vec<(NaiveDate, u32)> {
    let mut per_day: BTreeMap<NaiveDate, u32> = BTreeMap::new();
    for event in events.iter().filter(|e| e.removed_total() > 0) {
        *per_day.entry(zone.date_of(event.scanned_at)).or_default() += event.removed_total() as u32;
    }
    per_day.into_iter().collect()
}

pub fn day_changes(
    prev: &[(NaiveDate, u32)],
    now: &[(NaiveDate, u32)],
) -> (StarCounts, Vec<(NaiveDate, u32)>) {
    let prev_days: BTreeMap<NaiveDate, u32> = prev.iter().copied().collect();
    let now_days: BTreeMap<NaiveDate, u32> = now.iter().copied().collect();
    let all_days: BTreeSet<NaiveDate> = prev_days.keys().chain(now_days.keys()).copied().collect();

    let mut counts = StarCounts {
        added: 0,
        removed: 0,
    };
    let mut gains = Vec::new();
    for day in all_days {
        let before = prev_days.get(&day).copied().unwrap_or(0);
        let after = now_days.get(&day).copied().unwrap_or(0);
        let gain = after.saturating_sub(before);
        counts.added += u64::from(gain);
        counts.removed += u64::from(before.saturating_sub(after));
        if gain > 0 {
            gains.push((day, gain));
        }
    }
    (counts, gains)
}

pub fn duplicates(events: &[ScanEvent], side: Side) -> Vec<(String, u32)> {
    let mut counts: HashMap<&str, u32> = HashMap::new();
    for event in events {
        for login in side_logins(event, side) {
            *counts.entry(login.as_str()).or_default() += 1;
        }
    }
    let mut result: Vec<(String, u32)> = counts
        .into_iter()
        .filter(|&(_, count)| count > 1)
        .map(|(login, count)| (login.to_string(), count))
        .collect();
    result.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    result
}

pub fn history_totals(
    events: &[ScanEvent],
    side: Side,
    stars_now: u64,
    today: NaiveDate,
    zone: &Zone,
) -> HistoryTotals {
    let total: u64 = events.iter().map(|e| side_total(e, side)).sum();
    let percent = if stars_now == 0 {
        0.0
    } else {
        (total as f64) * 100.0 / (stars_now as f64)
    };
    let days_since_first = events
        .first()
        .map_or(0, |e| (today - zone.date_of(e.prev_scanned_at)).num_days());
    HistoryTotals {
        total,
        percent,
        days_since_first,
    }
}

fn side_logins(event: &ScanEvent, side: Side) -> &[String] {
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, TimeZone, Utc};

    fn day(d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, d, 12, 0, 0).unwrap()
    }

    fn date(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    fn event(d: u32, added: &[&str], removed: &[&str]) -> ScanEvent {
        ScanEvent {
            scanned_at: day(d),
            prev_scanned_at: day(d - 1),
            stars_total: 0,
            added: strings(added),
            removed: strings(removed),
            counts: None,
        }
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn cross(login: &str, repos: &[&str]) -> CrossUser {
        CrossUser {
            login: login.to_string(),
            repos: strings(repos),
        }
    }

    #[test]
    fn diff_lists_both_directions_sorted() {
        let old = strings(&["z", "a", "m", "k"]);
        let new = strings(&["q", "b", "m", "k"]);
        assert_eq!(
            diff(&old, &new),
            Diff {
                added: strings(&["b", "q"]),
                removed: strings(&["a", "z"]),
            }
        );
    }

    #[test]
    fn cross_users_skips_current_and_sorts_by_repo_count() {
        let logins = set(&["ann", "bob", "cid", "dan"]);
        let others = vec![
            ("cur".to_string(), set(&["ann", "bob", "cid", "dan"])),
            ("r1".to_string(), set(&["ann", "bob", "dan"])),
            ("r2".to_string(), set(&["bob"])),
            ("r3".to_string(), set(&["ann", "bob"])),
        ];
        assert_eq!(
            cross_users("cur", &logins, &others),
            vec![
                cross("bob", &["cur", "r1", "r2", "r3"]),
                cross("ann", &["cur", "r1", "r3"]),
                cross("dan", &["cur", "r1"]),
            ]
        );
    }

    #[test]
    fn cross_users_compares_current_repository_ignoring_ascii_case() {
        let logins = set(&["ann"]);
        let others = vec![("Cur".to_string(), set(&["ann"]))];
        assert!(cross_users("cur", &logins, &others).is_empty());
    }

    #[test]
    fn gone_per_day_sums_removed_by_day() {
        let events = vec![
            event(10, &[], &["a", "b"]),
            event(10, &[], &["c"]),
            event(11, &["x"], &[]),
            event(12, &[], &["d"]),
        ];
        assert_eq!(
            gone_per_day(&events, &Zone::Utc),
            vec![(date(10), 3), (date(12), 1)]
        );
    }

    #[test]
    fn duplicates_count_repeats_per_side() {
        let events = vec![
            event(10, &["a", "b"], &["z"]),
            event(11, &["a", "c", "d"], &["z"]),
            event(12, &["b", "a", "d"], &["z", "z"]),
        ];
        assert_eq!(
            duplicates(&events, Side::Added),
            vec![
                ("a".to_string(), 3),
                ("b".to_string(), 2),
                ("d".to_string(), 2),
            ]
        );
        assert_eq!(
            duplicates(&events, Side::Removed),
            vec![("z".to_string(), 4)]
        );
    }

    #[test]
    fn history_totals_counts_side_and_days_since_first() {
        let events = vec![event(10, &["a", "b"], &["c"]), event(11, &["d"], &[])];
        let today = date(20);
        assert_eq!(
            history_totals(&events, Side::Added, 4, today, &Zone::Utc),
            HistoryTotals {
                total: 3,
                percent: 75.0,
                days_since_first: 11,
            }
        );
        assert_eq!(
            history_totals(&events, Side::Removed, 4, today, &Zone::Utc),
            HistoryTotals {
                total: 1,
                percent: 25.0,
                days_since_first: 11,
            }
        );
        assert_eq!(
            history_totals(&[], Side::Added, 0, today, &Zone::Utc),
            HistoryTotals {
                total: 0,
                percent: 0.0,
                days_since_first: 0,
            }
        );
    }

    #[test]
    fn day_changes_counts_growth_new_day_and_drop() {
        let prev = [(date(10), 2), (date(11), 3)];
        let now = [(date(10), 5), (date(11), 1), (date(12), 4)];
        assert_eq!(
            day_changes(&prev, &now),
            (
                StarCounts {
                    added: 7,
                    removed: 2,
                },
                vec![(date(10), 3), (date(12), 4)],
            )
        );
    }

    #[test]
    fn counted_event_without_names_counts_in_gone_and_totals() {
        let counted = ScanEvent {
            counts: Some(StarCounts {
                added: 2,
                removed: 3,
            }),
            ..event(10, &[], &[])
        };
        let events = vec![counted];
        assert_eq!(gone_per_day(&events, &Zone::Utc), vec![(date(10), 3)]);
        assert_eq!(
            history_totals(&events, Side::Removed, 0, date(20), &Zone::Utc).total,
            3
        );
        assert_eq!(
            history_totals(&events, Side::Added, 0, date(20), &Zone::Utc).total,
            2
        );
    }
}
