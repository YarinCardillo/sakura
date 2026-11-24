//! When an account's week resets, in the person's own time: "Mon 09:00".

use crate::l10n::{tr, tr_with, Key};
use gpui::SharedString;
use sakura_core::engine::moment::iso_seconds;

const SECONDS_PER_DAY: i64 = 86_400;
const SECONDS_PER_HOUR: i64 = 3_600;
const SECONDS_PER_MINUTE: i64 = 60;
/// The epoch's day, 1 January 1970, was a Thursday; days are counted from Sunday as 0.
const EPOCH_WEEKDAY: i64 = 4;
const DAYS_PER_WEEK: i64 = 7;

/// The short names of the days, Sunday first.
const DAYS: [Key; 7] = [
    Key("engine_page.day.sun"),
    Key("engine_page.day.mon"),
    Key("engine_page.day.tue"),
    Key("engine_page.day.wed"),
    Key("engine_page.day.thu"),
    Key("engine_page.day.fri"),
    Key("engine_page.day.sat"),
];

/// The weekday (Sunday is 0), the hour and the minute of the moment `epoch` in a zone `offset` seconds
/// ahead of UTC.
pub fn weekday_and_clock(epoch: i64, offset: i64) -> (usize, i64, i64) {
    let local = epoch + offset;
    let days = local.div_euclid(SECONDS_PER_DAY);
    let weekday = (days + EPOCH_WEEKDAY).rem_euclid(DAYS_PER_WEEK) as usize;
    let seconds = local.rem_euclid(SECONDS_PER_DAY);
    (
        weekday,
        seconds / SECONDS_PER_HOUR,
        seconds % SECONDS_PER_HOUR / SECONDS_PER_MINUTE,
    )
}

/// When `epoch` was, as the person's clock says it, for a moment `now` and a zone `offset` seconds ahead of
/// UTC: the time of day for today ("21:40"), the day and time for the week before ("Tue 21:40"), the date
/// for anything older.
pub fn clock_words_in(epoch: i64, now: i64, offset: i64) -> String {
    let day = |moment: i64| (moment + offset).div_euclid(SECONDS_PER_DAY);
    let (weekday, hour, minute) = weekday_and_clock(epoch, offset);
    match day(now) - day(epoch) {
        0 => format!("{hour:02}:{minute:02}"),
        1..=6 => format!("{} {hour:02}:{minute:02}", tr(DAYS[weekday])),
        _ => iso_date(day(epoch)),
    }
}

/// The date of the day number `days` since the epoch, as `2026-11-18`.
fn iso_date(days: i64) -> String {
    // Civil-from-days (Howard Hinnant): the proleptic Gregorian calendar.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// How far this Mac's clock is ahead of UTC at the moment `epoch`, in seconds.
pub fn local_offset(epoch: i64) -> i64 {
    use chrono::{Local, TimeZone};

    Local
        .timestamp_opt(epoch, 0)
        .single()
        .map_or(0, |time| i64::from(time.offset().local_minus_utc()))
}

/// "Resets Mon 09:00" for the moment `reset` (the engine's ISO text), in this Mac's zone; nothing when the
/// text is no time.
pub fn resets(reset: &str) -> Option<SharedString> {
    let epoch = iso_seconds(reset)?;
    Some(resets_in(epoch, local_offset(epoch)))
}

/// "Updated 07:55" for the moment `as_of` (the engine's ISO text) of the newest usage reading, by this Mac's clock
/// and zone; nothing when the text is no time.
pub fn updated(as_of: &str) -> Option<SharedString> {
    let epoch = iso_seconds(as_of)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    Some(updated_in(epoch, now, local_offset(epoch)))
}

/// The same for a moment, a "now" and a zone given: what [`updated`] says, testable.
pub fn updated_in(epoch: i64, now: i64, offset: i64) -> SharedString {
    let when = clock_words_in(epoch, now, offset);
    tr_with(Key("engine_page.updated"), &[("when", &when)])
}

/// The same for a moment and a zone given: what [`resets`] says, testable.
pub fn resets_in(epoch: i64, offset: i64) -> SharedString {
    let (weekday, hour, minute) = weekday_and_clock(epoch, offset);
    let when = format!("{} {hour:02}:{minute:02}", tr(DAYS[weekday]));
    tr_with(Key("engine_page.resets"), &[("when", &when)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use sakura_core::graph::parse_time;

    #[test]
    fn a_moment_is_said_by_the_clock_today_by_the_day_in_the_week_and_by_the_date_after() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let at = |text: &str| parse_time(text).expect("a time");
        let now = at("2026-11-18T12:00:00Z");
        let said = |text: &str, offset: i64| clock_words_in(at(text), now, offset);
        assert_eq!(said("2026-11-18T10:05:00Z", 0), "10:05");
        assert_eq!(said("2026-11-17T21:40:00Z", 0), "Tue 21:40");
        assert_eq!(said("2026-11-01T08:00:00Z", 0), "2026-11-01");
        assert_eq!(
            said("2026-11-18T10:05:00Z", 3_600),
            "11:05",
            "the person's zone"
        );
        assert_eq!(
            said("2026-11-17T23:30:00Z", 3_600),
            "00:30",
            "past midnight there: today"
        );
        assert_eq!(said("2024-02-29T08:00:00Z", 0), "2024-02-29", "a leap day");
    }

    #[test]
    fn a_monday_morning_in_utc_is_monday_at_nine() {
        let epoch = parse_time("2026-11-23T09:00:00Z").expect("a time");
        assert_eq!(weekday_and_clock(epoch, 0), (1, 9, 0));
    }

    #[test]
    fn a_zone_ahead_of_utc_moves_the_clock_and_can_move_the_day() {
        let epoch = parse_time("2026-11-22T23:30:00Z").expect("a time");
        assert_eq!(weekday_and_clock(epoch, 0), (0, 23, 30), "Sunday in UTC");
        assert_eq!(
            weekday_and_clock(epoch, SECONDS_PER_HOUR),
            (1, 0, 30),
            "Monday an hour ahead"
        );
        assert_eq!(weekday_and_clock(epoch, -SECONDS_PER_DAY / 2), (0, 11, 30));
    }

    #[test]
    fn the_usage_reading_says_when_it_was_updated_by_the_clock_of_its_day() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let at = |text: &str| parse_time(text).expect("a time");
        let now = at("2026-11-18T12:00:00Z");
        let said = |text: &str| updated_in(at(text), now, 0).to_string();
        assert_eq!(said("2026-11-18T07:55:00Z"), "Updated 07:55");
        assert_eq!(said("2026-11-17T21:40:00Z"), "Updated Tue 21:40");
        assert_eq!(updated("not a time"), None);
    }

    #[test]
    fn the_words_name_the_day_and_pad_the_clock() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let epoch = parse_time("2026-11-24T11:05:00Z").expect("a time");
        assert_eq!(resets_in(epoch, 0).as_ref(), "Resets Tue 11:05");
        assert_eq!(resets("not a time"), None);
    }
}
