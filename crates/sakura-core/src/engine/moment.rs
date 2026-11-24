//! A time as the engine writes it, read as seconds since the epoch.

use crate::graph::parse_time;

const SECONDS_PER_MINUTE: i64 = 60;
const SECONDS_PER_HOUR: i64 = 3600;
/// The most a zone offset can be: a day.
const MAX_OFFSET_HOURS: i64 = 24;

/// Seconds since the epoch of `time`, written as ISO 8601 with a `Z` or a `+HH:MM` offset, with or without
/// fractions of a second. A text that is no such time is none.
pub fn iso_seconds(time: &str) -> Option<i64> {
    let (clock_start, _) = time.split_once('T')?;
    let split_at = time[clock_start.len()..].find(['Z', '+', '-'])? + clock_start.len();
    let (moment, zone) = time.split_at(split_at);
    let whole = moment.split_once('.').map_or(moment, |(whole, _)| whole);
    let utc = parse_time(&format!("{whole}Z"))?;
    Some(utc - offset_seconds(zone)?)
}

/// The offset a zone suffix names: `Z` is none, `+01:00` is an hour ahead of UTC.
fn offset_seconds(zone: &str) -> Option<i64> {
    if zone == "Z" {
        return Some(0);
    }
    let sign = if zone.starts_with('-') { -1 } else { 1 };
    let (hours, minutes) = zone.get(1..)?.split_once(':')?;
    let (hours, minutes): (i64, i64) = (hours.parse().ok()?, minutes.parse().ok()?);
    let is_valid = (0..MAX_OFFSET_HOURS).contains(&hours) && (0..60).contains(&minutes);
    is_valid.then_some(sign * (hours * SECONDS_PER_HOUR + minutes * SECONDS_PER_MINUTE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_with_z_is_read_as_utc() {
        assert_eq!(
            iso_seconds("2026-11-18T07:00:00Z"),
            parse_time("2026-11-18T07:00:00Z")
        );
    }

    #[test]
    fn a_time_with_an_offset_is_moved_to_utc() {
        let utc = parse_time("2026-11-18T06:00:00Z");
        assert_eq!(iso_seconds("2026-11-18T07:00:00+01:00"), utc);
        assert_eq!(iso_seconds("2026-11-18T01:00:00-05:00"), utc);
    }

    #[test]
    fn fractions_of_a_second_are_dropped() {
        assert_eq!(
            iso_seconds("2026-11-18T07:00:00.250Z"),
            parse_time("2026-11-18T07:00:00Z")
        );
    }

    #[test]
    fn text_that_is_no_time_is_none() {
        for text in [
            "",
            "tomorrow",
            "2026-11-18",
            "2026-11-18T07:00:00",
            "2026-13-18T07:00:00Z",
        ] {
            assert_eq!(iso_seconds(text), None, "{text:?}");
        }
    }
}
