//! Parse and render byte sizes, durations, counts, and times.

use crate::error::{Result, usage};
use crate::sys;

const SIZE_PREFIXES: &str = "kmgtpe";
const BYTE_UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
const AGE_UNITS: [(&str, i64); 4] = [("d", 86_400), ("h", 3_600), ("m", 60), ("s", 1)];

/// Split a decimal number with an optional fraction from its suffix.
fn split_number(value: &str) -> Option<(&str, &str, &str)> {
    let digits_end = value
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(value.len());
    if digits_end == 0 {
        return None;
    }
    let whole = &value[..digits_end];
    let rest = &value[digits_end..];
    if let Some(after_point) = rest.strip_prefix('.') {
        let fraction_end = after_point
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(after_point.len());
        if fraction_end == 0 {
            return None;
        }
        return Some((
            whole,
            &after_point[..fraction_end],
            &after_point[fraction_end..],
        ));
    }
    Some((whole, "", rest))
}

/// Multiply a decimal number by an integer exactly, rounding toward zero.
fn scale(whole: &str, fraction: &str, multiplier: u128) -> Option<u64> {
    let digits: u128 = format!("{whole}{fraction}").parse().ok()?;
    let divisor = 10u128.checked_pow(u32::try_from(fraction.len()).ok()?)?;
    let value = digits.checked_mul(multiplier)? / divisor;
    // SQLite stores signed 64-bit integers.
    if value > i64::MAX as u128 {
        return None;
    }
    u64::try_from(value).ok()
}

/// Parse a size such as `500MB`, `1.5GiB`, or `2T` into bytes.
///
/// `B` after a prefix makes it decimal (`GB` is 1000^3 bytes), while `iB` or
/// the prefix alone makes it binary (`GiB` and `G` are 1024^3), as in `du`.
pub fn parse_size(value: &str) -> Result<u64> {
    let invalid = || usage(format!("invalid size: {value}"));
    let (whole, fraction, suffix) = split_number(value.trim()).ok_or_else(invalid)?;
    let suffix = suffix.to_ascii_lowercase();
    let multiplier: u128 = if suffix.is_empty() || suffix == "b" {
        1
    } else {
        let mut characters = suffix.chars();
        let prefix = characters.next().ok_or_else(invalid)?;
        let power = SIZE_PREFIXES.find(prefix).ok_or_else(invalid)? as u32 + 1;
        let base: u128 = match characters.as_str() {
            "" | "ib" => 1024,
            "b" => 1000,
            _ => return Err(invalid()),
        };
        base.pow(power)
    };
    scale(whole, fraction, multiplier).ok_or_else(invalid)
}

/// Parse a duration with an `s`, `m`, `h`, `d`, or `w` suffix into seconds.
pub fn parse_duration(value: &str) -> Result<u64> {
    let invalid = || {
        usage(format!(
            "invalid duration: {value}; use a number with s, m, h, d, or w, such as 90d"
        ))
    };
    let (whole, fraction, suffix) = split_number(value.trim()).ok_or_else(invalid)?;
    let multiplier: u128 = match suffix.to_ascii_lowercase().as_str() {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        "w" => 604_800,
        _ => return Err(invalid()),
    };
    scale(whole, fraction, multiplier).ok_or_else(invalid)
}

/// Render seconds with the largest exact unit among days, hours, and minutes.
pub fn format_duration(seconds: u64) -> String {
    for (suffix, unit) in [("d", 86_400u64), ("h", 3_600), ("m", 60)] {
        if seconds != 0 && seconds % unit == 0 {
            return format!("{}{suffix}", seconds / unit);
        }
    }
    format!("{seconds}s")
}

/// Render bytes with an IEC unit, as in `1.5KiB` or `+12GiB`.
pub fn format_bytes(value: i64, signed: bool) -> String {
    let sign = if signed && value > 0 {
        "+"
    } else if signed && value < 0 {
        "-"
    } else {
        ""
    };
    let mut number = if signed {
        value.unsigned_abs() as f64
    } else {
        value as f64
    };
    let mut unit = 0;
    while number >= 1024.0 && unit < BYTE_UNITS.len() - 1 {
        number /= 1024.0;
        unit += 1;
    }
    if unit == 0 || number >= 10.0 {
        format!("{sign}{number:.0}{}", BYTE_UNITS[unit])
    } else {
        format!("{sign}{number:.1}{}", BYTE_UNITS[unit])
    }
}

/// Render unsigned bytes with an IEC unit.
pub fn format_size(value: u64) -> String {
    format_bytes(i64::try_from(value).unwrap_or(i64::MAX), false)
}

