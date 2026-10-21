//! GitHub's timestamps, read as JS `Date.parse` reads them.

/// Milliseconds since the epoch of an ISO 8601 timestamp, as `Date.parse` gives them for
/// the forms GitHub writes (`2026-10-07T11:50:00Z`) and their near relatives: `YYYY`,
/// `YYYY-MM` or `YYYY-MM-DD`, then optionally `T` (or `t`, or a space) and `HH:MM`, `:SS`,
/// a fraction (milliseconds; further digits ignored), and `Z` or `±HH:MM`. `None` where
/// `Date.parse` gives `NaN`, or where the text is outside these forms.
///
/// As in V8, a day up to 31 rolls over into the next month (`2026-02-30` is March 2nd), and
/// `24:00:00` is the next midnight. Unlike JS, a date-time with no offset is UTC, not local
/// time: this crate reads no clock or zone. GitHub always says `Z`.
#[must_use]
pub fn parse_iso(text: &str) -> Option<i64> {
    let mut at = Cursor(text.as_bytes());
    let year = at.digits(4)?;
    let (month, day) = if at.eat(b'-') {
        let month = at.digits(2)?;
        let day = if at.eat(b'-') { at.digits(2)? } else { 1 };
        (month, day)
    } else {
        (1, 1)
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut ms_of_day = 0;
    let mut offset_min = 0;
    if at.eat(b'T') || at.eat(b't') || at.eat(b' ') {
        let hour = at.digits(2)?;
        if !at.eat(b':') {
            return None;
        }
        let minute = at.digits(2)?;
        let (second, milli) = if at.eat(b':') {
            let second = at.digits(2)?;
            let milli = if at.eat(b'.') { at.fraction()? } else { 0 };
            (second, milli)
        } else {
            (0, 0)
        };
        let is_midnight_after = hour == 24 && minute == 0 && second == 0 && milli == 0;
        if (hour > 23 && !is_midnight_after) || minute > 59 || second > 59 {
            return None;
        }
        ms_of_day = ((hour * 60 + minute) * 60 + second) * 1000 + milli;
        let is_utc = at.eat(b'Z') || at.eat(b'z');
        if !is_utc && let Some(sign) = at.sign() {
            let hours = at.digits(2)?;
            if !at.eat(b':') {
                return None;
            }
            let minutes = at.digits(2)?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            offset_min = sign * (hours * 60 + minutes);
        }
    }
    if !at.0.is_empty() {
        return None;
    }

    Some(days_from_civil(year, month, day) * 86_400_000 + ms_of_day - offset_min * 60_000)
}

struct Cursor<'a>(&'a [u8]);

impl Cursor<'_> {
    fn eat(&mut self, byte: u8) -> bool {
        match self.0.split_first() {
            Some((&b, rest)) if b == byte => {
                self.0 = rest;
                true
            }
            _ => false,
        }
    }

    fn digits(&mut self, count: usize) -> Option<i64> {
        let (head, rest) = self.0.split_at_checked(count)?;
        let mut n = 0;
        for &b in head {
            if !b.is_ascii_digit() {
                return None;
            }
            n = n * 10 + i64::from(b - b'0');
        }
        self.0 = rest;
        Some(n)
    }

    /// One or more digits after the point, as milliseconds (the first three count).
    fn fraction(&mut self) -> Option<i64> {
        let len = self.0.iter().take_while(|b| b.is_ascii_digit()).count();
        if len == 0 {
            return None;
        }
        let mut milli = 0;
        for i in 0..3 {
            milli = milli * 10
                + self
                    .0
                    .get(i)
                    .filter(|_| i < len)
                    .map_or(0, |b| i64::from(b - b'0'));
        }
        self.0 = &self.0[len..];
        Some(milli)
    }

    fn sign(&mut self) -> Option<i64> {
        if self.eat(b'+') {
            Some(1)
        } else if self.eat(b'-') {
            Some(-1)
        } else {
            None
        }
    }
}

/// Days from 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
/// A day past the month's end counts on into the next.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::parse_iso;

    #[test]
    fn reads_what_date_parse_reads() {
        // Each from node's Date.parse.
        let cases: [(&str, Option<i64>); 20] = [
            ("2026-10-07T11:50:00Z", Some(1_791_373_800_000)),
            ("2026-10-07T11:50:00.123Z", Some(1_791_373_800_123)),
            ("2026-10-07T11:50:00.1Z", Some(1_791_373_800_100)),
            ("2026-10-07T11:50:00.123456Z", Some(1_791_373_800_123)),
            ("2026-10-07T11:50:00+02:00", Some(1_791_366_600_000)),
            ("2026-10-07T11:50Z", Some(1_791_373_800_000)),
            ("2026-10-07", Some(1_791_331_200_000)),
            ("2026-02-30T00:00:00Z", Some(1_772_409_600_000)),
            ("2024-02-29T00:00:00Z", Some(1_709_164_800_000)),
            ("2026-10-07T24:00:00Z", Some(1_791_417_600_000)),
            ("2026-10-07T24:00:01Z", None),
            ("2026-10-07t11:50:00z", Some(1_791_373_800_000)),
            ("2026-10-07 11:50:00Z", Some(1_791_373_800_000)),
            ("1970-01-01T00:00:00Z", Some(0)),
            ("2026-10-07T11:50:60Z", None),
            ("2026-10", Some(1_790_812_800_000)),
            ("2026", Some(1_767_225_600_000)),
            ("2026-1-07T00:00:00Z", None),
            ("garbage", None),
            ("", None),
        ];
        for (text, ms) in cases {
            assert_eq!(parse_iso(text), ms, "{text:?}");
        }
    }

    #[test]
    fn never_panics_on_odd_text() {
        for text in [
            "2026-10-07T",
            "2026-10-07T11",
            "2026-10-07T11:50:00.",
            "2026-10-07T11:50:00+0",
            "é",
            "2026-10-07T11:50:00.12é",
        ] {
            let _ = parse_iso(text);
        }
    }
}
