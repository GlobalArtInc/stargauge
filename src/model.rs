use chrono::{DateTime, Local, NaiveDate, Timelike, Utc, Weekday};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoId {
    pub owner: String,
    pub name: String,
}

impl RepoId {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoMeta {
    pub full_name: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub size_kb: Option<u64>,
    pub stargazers_count: u64,
    pub created_at: DateTime<Utc>,
    pub pushed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Star {
    pub login: String,
    pub starred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateInfo {
    pub limit: u32,
    pub remaining: u32,
    pub reset_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    List,
    History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StarCounts {
    pub added: u64,
    pub removed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanEvent {
    pub scanned_at: DateTime<Utc>,
    pub prev_scanned_at: DateTime<Utc>,
    pub stars_total: u64,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counts: Option<StarCounts>,
}

impl ScanEvent {
    pub fn added_total(&self) -> u64 {
        self.counts.map_or(self.added.len() as u64, |c| c.added)
    }

    pub fn removed_total(&self) -> u64 {
        self.counts.map_or(self.removed.len() as u64, |c| c.removed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoState {
    pub schema: u32,
    pub repo: String,
    pub scanned_at: DateTime<Utc>,
    pub stars_total: u64,
    pub logins: Vec<String>,
    pub events: Vec<ScanEvent>,
    #[serde(default)]
    pub source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub star_days: Option<Vec<(NaiveDate, u32)>>,
}

pub const STATE_SCHEMA: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Local,
    Utc,
}

impl Zone {
    pub fn hour_of(&self, t: DateTime<Utc>) -> u32 {
        match self {
            Zone::Local => t.with_timezone(&Local).hour(),
            Zone::Utc => t.hour(),
        }
    }

    pub fn date_of(&self, t: DateTime<Utc>) -> NaiveDate {
        match self {
            Zone::Local => t.with_timezone(&Local).date_naive(),
            Zone::Utc => t.date_naive(),
        }
    }

    pub fn format(&self, t: DateTime<Utc>, fmt: &str) -> String {
        match self {
            Zone::Local => t.with_timezone(&Local).format(fmt).to_string(),
            Zone::Utc => t.format(fmt).to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marketing {
    NotEnoughData,
    None,
    Low,
    Medium,
    High,
    Hard,
    HardPlus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Share {
    pub label: String,
    pub stars: u64,
    pub percent: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Quartiles {
    pub top: Vec<Share>,
    pub middle: Vec<Share>,
    pub low: Vec<Share>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Last30 {
    pub median_change_percent: Option<f64>,
    pub mean_change_times: Option<f64>,
    pub sum_change: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    pub start: NaiveDate,
    pub days: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    pub age_days: i64,
    pub daily: Vec<(NaiveDate, u32)>,
    pub cumulative: Vec<(NaiveDate, u64)>,
    pub peak: Option<(NaiveDate, u32)>,
    pub trend_per_day: u64,
    pub mean_per_day: f64,
    pub mean_per_year: Option<f64>,
    pub last30: Option<Last30>,
    pub stdev: Option<f64>,
    pub marketing: Marketing,
    pub by_weekday: [u64; 7],
    pub by_month: Vec<(u32, u64)>,
    pub by_year: Vec<(i32, u64)>,
    pub by_hour: Option<[u64; 24]>,
    pub best_weekday: Option<Weekday>,
    pub worst_weekday: Option<Weekday>,
    pub best_month: Option<u32>,
    pub worst_month: Option<u32>,
    pub month_quartiles: Option<Quartiles>,
    pub year_quartiles: Option<Quartiles>,
    pub fetched_stars: u64,
    pub private_stars: u64,
    pub longest_gap: Option<Gap>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossUser {
    pub login: String,
    pub repos: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryTotals {
    pub total: u64,
    pub percent: f64,
    pub days_since_first: i64,
}

pub fn format_interval(d: chrono::Duration) -> String {
    let days = d.num_days();
    let hours = d.num_hours() % 24;
    let minutes = d.num_minutes() % 60;
    format!("{days}d. {hours:02}h. {minutes:02}m.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn format_interval_pads_hours_and_minutes() {
        assert_eq!(
            format_interval(chrono::Duration::minutes(1565)),
            "1d. 02h. 05m."
        );
    }

    #[test]
    fn utc_zone_takes_hour_as_is() {
        let t = Utc.with_ymd_and_hms(2026, 3, 15, 13, 45, 0).unwrap();
        assert_eq!(Zone::Utc.hour_of(t), 13);
    }
}
