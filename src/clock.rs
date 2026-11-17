//! Times as the match files write them (UTC) and as the console shows them (local).

use time::OffsetDateTime;

/// Like `2026-11-09T20:31:22.123Z`.
pub fn iso(t: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        t.year(),
        t.month() as u8,
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        t.millisecond()
    )
}

pub fn now() -> String {
    iso(OffsetDateTime::now_utc())
}

/// The local time of day, for the live trace.
pub fn local_clock() -> String {
    let mut t = windows_sys::Win32::Foundation::SYSTEMTIME::default();
    // SAFETY: t is a writable SYSTEMTIME.
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&raw mut t) };
    format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond)
}

/// A time the match files wrote, in local time.
fn local(iso: &str) -> Option<windows_sys::Win32::Foundation::SYSTEMTIME> {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    let n = |at: usize, len: usize| iso.get(at..at + len)?.parse::<u16>().ok();
    let utc = SYSTEMTIME {
        wYear: n(0, 4)?,
        wMonth: n(5, 2)?,
        wDay: n(8, 2)?,
        wHour: n(11, 2)?,
        wMinute: n(14, 2)?,
        wSecond: n(17, 2)?,
        ..Default::default()
    };
    let mut t = SYSTEMTIME::default();
    // SAFETY: utc is read and t is a writable SYSTEMTIME; a null time zone means the
    // current one.
    let ok = unsafe {
        windows_sys::Win32::System::Time::SystemTimeToTzSpecificLocalTime(std::ptr::null(), &raw const utc, &raw mut t)
    };
    (ok != 0).then_some(t)
}

/// Seconds since 1970 of a time the match files wrote.
pub fn seconds(iso: &str) -> Option<i64> {
    let n = |at: usize, len: usize| iso.get(at..at + len)?.parse::<i64>().ok();
    let (y, m, d) = (n(0, 4)?, n(5, 2)?, n(8, 2)?);
    // Days from the civil calendar, as in Howard Hinnant's days_from_civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + n(11, 2)? * 3600 + n(14, 2)? * 60 + n(17, 2)?)
}

/// The local hour and minute of a time the match files wrote, like `20:31`.
pub fn local_minute(iso: &str) -> Option<String> {
    local(iso).map(|t| format!("{:02}:{:02}", t.wHour, t.wMinute))
}

/// The local day of a time the match files wrote, like `Nov 9`.
pub fn local_day(iso: &str) -> Option<String> {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    local(iso).and_then(|t| Some(format!("{} {}", MONTHS.get(t.wMonth.checked_sub(1)? as usize)?, t.wDay)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn iso_is_utc_with_milliseconds() {
        let t = time::OffsetDateTime::from_unix_timestamp_nanos(1_794_256_282_123_000_000).unwrap();
        assert_eq!(super::iso(t), "2026-11-09T20:31:22.123Z");
    }

    #[test]
    fn local_minute_reads_the_files_times() {
        let shown = super::local_minute("2026-11-09T20:31:22.123Z").unwrap();
        assert!(shown.len() == 5 && shown.as_bytes()[2] == b':', "{shown}");
        assert_eq!(super::local_minute(""), None);
        assert_eq!(super::seconds("2026-11-09T20:31:22.123Z"), Some(1_794_256_282));
        assert!(super::local_day("2026-11-09T20:31:22.123Z").unwrap().starts_with("Nov "));
        assert_eq!(super::local_minute("2026-13-09T20:31:22.123Z"), None);
    }
}
