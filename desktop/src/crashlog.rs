//! Where a panic goes when there is no terminal to see it.
//!
//! A Rust panic unwinds and the process exits 101, which means the app simply
//! vanishes and takes the one useful sentence with it. macOS files no crash
//! report for an exit — `~/Library/Logs/DiagnosticReports` stays empty, because
//! nothing was signalled — and the unified log does not carry the stderr of an
//! app launched from Finder. On Windows `#![windows_subsystem = "windows"]`
//! means there is no console to write to at all. So the only way anyone could
//! report the list panic this module was written for was "it terminates", and
//! the line number took a terminal and a session to get to.
//!
//! The hook writes the message, the source location and a backtrace beside the
//! settings file, and the About dialog names that file once it exists. The
//! location is the part that always survives: it is a static string in the
//! binary, so stripping does not touch it.

use std::backtrace::Backtrace;
use std::io::Write;
use std::panic::{Location, PanicHookInfo};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// How large the log may get before it is started again. A panic that repeats
/// on every launch should not grow a file without end, and it is the newest
/// report anyone ever asks for.
const MAX_BYTES: u64 = 64 * 1024;

/// The log's path, or `None` on a machine with no home directory to put it in.
pub fn path() -> Option<PathBuf> {
    crate::settings::config_dir().map(|d| d.join("panic.log"))
}

/// Install the hook. Called first thing in `main`, before anything can panic.
pub fn install() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // stderr first, and always: running from a terminal should look exactly
        // as it did before this module existed.
        default(info);
        if let Some(path) = path() {
            let trace = Backtrace::force_capture().to_string();
            let _ = append(&path, &report(&message(info), info.location(), &trace, now()));
        }
    }));
}

/// The payload as text. `panic!` hands over a `&str` or a `String`; anything
/// else came from `panic_any` and there is nothing to read.
fn message(info: &PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panicked with a payload that is not a string".to_string()
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// One report, in the shape a bug report can be pasted from.
fn report(message: &str, location: Option<&Location<'_>>, backtrace: &str, secs: u64) -> String {
    let at = match location {
        Some(l) => format!("{}:{}:{}", l.file(), l.line(), l.column()),
        None => "an unknown location".to_string(),
    };
    format!(
        "---- {} ----\nTapeti {} ({} {})\npanicked at {at}:\n{message}\n\n{}\n\n",
        utc(secs),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        backtrace.trim_end(),
    )
}

/// `YYYY-MM-DD HH:MM:SSZ` from a Unix timestamp: Howard Hinnant's
/// civil-from-days, because the date on a crash report is not worth a
/// dependency.
fn utc(secs: u64) -> String {
    let (days, tod) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let (h, m, s) = (tod / 3600, (tod / 60) % 60, tod % 60);
    format!("{year:04}-{month:02}-{day:02} {h:02}:{m:02}:{s:02}Z")
}

/// Append a report, starting the file again once it has grown past `MAX_BYTES`.
fn append(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::remove_file(path);
    }
    std::fs::OpenOptions::new().create(true).append(true).open(path)?.write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_the_ones_a_calendar_agrees_with() {
        assert_eq!(utc(0), "1970-01-01 00:00:00Z");
        assert_eq!(utc(1_789_750_542), "2026-09-18 16:55:42Z");
        // Both kinds of leap year: divisible by four, and by four hundred.
        assert_eq!(utc(1_709_164_800), "2024-02-29 00:00:00Z");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00:00Z");
    }

    /// The location is what a report is worth, so it has to be in there whole —
    /// file, line and column, the way the panic prints it.
    #[test]
    fn a_report_carries_the_location_and_the_message() {
        let text = report("index out of bounds", Some(Location::caller()), "<trace>", 0);
        assert!(text.contains("crashlog.rs:"), "no source file in {text:?}");
        assert!(text.contains("index out of bounds"), "no message in {text:?}");
        assert!(text.contains(env!("CARGO_PKG_VERSION")), "no version in {text:?}");
        assert!(text.contains("1970-01-01"), "no timestamp in {text:?}");
        assert!(text.contains("<trace>"), "no backtrace in {text:?}");
    }

    #[test]
    fn a_missing_location_is_not_a_second_panic() {
        assert!(report("boom", None, "", 0).contains("an unknown location"));
    }

    #[test]
    fn reports_accumulate_until_the_file_is_too_large() {
        let dir = std::env::temp_dir().join(format!("tapeti-crashlog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("panic.log");

        append(&path, "first\n").expect("the directory is created on the way");
        append(&path, "second\n").expect("write");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("first") && text.contains("second"), "both reports: {text:?}");

        append(&path, &"x".repeat(MAX_BYTES as usize + 1)).expect("write");
        append(&path, "after the cap\n").expect("write");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(!text.contains("first"), "the old reports were dropped");
        assert_eq!(text, "after the cap\n");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
