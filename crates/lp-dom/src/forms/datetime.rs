//! Date and time microsyntaxes of `<input>` (HTML §2.3.5 and the
//! date/month/week/time/datetime-local states): parsing, the "convert a
//! string to a number" / "convert a number to a string" pairs, and Date
//! (milliseconds since the epoch, UTC) conversions.

pub(crate) const MS_PER_DAY: f64 = 86_400_000.0;
/// Chrome's (and ECMAScript's) last representable day: 275760-09-13.
const MAX_YEAR: i64 = 275_760;

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(y) => 29,
        _ => 28,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date `days` after 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// 0 = Monday … 6 = Sunday.
fn weekday(days: i64) -> i64 {
    (days + 3).rem_euclid(7)
}

/// Days since the epoch of the Monday of ISO week 1 of `year`.
fn week1_monday(year: i64) -> i64 {
    let jan4 = days_from_civil(year, 1, 4);
    jan4 - weekday(jan4)
}

fn weeks_in_year(year: i64) -> u32 {
    let jan1 = weekday(days_from_civil(year, 1, 1));
    // Thursday, or a leap year starting on Wednesday.
    if jan1 == 3 || (jan1 == 2 && is_leap(year)) {
        53
    } else {
        52
    }
}

/// Exactly `n` ASCII digits at the start of `s`.
fn digits(s: &str, n: usize) -> Option<(u32, &str)> {
    let b = s.as_bytes();
    if b.len() < n || !b[..n].iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some((s[..n].parse().ok()?, &s[n..]))
}

/// A year: four or more ASCII digits, greater than zero.
fn year(s: &str) -> Option<(i64, &str)> {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    if n < 4 {
        return None;
    }
    let y: i64 = s[..n].parse().ok()?;
    (y > 0 && y <= MAX_YEAR).then_some((y, &s[n..]))
}

fn month_component(s: &str) -> Option<(i64, u32, &str)> {
    let (y, rest) = year(s)?;
    let rest = rest.strip_prefix('-')?;
    let (m, rest) = digits(rest, 2)?;
    (1..=12).contains(&m).then_some((y, m, rest))
}

fn date_component(s: &str) -> Option<(i64, u32, u32, &str)> {
    let (y, m, rest) = month_component(s)?;
    let rest = rest.strip_prefix('-')?;
    let (d, rest) = digits(rest, 2)?;
    (d >= 1 && d <= days_in_month(y, m)).then_some((y, m, d, rest))
}

/// A time component: milliseconds since midnight.
fn time_component(s: &str) -> Option<(f64, &str)> {
    let (h, rest) = digits(s, 2)?;
    let rest = rest.strip_prefix(':')?;
    let (min, mut rest) = digits(rest, 2)?;
    if h > 23 || min > 59 {
        return None;
    }
    let mut ms = (h * 3_600_000 + min * 60_000) as f64;
    if let Some(r) = rest.strip_prefix(':') {
        let (sec, r) = digits(r, 2)?;
        if sec > 59 {
            return None;
        }
        ms += (sec * 1000) as f64;
        rest = r;
        if let Some(r) = rest.strip_prefix('.') {
            let n = r.bytes().take_while(u8::is_ascii_digit).count();
            if !(1..=3).contains(&n) {
                return None;
            }
            let frac: u32 = r[..n].parse().ok()?;
            ms += (frac * 10u32.pow(3 - n as u32)) as f64;
            rest = &r[n..];
        }
    }
    Some((ms, rest))
}

fn whole<T>(r: Option<(T, &str)>) -> Option<T> {
    r.filter(|(_, rest)| rest.is_empty()).map(|(v, _)| v)
}

/// Valid date string → ms since the epoch.
pub(crate) fn parse_date(s: &str) -> Option<f64> {
    let (y, m, d, rest) = date_component(s)?;
    rest.is_empty().then(|| days_from_civil(y, m, d) as f64 * MS_PER_DAY)
}

/// Valid month string → months since 1970-01.
pub(crate) fn parse_month(s: &str) -> Option<f64> {
    let (y, m, rest) = month_component(s)?;
    rest.is_empty().then(|| ((y - 1970) * 12 + m as i64 - 1) as f64)
}

/// Valid week string → ms since the epoch of the week's Monday.
pub(crate) fn parse_week(s: &str) -> Option<f64> {
    let (y, rest) = year(s)?;
    let rest = rest.strip_prefix("-W")?;
    let w = whole(digits(rest, 2))?;
    if w < 1 || w > weeks_in_year(y) {
        return None;
    }
    Some((week1_monday(y) + (w as i64 - 1) * 7) as f64 * MS_PER_DAY)
}

/// Valid time string → ms since midnight.
pub(crate) fn parse_time(s: &str) -> Option<f64> {
    whole(time_component(s))
}

/// Valid local date and time string (`T` or a space) → ms since the epoch.
pub(crate) fn parse_local_datetime(s: &str) -> Option<f64> {
    let (y, m, d, rest) = date_component(s)?;
    let rest = rest.strip_prefix(['T', ' '])?;
    let t = whole(time_component(rest))?;
    Some(days_from_civil(y, m, d) as f64 * MS_PER_DAY + t)
}

fn fmt_year(y: i64) -> String {
    format!("{y:04}")
}

