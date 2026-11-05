//! Dates and sizes as the window shows them.

use gtk::glib;

fn local(ms: i64, tz: &glib::TimeZone) -> Option<glib::DateTime> {
	glib::DateTime::from_unix_utc(ms.div_euclid(1000)).ok()?.to_timezone(tz).ok()
}

/// The mail list's short date: a time today, "Yesterday", a weekday this week, else a date.
pub fn short_date(ms: i64, now_ms: i64, tz: &glib::TimeZone) -> String {
	let (Some(t), Some(now)) = (local(ms, tz), local(now_ms, tz)) else { return String::new() };
	let day = |d: &glib::DateTime| (d.year(), d.day_of_year());
	let days_ago = {
		let a = glib::DateTime::new(tz, t.year(), t.month(), t.day_of_month(), 0, 0, 0.0);
		let b = glib::DateTime::new(tz, now.year(), now.month(), now.day_of_month(), 0, 0, 0.0);
		match (a, b) {
			(Ok(a), Ok(b)) => b.difference(&a).as_days(),
			_ => i64::MAX,
		}
	};
	let fmt = if day(&t) == day(&now) {
		"%H:%M"
	} else if days_ago == 1 {
		return "Yesterday".into();
	} else if (2..7).contains(&days_ago) {
		"%A"
	} else if t.year() == now.year() {
		"%e %b"
	} else {
		"%e %b %Y"
	};
	t.format(fmt).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// The reading pane's and quote's full date, e.g. "Tue, 3 Oct 2026, 14:05".
pub fn long_date(ms: i64, tz: &glib::TimeZone) -> String {
	local(ms, tz).and_then(|t| t.format("%a, %e %b %Y, %H:%M").ok()).map(|s| s.replace("  ", " ")).unwrap_or_default()
}

pub fn now_ms() -> i64 {
	glib::real_time() / 1000
}

pub fn size(bytes: u64) -> String {
	glib::format_size(bytes).to_string()
}

/// The toast once a mail is sent: "Sent", with the start of its subject when it has one.
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

	const HOUR: i64 = 3_600_000;
	// Thu, 30 Oct 2025 12:00 UTC.
	const NOW: i64 = 1_761_825_600_000;

	#[test]
	fn short_dates_get_coarser_with_age() {
		let utc = glib::TimeZone::utc();
		assert_eq!(short_date(NOW - HOUR, NOW, &utc), "11:00");
		assert_eq!(short_date(NOW - 20 * HOUR, NOW, &utc), "Yesterday");
		assert_eq!(short_date(NOW - 72 * HOUR, NOW, &utc), "Monday");
		assert_eq!(short_date(NOW - 30 * 24 * HOUR, NOW, &utc), "30 Sep");
		assert_eq!(short_date(NOW - 400 * 24 * HOUR, NOW, &utc), "25 Sep 2024");
	}

	#[test]
	fn the_sent_toast_names_the_mail() {
		assert_eq!(sent_toast("Lunch on Friday?"), "Sent “Lunch on Friday?”");
		assert_eq!(sent_toast("  "), "Sent");
		assert_eq!(sent_toast("Re: the quarterly numbers and what we do about them next"), "Sent “Re: the quarterly numbers and what we do…”");
		assert_eq!(sent_toast("Zürich\nplans"), "Sent “Zürich plans”");
	}

	#[test]
	fn long_date_is_complete() {
		assert_eq!(long_date(NOW, &glib::TimeZone::utc()), "Thu, 30 Oct 2025, 12:00");
	}
}
