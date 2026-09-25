//! Resource figures: what is measured, over what range, and how it reads.

use serde::{Deserialize, Serialize};

/// Most points in any series sent to a client. A year and an hour cost a
/// phone the same.
pub const MAX_POINTS: usize = 300;
/// Seconds between live points.
pub const LIVE_STEP: i64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    Host,
    Container,
    Disk,
    Network,
}

impl SubjectKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Container => "container",
            Self::Disk => "disk",
            Self::Network => "network",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        [Self::Host, Self::Container, Self::Disk, Self::Network]
            .into_iter()
            .find(|k| k.as_str() == text)
    }
}

/// What a series describes: one measured subject, or a stack's containers
/// summed. Written `kind:key`, or `host`, or `stack:<project>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    Subject(SubjectKind, String),
    Stack(String),
}

impl Target {
    #[must_use]
    pub fn host() -> Self {
        Self::Subject(SubjectKind::Host, "host".to_owned())
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        if text == "host" {
            return Some(Self::host());
        }
        let (kind, key) = text.split_once(':')?;
        let fine = !key.is_empty() && key.len() <= 255 && !key.chars().any(char::is_control);
        if !fine {
            return None;
        }
        if kind == "stack" {
            return Some(Self::Stack(key.to_owned()));
        }
        match SubjectKind::parse(kind)? {
            SubjectKind::Host => None,
            other => Some(Self::Subject(other, key.to_owned())),
        }
    }

    #[must_use]
    pub fn to_param(&self) -> String {
        match self {
            Self::Subject(SubjectKind::Host, _) => "host".to_owned(),
            Self::Subject(kind, key) => format!("{}:{key}", kind.as_str()),
            Self::Stack(project) => format!("stack:{project}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Range {
    #[serde(rename = "1h")]
    Hour,
    #[serde(rename = "24h")]
    Day,
    #[serde(rename = "7d")]
    Week,
    #[serde(rename = "30d")]
    Month,
    #[serde(rename = "1y")]
    Year,
}

/// Where a range's points come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// The server's in-memory hour, one point per 5 s.
    Live,
    /// Stored 1-minute rows.
    Minute,
    /// Stored 15-minute rows.
    Quarter,
}

impl Resolution {
    #[must_use]
    pub fn step(self) -> i64 {
        match self {
            Self::Live => LIVE_STEP,
            Self::Minute => 60,
            Self::Quarter => 900,
        }
    }
}

impl Range {
    pub const ALL: [Self; 5] = [Self::Hour, Self::Day, Self::Week, Self::Month, Self::Year];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hour => "1h",
            Self::Day => "24h",
            Self::Week => "7d",
            Self::Month => "30d",
            Self::Year => "1y",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == text)
    }

    #[must_use]
    pub fn seconds(self) -> i64 {
        match self {
            Self::Hour => 3_600,
            Self::Day => 86_400,
            Self::Week => 7 * 86_400,
            Self::Month => 30 * 86_400,
            Self::Year => 365 * 86_400,
        }
    }

    #[must_use]
    pub fn resolution(self) -> Resolution {
        match self {
            Self::Hour => Resolution::Live,
            Self::Day | Self::Week | Self::Month => Resolution::Minute,
            Self::Year => Resolution::Quarter,
        }
    }
}

/// One point: averages over its interval, and peaks, so thinning a series
/// never hides a spike. A field is `None` where it does not apply.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Reading {
    /// Unix seconds at the start of the interval.
    pub t: i64,
    /// CPU in cores.
    pub cpu: Option<f64>,
    pub cpu_max: Option<f64>,
    /// Memory in bytes; for a disk, space used.
    pub mem: Option<u64>,
    pub mem_max: Option<u64>,
    /// Memory limit; for the host, total memory; for a disk, capacity.
    pub mem_limit: Option<u64>,
    /// Bytes per second.
    pub net_rx: Option<f64>,
    pub net_tx: Option<f64>,
    pub io_read: Option<f64>,
    pub io_write: Option<f64>,
    /// Fraction of CPU periods throttled by a CPU limit.
    pub throttled: Option<f64>,
    /// 1-minute load average; the host only.
    pub load: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Series {
    pub target: String,
    pub range: Range,
    /// Seconds between the stored points the series was built from.
    pub step: i64,
    pub points: Vec<Reading>,
}