/// `hh:mm`, `hh:mm:ss` or `hh:mm:ss.s{1,3}`: the shortest form.
fn fmt_time(ms: i64) -> String {
    let (h, rem) = (ms / 3_600_000, ms % 3_600_000);
    let (min, rem) = (rem / 60_000, rem % 60_000);
    let (sec, frac) = (rem / 1000, rem % 1000);
    let mut out = format!("{h:02}:{min:02}");
    if sec != 0 || frac != 0 {
        out.push_str(&format!(":{sec:02}"));
        if frac != 0 {
            let f = format!("{frac:03}");
            out.push('.');
            out.push_str(f.trim_end_matches('0'));
        }
    }
    out
}

fn in_date_range(days: i64) -> bool {
    (days_from_civil(1, 1, 1)..=days_from_civil(MAX_YEAR, 9, 13)).contains(&days)
}

/// ms since the epoch → valid date string (the day it falls in).
pub(crate) fn format_date(ms: f64) -> Option<String> {
    let days = (ms / MS_PER_DAY).floor() as i64;
    in_date_range(days).then(|| {
        let (y, m, d) = civil_from_days(days);
        format!("{}-{m:02}-{d:02}", fmt_year(y))
    })
}

/// Months since 1970-01 → valid month string.
pub(crate) fn format_month(months: f64) -> Option<String> {
    let months = months.floor() as i64;
    let y = 1970 + months.div_euclid(12);
    let m = months.rem_euclid(12) + 1;
    (1..=MAX_YEAR).contains(&y).then(|| format!("{}-{m:02}", fmt_year(y)))
}

/// ms since the epoch → the valid week string of the ISO week it falls in.
pub(crate) fn format_week(ms: f64) -> Option<String> {
    let days = (ms / MS_PER_DAY).floor() as i64;
    if !in_date_range(days) {
        return None;
    }
    // The ISO week-numbering year is the year of the week's Thursday.
    let thursday = days - weekday(days) + 3;
    let (wy, _, _) = civil_from_days(thursday);
    let w = (days - week1_monday(wy)) / 7 + 1;
    (1..=MAX_YEAR).contains(&wy).then(|| format!("{}-W{w:02}", fmt_year(wy)))
}

/// ms since midnight (taken modulo one day) → the shortest valid time string.
pub(crate) fn format_time(ms: f64) -> Option<String> {
    let ms = ms.rem_euclid(MS_PER_DAY).floor() as i64;
    Some(fmt_time(ms))
}

/// ms since the epoch → valid normalized local date and time string.
pub(crate) fn format_local_datetime(ms: f64) -> Option<String> {
    let days = (ms / MS_PER_DAY).floor() as i64;
    let date = format_date(ms)?;
    let t = (ms - days as f64 * MS_PER_DAY).floor() as i64;
    Some(format!("{date}T{}", fmt_time(t)))
}

/// A Date's time value (ms, UTC) → the year and month it falls in, as
/// months since 1970-01 (None outside the representable dates: a script's
/// `valueOf` can return any number).
pub(crate) fn months_of_date(ms: f64) -> Option<f64> {
    let days = (ms / MS_PER_DAY).floor() as i64;
    if !in_date_range(days) {
        return None;
    }
    let (y, m, _) = civil_from_days(days);
    Some(((y - 1970) * 12 + m as i64 - 1) as f64)
}

/// Months since 1970-01 → ms since the epoch of the month's first day.
pub(crate) fn date_of_months(months: f64) -> f64 {
    let months = months as i64;
    days_from_civil(1970 + months.div_euclid(12), (months.rem_euclid(12) + 1) as u32, 1) as f64 * MS_PER_DAY
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        assert_eq!(parse_date("1970-01-02"), Some(MS_PER_DAY));
        assert_eq!(format_date(parse_date("2016-02-29").unwrap()).as_deref(), Some("2016-02-29"));
        assert_eq!(parse_date("2019-02-29"), None);
        assert_eq!(parse_date("0000-12-10"), None);
        assert_eq!(parse_week("2019-W50"), parse_date("2019-12-09"));
        assert_eq!(parse_week("2014-W53"), None);
        assert!(parse_week("2015-W53").is_some());
        assert_eq!(format_week(parse_date("2019-12-09").unwrap()).as_deref(), Some("2019-W50"));
        assert_eq!(format_week(parse_date("2021-01-01").unwrap()).as_deref(), Some("2020-W53"));
        assert_eq!(parse_month("1970-02"), Some(1.0));
        assert_eq!(format_month(-1.0).as_deref(), Some("1969-12"));
        assert_eq!(months_of_date(1e300), None);
        assert_eq!(months_of_date(parse_date("2020-03-15").unwrap()), parse_month("2020-03"));
        assert_eq!(parse_time("00:00:00.0000"), None);
        assert_eq!(parse_time("12:00:00.5"), Some(43_200_500.0));
        assert_eq!(format_time(43_200_010.0).as_deref(), Some("12:00:00.01"));
        assert_eq!(format_time(43_200_000.0).as_deref(), Some("12:00"));
        assert_eq!(
            format_local_datetime(parse_local_datetime("2022-04-19 12:34:56.010").unwrap()).as_deref(),
            Some("2022-04-19T12:34:56.01")
        );
        assert_eq!(parse_local_datetime("2014-01-01H11:11"), None);
    }
}