/// Render bytes exactly, with the shortest unit that [`parse_size`] reads.
pub fn format_exact_size(value: u64) -> String {
    let mut best = format!("{value}B");
    for (power, prefix) in SIZE_PREFIXES.to_ascii_uppercase().chars().enumerate() {
        for (base, suffix) in [(1024u128, "iB"), (1000u128, "B")] {
            let unit = base.pow(power as u32 + 1);
            for decimals in 0..3u32 {
                let scaled = u128::from(value) * 10u128.pow(decimals);
                if value != 0 && scaled % unit == 0 {
                    let number = scaled / unit;
                    let divisor = 10u128.pow(decimals);
                    let text = if decimals == 0 {
                        number.to_string()
                    } else {
                        format!(
                            "{}.{:0width$}",
                            number / divisor,
                            number % divisor,
                            width = decimals as usize
                        )
                    };
                    let candidate = format!("{text}{prefix}{suffix}");
                    if candidate.len() < best.len() {
                        best = candidate;
                    }
                    break;
                }
            }
        }
    }
    best
}

/// Render a duration with its largest unit and the next one, as in `3d4h`.
pub fn format_age(seconds: i64) -> String {
    let seconds = seconds.max(0);
    for pair in AGE_UNITS.windows(2) {
        let (suffix, size) = pair[0];
        let (next_suffix, next_size) = pair[1];
        if seconds >= size {
            let amount = seconds / size;
            let part = (seconds % size) / next_size;
            return if part > 0 {
                format!("{amount}{suffix}{part}{next_suffix}")
            } else {
                format!("{amount}{suffix}")
            };
        }
    }
    format!("{seconds}s")
}

/// Render an epoch as local wall-clock time to the minute.
pub fn format_local_time(epoch: f64) -> String {
    let time = sys::local_time(epoch.floor() as i64);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        time.year, time.month, time.day, time.hour, time.minute
    )
}

/// Render an epoch as an ISO 8601 local time with its UTC offset.
pub fn format_iso_local(epoch: f64) -> String {
    let time = sys::local_time(epoch.floor() as i64);
    let offset_minutes = time.offset_seconds / 60;
    let sign = if offset_minutes < 0 { '-' } else { '+' };
    let offset_minutes = offset_minutes.abs();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
        time.year,
        time.month,
        time.day,
        time.hour,
        time.minute,
        time.second,
        offset_minutes / 60,
        offset_minutes % 60
    )
}

/// Render an integer with thousands separators, as in `1,234,567`.
pub fn group(value: i128) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if value < 0 {
        grouped.push('-');
    }
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Render a signed count with an explicit sign, as in `+1,024`.
pub fn group_signed(value: i128) -> String {
    if value > 0 {
        format!("+{}", group(value))
    } else {
        group(value)
    }
}

/// Render a percentage of a whole, treating a non-positive whole as zero.
pub fn percent(part: i64, whole: i64) -> String {
    if whole <= 0 {
        return "0.0%".to_string();
    }
    format!("{:.1}%", part as f64 * 100.0 / whole as f64)
}

/// Render bytes as lowercase hexadecimal.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_parse_decimal_binary_and_du_suffixes() {
        assert_eq!(parse_size("1KiB").unwrap(), 1024);
        assert_eq!(parse_size("1GB").unwrap(), 1_000_000_000);
        assert_eq!(parse_size("2T").unwrap(), 2 * 1024u64.pow(4));
        assert_eq!(parse_size("1.5g").unwrap(), 3 * 512 * 1024u64.pow(2));
        assert_eq!(parse_size("1.1KB").unwrap(), 1100);
        assert_eq!(parse_size("10").unwrap(), 10);
        assert_eq!(parse_size("10b").unwrap(), 10);
        for invalid in ["10ib", "10 GiB", "GiB", "-1KiB", "1.KiB", "", "1Ki"] {
            assert!(parse_size(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn durations_parse_and_render() {
        assert_eq!(parse_duration("2h").unwrap(), 7200);
        assert_eq!(parse_duration("1.5d").unwrap(), 129_600);
        assert!(parse_duration("soon").is_err());
        assert_eq!(format_duration(7 * 86_400), "7d");
        assert_eq!(format_duration(90), "90s");
    }

    #[test]
    fn bytes_ages_and_exact_sizes_render() {
        assert_eq!(format_bytes(1536, false), "1.5KiB");
        assert_eq!(format_bytes(-2048, true), "-2.0KiB");
        assert_eq!(format_bytes(0, true), "0B");
        for (seconds, text) in [(0, "0s"), (61, "1m1s"), (3601, "1h"), (90_061, "1d1h")] {
            assert_eq!(format_age(seconds), text);
        }
        for (size, text) in [(0, "0B"), (2_000_000_000_000, "2TB"), (3 << 39, "1.5TiB")] {
            assert_eq!(format_exact_size(size), text);
        }
        for size in [
            1u64,
            999,
            1536,
            1100,
            5 * (1 << 20) + (1 << 19),
            i64::MAX as u64,
        ] {
            assert_eq!(parse_size(&format_exact_size(size)).unwrap(), size);
        }
    }

    #[test]
    fn counts_and_percentages_render() {
        assert_eq!(group(1_234_567), "1,234,567");
        assert_eq!(group(-1000), "-1,000");
        assert_eq!(group_signed(5), "+5");
        assert_eq!(percent(1, 3), "33.3%");
        assert_eq!(percent(1, 0), "0.0%");
    }
}