/// The latest 5 s figures, for the board, the Host screen and live ticks.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Now {
    pub at: i64,
    pub host: Option<Reading>,
    pub host_cpus: Option<u32>,
    pub containers: Vec<Current>,
    pub stacks: Vec<StackNow>,
    pub disks: Vec<Current>,
    pub networks: Vec<Current>,
    /// True while the Docker daemon cannot be reached.
    pub containers_unavailable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Current {
    pub key: String,
    pub project: Option<String>,
    pub reading: Reading,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StackNow {
    pub project: String,
    pub reading: Reading,
    /// The last hour of CPU, one point a minute, oldest first.
    pub cpu_hour: Vec<Option<f64>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Bad,
    Degraded,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flag {
    pub severity: Severity,
    pub text: String,
}

/// One container's figures over a range, for its stack's page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerFigures {
    pub key: String,
    pub service: Option<String>,
    /// The 95th percentile of its averages: what it typically needs.
    pub cpu_typical: Option<f64>,
    /// The highest seen, peaks inside an average included.
    pub cpu_peak: Option<f64>,
    pub mem_typical: Option<u64>,
    pub mem_peak: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recommendation {
    pub container: String,
    pub project: Option<String>,
    pub service: Option<String>,
    /// Days of running history the advice rests on.
    pub days: f64,
    /// Plain words: what was seen, or why nothing is suggested yet.
    pub evidence: String,
    pub cpus: Option<f64>,
    pub memory: Option<u64>,
    pub flags: Vec<Flag>,
    /// Compose lines to paste, when there is a suggestion.
    pub snippet: Option<String>,
}

const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];

/// `value` with `places` decimals, as `format!("{value:.places$}")` lays it
/// out: halves go to the even digit, and the sign stays on a negative that
/// rounds to zero.
///
/// Worked in integers because formatting a float compiles in Rust's whole
/// float printer, about 12 KB of the web client for figures that never need
/// more than a few decimals. Exact, like Rust's: the rounding is done on the
/// float's own binary value, not on a product that may itself have rounded.
/// Saturates past `u64::MAX` units of the last place.
#[must_use]
pub fn fixed(value: f64, places: u8) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    let sign = if value.is_sign_negative() { "-" } else { "" };
    if value.is_infinite() {
        return format!("{sign}inf");
    }
    let factor = 10_u64.pow(u32::from(places));
    let scaled = scale_exactly(value.abs(), factor);
    let whole = scaled / factor;
    if places == 0 {
        return format!("{sign}{whole}");
    }
    let width = usize::from(places);
    format!("{sign}{whole}.{:0width$}", scaled % factor)
}

/// `value * factor` rounded to a whole number, halves to even, computed on
/// the exact binary value: `value` is `mantissa * 2^exponent`.
fn scale_exactly(value: f64, factor: u64) -> u64 {
    let bits = value.to_bits();
    let biased = (bits >> 52) & 0x7ff;
    let fraction = bits & ((1 << 52) - 1);
    let (mantissa, exponent) = if biased == 0 {
        (fraction, -1074_i64)
    } else {
        #[allow(clippy::cast_possible_wrap)] // 11 bits.
        (fraction | (1 << 52), biased as i64 - 1075)
    };
    // Below 2^63: a 53-bit mantissa times a factor under 2^10 per place.
    let n = u128::from(mantissa) * u128::from(factor);
    if exponent >= 0 {
        // A whole number already; past 2^64 it saturates either way.
        return u32::try_from(exponent)
            .ok()
            .filter(|e| *e < 64)
            .and_then(|e| u64::try_from(n << e).ok())
            .unwrap_or(u64::MAX);
    }
    let Ok(shift) = u32::try_from(-exponent) else {
        return 0;
    };
    if shift >= 127 {
        // Less than a half of the last place.
        return 0;
    }
    let whole = n >> shift;
    let rest = n - (whole << shift);
    let half = 1_u128 << (shift - 1);
    let rounded = if rest > half || (rest == half && whole & 1 == 1) {
        whole + 1
    } else {
        whole
    };
    u64::try_from(rounded).unwrap_or(u64::MAX)
}

