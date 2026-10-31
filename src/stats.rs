use crate::model::{Gap, Last30, Marketing, Quartiles, Share, Star, Stats, Zone};
use chrono::{Datelike, NaiveDate, Weekday};
use std::cmp::Reverse;
use std::collections::BTreeMap;

const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

const WEEKDAYS: [Weekday; 7] = [
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
    Weekday::Sat,
    Weekday::Sun,
];

pub fn compute(
    stars: &[Star],
    created: NaiveDate,
    today: NaiveDate,
    stargazers_count: u64,
    zone: &Zone,
) -> Stats {
    let mut per_day: BTreeMap<NaiveDate, u32> = BTreeMap::new();
    for star in stars {
        *per_day.entry(star.starred_at.date_naive()).or_insert(0) += 1;
    }
    let days: Vec<(NaiveDate, u32)> = per_day.into_iter().collect();

    let mut by_hour = [0u64; 24];
    for star in stars {
        by_hour[zone.hour_of(star.starred_at) as usize] += 1;
    }

    let mut stats = from_days(&days, created, today);
    stats.by_hour = Some(by_hour);
    stats.fetched_stars = stars.len() as u64;
    stats.private_stars = stargazers_count.abs_diff(stats.fetched_stars);
    stats
}

pub fn from_days(days: &[(NaiveDate, u32)], created: NaiveDate, today: NaiveDate) -> Stats {
    let age_days = (today - created).num_days();

    let per_day: BTreeMap<NaiveDate, u32> = days
        .iter()
        .filter(|&&(_, count)| count > 0)
        .copied()
        .collect();
    let fetched_stars: u64 = per_day.values().map(|&count| u64::from(count)).sum();
    let start = per_day
        .keys()
        .next()
        .map_or(created, |&first| first.min(created));
    let end = per_day
        .keys()
        .next_back()
        .map_or(today, |&last| last.max(today));

    let daily: Vec<(NaiveDate, u32)> = start
        .iter_days()
        .take_while(|&day| day <= end)
        .map(|day| (day, per_day.get(&day).copied().unwrap_or(0)))
        .collect();
    let daily_counts: Vec<f64> = daily.iter().map(|&(_, count)| f64::from(count)).collect();

    let mut running = 0u64;
    let cumulative: Vec<(NaiveDate, u64)> = daily
        .iter()
        .map(|&(day, count)| {
            running += u64::from(count);
            (day, running)
        })
        .collect();

    let peak = daily
        .iter()
        .fold(
            None,
            |best: Option<(NaiveDate, u32)>, &(day, count)| match best {
                Some((_, top)) if top >= count => best,
                _ => Some((day, count)),
            },
        )
        .filter(|&(_, count)| count > 0);

    let mut by_weekday = [0u64; 7];
    let mut weekday_present = [false; 7];
    let mut month_sums = BTreeMap::new();
    let mut year_sums = BTreeMap::new();
    for &(day, count) in &daily {
        let count = u64::from(count);
        let weekday = day.weekday().num_days_from_monday() as usize;
        by_weekday[weekday] += count;
        weekday_present[weekday] = true;
        *month_sums.entry(day.month()).or_insert(0) += count;
        *year_sums.entry(day.year()).or_insert(0) += count;
    }

    let by_month: Vec<(u32, u64)> = month_sums.into_iter().collect();
    let by_year: Vec<(i32, u64)> = year_sums.into_iter().collect();

    let weekday_items: Vec<(Weekday, u64)> = (0..7_usize)
        .filter(|&index| weekday_present[index])
        .map(|index| (WEEKDAYS[index], by_weekday[index]))
        .collect();
    let (best_weekday, worst_weekday) = best_and_worst(&weekday_items);
    let (best_month, worst_month) = best_and_worst(&by_month);

    let month_entries: Vec<(String, u64)> = by_month
        .iter()
        .map(|&(month, sum)| (MONTH_NAMES[(month - 1) as usize].to_string(), sum))
        .collect();
    let year_entries: Vec<(String, u64)> = by_year
        .iter()
        .map(|&(year, sum)| (year.to_string(), sum))
        .collect();
    let month_quartiles = if age_days > 32 {
        quartiles(&month_entries)
    } else {
        None
    };
    let year_quartiles = if age_days > 365 {
        quartiles(&year_entries)
    } else {
        None
    };

    let stdev_value = if age_days > 62 {
        stdev(&daily_counts)
    } else {
        None
    };
    let marketing_level = match stdev_value {
        Some(value) => marketing(value),
        None => Marketing::NotEnoughData,
    };

    let mean_per_day = mean(&daily_counts).unwrap_or(0.0);
    let longest_gap_value = longest_gap(&daily);

    Stats {
        age_days,
        daily,
        cumulative,
        peak,
        trend_per_day: trend(&daily_counts, age_days),
        mean_per_day,
        mean_per_year: if age_days > 365 {
            Some(mean_per_day * 365.0)
        } else {
            None
        },
        last30: if age_days > 62 {
            last30(&daily_counts)
        } else {
            None
        },
        stdev: stdev_value,
        marketing: marketing_level,
        by_weekday,
        by_month,
        by_year,
        by_hour: None,
        best_weekday,
        worst_weekday,
        best_month,
        worst_month,
        month_quartiles,
        year_quartiles,
        fetched_stars,
        private_stars: 0,
        longest_gap: longest_gap_value,
    }
}

