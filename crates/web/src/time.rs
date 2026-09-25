//! Times as the person reading them keeps time.
//!
//! The server speaks UTC. Shown as it arrives, a deploy at nine in the
//! morning read as seven, beside a chart whose axis said nine. Every time on
//! screen goes through here, so they all agree with each other and with the
//! clock on the wall.

use chrono::{DateTime, Datelike as _, Local, Timelike as _, Utc};

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `at` in this device's time zone, as `format` lays it out.
///
/// `format` takes the strftime directives this client uses and no others:
/// `%d`, `%b`, `%Y`, `%H` and `%M`, laid out as chrono would. Written out
/// rather than handed to chrono's formatter, which parses the pattern at
/// run time and so brings in every directive there is: about 8 KB of the
/// bundle for five of them.
#[must_use]
pub fn local(at: DateTime<Utc>, format: &str) -> String {
    let at = at.with_timezone(&Local);
    let two = |out: &mut String, n: u32| {
        out.push(char::from_digit(n / 10 % 10, 10).unwrap_or('0'));
        out.push(char::from_digit(n % 10, 10).unwrap_or('0'));
    };
    let mut out = String::with_capacity(format.len() + 8);
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('d') => two(&mut out, at.day()),
            Some('H') => two(&mut out, at.hour()),
            Some('M') => two(&mut out, at.minute()),
            Some('b') => out.push_str(MONTHS.get(at.month0() as usize).copied().unwrap_or("")),
            Some('Y') => out.push_str(&at.year().to_string()),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// A Unix timestamp in this device's time zone; empty if out of range.
#[must_use]
pub fn local_timestamp(t: i64, format: &str) -> String {
    DateTime::from_timestamp(t, 0).map_or_else(String::new, |at| local(at, format))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_is_shown_in_the_local_zone() {
        let at = DateTime::from_timestamp(1_700_000_000, 0).unwrap_or_default();
        let expected = at.with_timezone(&Local).format("%d %b %H:%M").to_string();
        assert_eq!(local(at, "%d %b %H:%M"), expected);
        assert_eq!(local_timestamp(1_700_000_000, "%d %b %H:%M"), expected);
        assert_eq!(local_timestamp(i64::MAX, "%H:%M"), "");
    }

    #[test]
    fn every_layout_in_use_reads_as_chrono_would_lay_it_out() {
        let layouts = [
            "%d %b %H:%M",
            "%d %b %Y",
            "%d %b %Y, %H:%M",
            "%H:%M",
            "%d %b",
            "%b %Y",
        ];
        // A day in each month, at times that test both digits of each field.
        for (i, t) in (0..12)
            .map(|m| 1_704_067_200 + m * 31 * 86_400 + m * 3_725)
            .enumerate()
        {
            let at = DateTime::from_timestamp(t, 0).unwrap_or_default();
            for layout in layouts {
                let expected = at.with_timezone(&Local).format(layout).to_string();
                assert_eq!(local(at, layout), expected, "month {i}, {layout}");
            }
        }
    }
}