/// `value` with at most `places` decimals and no trailing zeros, as a
/// person would write a threshold: 80, 80.5.
#[must_use]
pub fn trimmed(value: f64, places: u8) -> String {
    let text = fixed(value, places);
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    }
}

/// Binary units, as memory is sized.
#[must_use]
pub fn format_bytes(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)] // A display figure.
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let name = UNITS.get(unit).copied().unwrap_or("TiB");
    if unit == 0 {
        format!("{bytes} B")
    } else if value < 10.0 {
        format!("{} {name}", fixed(value, 1))
    } else {
        format!("{} {name}", fixed(value, 0))
    }
}

#[must_use]
pub fn format_rate(bytes_per_second: f64) -> String {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    // Non-negative display figure.
    let whole = bytes_per_second.max(0.0).round() as u64;
    format!("{}/s", format_bytes(whole))
}

#[must_use]
pub fn format_cores(cores: f64) -> String {
    if cores < 10.0 {
        format!("{} cores", fixed(cores, 2))
    } else {
        format!("{} cores", fixed(cores, 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_decimals_read_as_rust_formats_them() {
        let mut values = vec![
            0.0,
            -0.0,
            0.5,
            1.5,
            2.5,
            -0.5,
            0.125,
            0.375,
            2.675,
            1.005,
            0.04,
            -0.04,
            9.995,
            99.95,
            1e9,
            123_456.789,
            599.96,
            4_503_599_627_370_497.0,
            1e17,
            5e-324,
            f64::MIN_POSITIVE,
            f64::NAN,
            f64::INFINITY,
            -f64::INFINITY,
        ];
        // Chart coordinates, percentages and cores: the figures shown.
        values.extend((0..6000).map(|i| f64::from(i) * 0.1037));
        values.extend((0..2000).map(|i| f64::from(i) / 64.0));
        for v in values {
            assert_eq!(fixed(v, 0), format!("{v:.0}"), "{v}");
            assert_eq!(fixed(v, 1), format!("{v:.1}"), "{v}");
            assert_eq!(fixed(v, 2), format!("{v:.2}"), "{v}");
        }
        assert_eq!(trimmed(80.0, 3), "80");
        assert_eq!(trimmed(80.5, 3), "80.5");
        assert_eq!(trimmed(80.25, 3), "80.25");
        assert_eq!(trimmed(0.0, 3), "0");
    }

    #[test]
    fn targets_round_trip_through_their_parameter_form() {
        for text in [
            "host",
            "container:blog-web-1",
            "disk:/host/disks/media",
            "network:eth0",
            "stack:blog",
        ] {
            let target = Target::parse(text).unwrap();
            assert_eq!(target.to_param(), text);
        }
        assert_eq!(Target::parse("container:"), None);
        assert_eq!(Target::parse("planet:earth"), None);
        assert_eq!(Target::parse("container:a\nb"), None);
    }

    #[test]
    fn ranges_name_themselves_and_choose_a_resolution() {
        for range in Range::ALL {
            assert_eq!(Range::parse(range.as_str()), Some(range));
        }
        assert_eq!(Range::Hour.resolution(), Resolution::Live);
        assert_eq!(Range::Month.resolution(), Resolution::Minute);
        assert_eq!(Range::Year.resolution(), Resolution::Quarter);
        assert_eq!(Range::Week.seconds(), 7 * 86_400);
    }

    #[test]
    fn sizes_read_as_a_person_would_say_them() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(412 * 1024 * 1024), "412 MiB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024 / 2), "1.5 GiB");
        assert_eq!(format_rate(2048.0), "2.0 KiB/s");
        assert_eq!(format_cores(0.3), "0.30 cores");
        assert_eq!(format_cores(12.26), "12.3 cores");
    }
}