pub fn median(v: &[f64]) -> Option<f64> {
    let n = v.len();
    if n == 0 {
        return None;
    }
    let mut sorted = v.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = n / 2;
    if n.is_multiple_of(2) {
        Some((sorted[middle - 1] + sorted[middle]) / 2.0)
    } else {
        Some(sorted[middle])
    }
}

pub fn mean(v: &[f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    Some(v.iter().sum::<f64>() / v.len() as f64)
}

pub fn stdev(v: &[f64]) -> Option<f64> {
    if v.len() < 2 {
        return None;
    }
    let average = mean(v)?;
    let squares: f64 = v.iter().map(|x| (x - average).powi(2)).sum();
    Some((squares / (v.len() - 1) as f64).sqrt())
}

pub fn marketing(stdev: f64) -> Marketing {
    if stdev < 4.0 {
        Marketing::None
    } else if stdev < 6.0 {
        Marketing::Low
    } else if stdev < 11.0 {
        Marketing::Medium
    } else if stdev < 17.0 {
        Marketing::High
    } else if stdev < 35.0 {
        Marketing::Hard
    } else {
        Marketing::HardPlus
    }
}

pub fn quartiles(entries: &[(String, u64)]) -> Option<Quartiles> {
    let count = entries.len();
    if count < 2 {
        return None;
    }
    let mut values: Vec<f64> = entries.iter().map(|(_, stars)| *stars as f64).collect();
    values.sort_by(f64::total_cmp);
    let q1 = median(&values[..count / 2])?;
    let q3 = median(&values[count.div_ceil(2)..])?;
    let top_count = values.iter().filter(|&&x| x > q3 && x != 0.0).count();
    let middle_count = values
        .iter()
        .filter(|&&x| (q1..=q3).contains(&x) && x != 0.0)
        .count();
    let total: u64 = entries.iter().map(|(_, stars)| stars).sum();

    let mut ordered: Vec<&(String, u64)> = entries.iter().collect();
    ordered.sort_by_key(|entry| Reverse(entry.1));
    let middle_end = top_count + middle_count;

    Some(Quartiles {
        top: ordered[..top_count]
            .iter()
            .map(|entry| share(entry, total))
            .collect(),
        middle: ordered[top_count..middle_end]
            .iter()
            .map(|entry| share(entry, total))
            .collect(),
        low: ordered[middle_end..]
            .iter()
            .map(|entry| share(entry, total))
            .collect(),
    })
}

fn share(entry: &(String, u64), total: u64) -> Share {
    let (label, stars) = entry;
    Share {
        label: label.clone(),
        stars: *stars,
        percent: if total == 0 {
            0.0
        } else {
            *stars as f64 * 100.0 / total as f64
        },
    }
}

fn trend(daily_counts: &[f64], age_days: i64) -> u64 {
    let window = if age_days > 31 {
        &daily_counts[daily_counts.len().saturating_sub(30)..]
    } else {
        daily_counts
    };
    let middle = median(window).unwrap_or(0.0);
    if middle == 0.5 {
        1
    } else {
        middle.round_ties_even() as u64
    }
}

fn last30(daily_counts: &[f64]) -> Option<Last30> {
    let len = daily_counts.len();
    let before = &daily_counts[len.saturating_sub(60)..len.saturating_sub(30)];
    let after = &daily_counts[len.saturating_sub(30)..];
    let median_before = median(before)?;
    let median_after = median(after)?;
    let mean_before = mean(before)?;
    let mean_after = mean(after)?;

    let median_change_percent = if median_before == 0.0 {
        None
    } else {
        Some((median_after - median_before) / median_before * 100.0)
    };
    let mean_change_times = if mean_before == 0.0 {
        None
    } else {
        let ratio = mean_after / mean_before;
        if ratio >= 1.0 {
            Some(ratio)
        } else if mean_after == 0.0 {
            None
        } else {
            Some(-(mean_before / mean_after))
        }
    };

    Some(Last30 {
        median_change_percent,
        mean_change_times,
        sum_change: (after.iter().sum::<f64>() - before.iter().sum::<f64>()) as i64,
    })
}

fn longest_gap(daily: &[(NaiveDate, u32)]) -> Option<Gap> {
    let mut best: Option<Gap> = None;
    let mut current: Option<Gap> = None;
    for &(day, count) in daily {
        if count != 0 {
            current = None;
            continue;
        }
        let run = current.get_or_insert(Gap {
            start: day,
            days: 0,
        });
        run.days += 1;
        if best.as_ref().is_none_or(|top| run.days > top.days) {
            best = Some(run.clone());
        }
    }
    best
}

fn best_and_worst<K: Copy>(items: &[(K, u64)]) -> (Option<K>, Option<K>) {
    if items.iter().all(|&(_, sum)| sum == 0) {
        return (None, None);
    }
    let mut best = items[0];
    let mut worst = items[0];
    for &item in items {
        if item.1 > best.1 {
            best = item;
        }
        if item.1 < worst.1 {
            worst = item;
        }
    }
    (Some(best.0), Some(worst.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeDelta, TimeZone, Utc};

    fn day(offset: i64) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() + TimeDelta::days(offset)
    }

    fn daily(counts: &[u32]) -> Vec<(NaiveDate, u32)> {
        counts
            .iter()
            .enumerate()
            .map(|(i, &count)| (day(i as i64), count))
            .collect()
    }

    fn labels(tier: &[Share]) -> Vec<&str> {
        tier.iter().map(|share| share.label.as_str()).collect()
    }

    #[test]
    fn median_takes_middle_or_mean_of_two_middle() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), Some(2.5));
    }

    #[test]
    fn stdev_is_sample_with_n_minus_one() {
        let value = stdev(&[2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]).unwrap();
        assert!((value - (32.0_f64 / 7.0).sqrt()).abs() < 1e-12);
    }

    #[test]
    fn marketing_thresholds_are_half_open() {
        for (value, expected) in [
            (3.99, Marketing::None),
            (4.0, Marketing::Low),
            (6.0, Marketing::Medium),
            (11.0, Marketing::High),
            (17.0, Marketing::Hard),
            (35.0, Marketing::HardPlus),
        ] {
            assert_eq!(marketing(value), expected, "stdev {value}");
        }
    }

    #[test]
    fn quartiles_split_fixed_twelve_months() {
        let stars: [u64; 12] = [5, 1, 9, 3, 7, 11, 2, 8, 0, 4, 10, 6];
        let entries: Vec<(String, u64)> = MONTH_NAMES
            .iter()
            .zip(stars)
            .map(|(name, stars)| (name.to_string(), stars))
            .collect();
        let quartiles = quartiles(&entries).unwrap();
        assert_eq!(labels(&quartiles.top), ["Jun", "Nov", "Mar"]);
        assert_eq!(
            labels(&quartiles.middle),
            ["Aug", "May", "Dec", "Jan", "Oct", "Apr"]
        );
        assert_eq!(labels(&quartiles.low), ["Jul", "Feb", "Sep"]);
        assert!((quartiles.top[0].percent - 100.0 * 11.0 / 66.0).abs() < 1e-9);
    }

    #[test]
    fn longest_gap_in_the_middle() {
        assert_eq!(
            longest_gap(&daily(&[1, 0, 0, 1, 0, 1])),
            Some(Gap {
                start: day(1),
                days: 2
            })
        );
    }

    #[test]
    fn longest_gap_counts_trailing_run() {
        assert_eq!(
            longest_gap(&daily(&[1, 0, 1, 0, 0, 0])),
            Some(Gap {
                start: day(3),
                days: 3
            })
        );
    }

    #[test]
    fn trend_half_gives_one_and_two_and_half_gives_two() {
        assert_eq!(trend(&[0.0, 0.0, 1.0, 1.0], 3), 1);
        assert_eq!(trend(&[2.0, 3.0], 1), 2);
    }

    #[test]
    fn last30_negative_ratio_is_inverted() {
        let counts: Vec<f64> = [vec![0.0; 4], vec![2.0; 30], vec![1.0; 30]].concat();
        let last = last30(&counts).unwrap();
        assert_eq!(last.mean_change_times, Some(-2.0));
        assert_eq!(last.median_change_percent, Some(-50.0));
        assert_eq!(last.sum_change, -30);
    }

    #[test]
    fn zero_star_repo_older_than_63_days_does_not_panic() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let created = today - TimeDelta::days(100);
        let stats = compute(&[], created, today, 0, &Zone::Utc);
        assert_eq!(stats.peak, None);
        assert_eq!(stats.best_weekday, None);
        assert_eq!(stats.mean_per_day, 0.0);
        assert!(stats.longest_gap.is_some());
    }

    #[test]
    fn hours_are_bucketed_in_utc() {
        let stars = [
            Star {
                login: "a".into(),
                starred_at: Utc.with_ymd_and_hms(2026, 3, 15, 13, 45, 0).unwrap(),
            },
            Star {
                login: "b".into(),
                starred_at: Utc.with_ymd_and_hms(2026, 3, 16, 0, 10, 0).unwrap(),
            },
        ];
        let created = NaiveDate::from_ymd_opt(2026, 3, 14).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 3, 16).unwrap();
        let stats = compute(&stars, created, today, 2, &Zone::Utc);
        let by_hour = stats.by_hour.unwrap();
        assert_eq!(by_hour[13], 1);
        assert_eq!(by_hour[0], 1);
        assert_eq!(by_hour.iter().sum::<u64>(), 2);
    }

    #[test]
    fn from_days_matches_compute_without_hours() {
        let on = |d: u32| NaiveDate::from_ymd_opt(2026, 3, d).unwrap();
        let days = [(on(15), 2), (on(16), 3), (on(18), 1)];
        let stars: Vec<Star> = days
            .iter()
            .flat_map(|&(day, count)| {
                (0..count).map(move |hour| Star {
                    login: format!("{day}-{hour}"),
                    starred_at: Utc.from_utc_datetime(&day.and_hms_opt(hour, 0, 0).unwrap()),
                })
            })
            .collect();
        let created = on(14);
        let today = on(20);
        let from = from_days(&days, created, today);
        let full = compute(&stars, created, today, 6, &Zone::Utc);
        assert_eq!(from.daily, full.daily);
        assert_eq!(from.peak, full.peak);
        assert_eq!(from.trend_per_day, full.trend_per_day);
        assert_eq!(from.by_hour, None);
        assert_eq!(from.fetched_stars, 6);
    }
}
