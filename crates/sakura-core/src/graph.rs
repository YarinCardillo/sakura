//! The commits of a project to draw as a graph, read through `box graph`: the base branch and every
//! lavoro's branch as lanes. The shape is the one `box` documents in its `docs/interface.md`. Subjects
//! and branch names were written by agents: they are shown, never run.

mod layout;

pub use layout::{Layout, Row, Stroke};

use crate::box_cli::{BoxCli, Limit};
use crate::error::{BridgeError, BridgeResult};
use crate::said::Said;
use serde::Deserialize;

/// The longest name of a project or a lavoro `box graph` is asked about.
const NAME_MAX_LEN: usize = 64;
/// The most commits of the base branch `box graph` lists.
const LIMIT_MAX: u32 = 1000;
/// The most commits, lanes and parents a graph is drawn from: the base's thousand and two hundred a
/// task, with room. A reply over any of them is refused whole, before anything is laid out.
const COMMITS_MAX: usize = 10_000;
const LANES_MAX: usize = 1_000;
const PARENTS_MAX: usize = 64;
/// The most edges between commits a graph is drawn from.
const EDGES_MAX: usize = 40_000;
const SECONDS_PER_MINUTE: i64 = 60;
const SECONDS_PER_HOUR: i64 = 60 * SECONDS_PER_MINUTE;
const SECONDS_PER_DAY: i64 = 24 * SECONDS_PER_HOUR;
/// Up to this many hours an age is said in hours, then in days.
const HOURS_SAID: i64 = 48;
/// The years a commit's time may be in: from the epoch to the last year of four digits.
const YEARS: std::ops::RangeInclusive<i64> = 1970..=9999;

/// One lane: the base branch (the first, with no lavoro) or the branch of a lavoro.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Lane {
    pub lavoro: Option<String>,
    pub branch: String,
    /// Where the lavoro lives (`mac` or `box`); nothing for the base.
    #[serde(rename = "where", default)]
    pub place: Option<String>,
    /// The lane's last commit; nothing when the lavoro cannot be read.
    #[serde(default)]
    pub head: Option<String>,
    /// The commit the lavoro started from.
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub readable: bool,
}

/// One commit, listed once.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Commit {
    pub sha: String,
    #[serde(default)]
    pub parents: Vec<String>,
    pub subject: String,
    /// The author time, ISO 8601 in UTC.
    pub time: String,
    /// The lavoro the commit belongs to; nothing for a commit of the base branch.
    #[serde(default)]
    pub lavoro: Option<String>,
    /// The branches of the lanes whose head it is.
    #[serde(default)]
    pub branches: Vec<String>,
}

/// What `box graph` answers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Graph {
    pub project: String,
    /// The base branch.
    pub base: String,
    pub lanes: Vec<Lane>,
    /// Newest first.
    pub commits: Vec<Commit>,
}

impl Graph {
    /// The graph when it can be drawn: its commits, lanes and edges counted before any layout work is
    /// done for it, and its commits a history (each listed once, none its own ancestor).
    pub fn checked(self) -> BridgeResult<Self> {
        let parents = self.commits.iter().map(|commit| commit.parents.len());
        let is_within = self.commits.len() <= COMMITS_MAX
            && self.lanes.len() <= LANES_MAX
            && parents.clone().all(|count| count <= PARENTS_MAX)
            && parents.sum::<usize>() <= EDGES_MAX;
        match is_within && layout::is_history(&self) {
            true => Ok(self),
            false => Err(not_a_graph()),
        }
    }
}

/// What is said of commits that cannot be drawn.
pub(crate) fn not_a_graph() -> BridgeError {
    BridgeError::failed(Said::new("graph.invalid.commits"))
}

/// How long ago something happened, as coarse as a row says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Age {
    Now,
    Minutes(u64),
    Hours(u64),
    Days(u64),
}

impl Age {
    /// The age at `now` of what happened at `then`, both in seconds since the epoch. Whatever the two
    /// are, the difference is counted without overflow; what is yet to happen is now.
    pub fn between(then: i64, now: i64) -> Self {
        let seconds = now.saturating_sub(then).max(0);
        let whole = |unit: i64| u64::try_from(seconds / unit).unwrap_or_default();
        match seconds {
            _ if seconds < SECONDS_PER_MINUTE => Age::Now,
            _ if seconds < SECONDS_PER_HOUR => Age::Minutes(whole(SECONDS_PER_MINUTE)),
            _ if seconds < HOURS_SAID * SECONDS_PER_HOUR => Age::Hours(whole(SECONDS_PER_HOUR)),
            _ => Age::Days(whole(SECONDS_PER_DAY)),
        }
    }
}

/// Seconds since the epoch of `time`, written as `box` writes it: `2026-11-10T16:00:00Z`. Only a date
/// the calendar has, from the epoch to the year 9999, is a time: the numbers come from `box` and are
/// bounded here, so nothing computed from them can overflow.
pub fn parse_time(time: &str) -> Option<i64> {
    let (date, clock) = time.strip_suffix('Z')?.split_once('T')?;
    let mut date = date.splitn(3, '-').map(str::parse::<i64>);
    let mut clock = clock.splitn(3, ':').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let (hour, minute, second) = (
        clock.next()?.ok()?,
        clock.next()?.ok()?,
        clock.next()?.ok()?,
    );
    let is_valid = YEARS.contains(&year)
        && (1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&day)
        && (0..24).contains(&hour)
        && (0..60).contains(&minute)
        && (0..=60).contains(&second);
    is_valid.then(|| {
        let clock = hour * SECONDS_PER_HOUR + minute * SECONDS_PER_MINUTE + second;
        days_from_civil(year, month, day) * SECONDS_PER_DAY + clock
    })
}

/// How many days `month` of `year` has.
fn days_in_month(year: i64, month: i64) -> i64 {
    let is_leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    match month {
        2 if is_leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from the epoch to a date of the Gregorian calendar (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    const DAYS_PER_ERA: i64 = 146_097;
    const EPOCH_SHIFT: i64 = 719_468;
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * DAYS_PER_ERA + day_of_era - EPOCH_SHIFT
}

/// The name of a project or of a lavoro as `box graph` takes it: never read as a flag.
pub fn name(value: &str) -> BridgeResult<&str> {
    let is_word = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
    let starts_plain = value.starts_with(|c: char| c.is_ascii_alphanumeric());
    if starts_plain && value.len() <= NAME_MAX_LEN && value.chars().all(is_word) {
        Ok(value)
    } else {
        Err(BridgeError::invalid(
            Said::new("graph.invalid.name").text("value", value),
        ))
    }
}

/// `box graph <project or lavoro> --json [--limit <n>]`.
pub async fn read_graph(cli: &BoxCli, of: &str, limit: Option<u32>) -> BridgeResult<Graph> {
    let of = name(of)?;
    let limit = limit.map(|limit| limit.clamp(1, LIMIT_MAX).to_string());
    let mut args = vec!["graph", of, "--json"];
    if let Some(limit) = &limit {
        args.extend(["--limit", limit]);
    }
    let graph: Graph = cli.run_json(&args, Limit::Read).await?;
    graph.checked()
}

#[cfg(test)]
mod tests;
