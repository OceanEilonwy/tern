//! Dates and sizes as a mail list shows them, in the device's time zone.

use chrono::{DateTime, Datelike, Local, TimeZone};

/// The mail list's short date: a time today, "Yesterday", a weekday this week, else a date.
pub fn short_date(ms: i64, now_ms: i64) -> String {
	short_date_in(ms, now_ms, &Local)
}

fn short_date_in<Tz: TimeZone>(ms: i64, now_ms: i64, tz: &Tz) -> String
where
	Tz::Offset: std::fmt::Display,
{
	let (Some(t), Some(now)) = (at(ms, tz), at(now_ms, tz)) else { return String::new() };
	let days_ago = now.date_naive().signed_duration_since(t.date_naive()).num_days();
	let fmt = match days_ago {
		0 => "%H:%M",
		1 => return "Yesterday".into(),
		2..=6 => "%A",
		_ if t.year() == now.year() => "%-d %b",
		_ => "%-d %b %Y",
	};
	t.format(fmt).to_string()
}

/// The full date, e.g. "Tue, 3 Oct 2026, 14:05".
pub fn long_date(ms: i64) -> String {
	long_date_in(ms, &Local)
}

fn long_date_in<Tz: TimeZone>(ms: i64, tz: &Tz) -> String
where
	Tz::Offset: std::fmt::Display,
{
	at(ms, tz).map(|t| t.format("%a, %-d %b %Y, %H:%M").to_string()).unwrap_or_default()
}

fn at<Tz: TimeZone>(ms: i64, tz: &Tz) -> Option<DateTime<Tz>> {
	tz.timestamp_millis_opt(ms).single()
}

pub fn now_ms() -> i64 {
	Local::now().timestamp_millis()
}

/// Bytes as people read them, in decimal units as GLib and Android both do.
pub fn size(bytes: u64) -> String {
	const UNITS: [(u64, &str); 3] = [(1_000_000_000, "GB"), (1_000_000, "MB"), (1_000, "kB")];
	for (unit, name) in UNITS {
		if bytes >= unit {
			return format!("{:.1} {name}", bytes as f64 / unit as f64);
		}
	}
	if bytes == 1 { "1 byte".into() } else { format!("{bytes} bytes") }
}

/// The message once a mail is sent: "Sent", with the start of its subject when it has one.
pub fn sent_toast(subject: &str) -> String {
	const SHOWN: usize = 40;
	let subject = subject.split_whitespace().collect::<Vec<_>>().join(" ");
	if subject.is_empty() {
		return "Sent".into();
	}
	let excerpt = match subject.char_indices().nth(SHOWN) {
		Some((cut, _)) => format!("{}…", subject[..cut].trim_end()),
		None => subject,
	};
	format!("Sent “{excerpt}”")
}

#[cfg(test)]
mod tests {
	use super::*;
	use chrono::Utc;

	const HOUR: i64 = 3_600_000;
	// Thu, 30 Oct 2025 12:00 UTC.
	const NOW: i64 = 1_761_825_600_000;

	#[test]
	fn short_dates_get_coarser_with_age() {
		assert_eq!(short_date_in(NOW - HOUR, NOW, &Utc), "11:00");
		assert_eq!(short_date_in(NOW - 20 * HOUR, NOW, &Utc), "Yesterday");
		assert_eq!(short_date_in(NOW - 72 * HOUR, NOW, &Utc), "Monday");
		assert_eq!(short_date_in(NOW - 30 * 24 * HOUR, NOW, &Utc), "30 Sep");
		assert_eq!(short_date_in(NOW - 400 * 24 * HOUR, NOW, &Utc), "25 Sep 2024");
	}

	#[test]
	fn long_date_is_complete() {
		assert_eq!(long_date_in(NOW, &Utc), "Thu, 30 Oct 2025, 12:00");
	}

	#[test]
	fn sizes_read_as_people_read_them() {
		assert_eq!(size(512), "512 bytes");
		assert_eq!(size(1_700), "1.7 kB");
		assert_eq!(size(312_000_000), "312.0 MB");
		assert_eq!(size(1_200_000_000), "1.2 GB");
	}

	#[test]
	fn the_sent_message_names_the_mail() {
		assert_eq!(sent_toast("Lunch on Friday?"), "Sent “Lunch on Friday?”");
		assert_eq!(sent_toast("  "), "Sent");
		assert_eq!(sent_toast("Re: the quarterly numbers and what we do about them next"), "Sent “Re: the quarterly numbers and what we do…”");
	}
}
