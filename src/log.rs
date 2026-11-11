//! The log: one line per request in `~/.local/state/hiss/hiss.log`, or in
//! `$XDG_STATE_HOME/hiss/hiss.log` when that is set. Timings and sizes only,
//! never code, paths or keys.
//!
//! Lines go through a channel to a thread of their own, so writing never
//! blocks Helix or the runtime, and a line that cannot be written is dropped.
//! A log that has grown past `KEEP` bytes moves to `hiss.log.1` when that
//! thread starts, replacing the one there, so the two stay small.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::api::Timing;

static ON: AtomicBool = AtomicBool::new(true);

/// About ten thousand lines, a few days of steady use.
const KEEP: u64 = 1 << 20;

static LINES: LazyLock<Option<Sender<String>>> = LazyLock::new(|| {
   let path = path(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME"))?;
   let (sender, receiver) = mpsc::channel::<String>();
   std::thread::Builder::new()
      .name("hiss-log".into())
      .spawn(move || {
         rotate(&path, KEEP);
         let mut file = None;
         for line in receiver {
            if file.is_none() {
               file = open(&path).ok();
            }
            if let Some(open) = file.as_mut()
               && open.write_all(line.as_bytes()).is_err()
            {
               file = None;
            }
         }
      })
      .ok()?;
   Some(sender)
});

/// Turns the log on or off (`log` in `config.toml`).
pub fn set_on(on: bool) {
   ON.store(on, Ordering::Relaxed);
}

/// Appends `event` with a time stamp, unless the log is off.
pub fn write(event: &str) {
   if ON.load(Ordering::Relaxed)
      && let Some(sender) = LINES.as_ref()
   {
      let _ = sender.send(format!("{} {event}\n", stamp(SystemTime::now())));
   }
}

/// The fields of a finished request, as `key=value`, times in ms.
pub fn timing_fields(timing: &Timing) -> String {
   let mut fields = format!(
      "ttft={} total={} headers={} in={} cached={} out={} chars={} stop={}",
      ms_or_dash(timing.first_text),
      timing.total.as_millis(),
      timing.headers.as_millis(),
      timing.input_tokens,
      timing.cache_read_tokens,
      timing.output_tokens,
      timing.chars,
      word(&timing.stop_reason),
   );
   if timing.retried {
      fields.push_str(" retried");
   }
   if timing.error_retries > 0 {
      fields.push_str(&format!(" again={}", timing.error_retries));
   }
   fields
}

pub fn ms_or_dash(duration: Option<Duration>) -> String {
   duration.map_or("-".into(), |d| d.as_millis().to_string())
}

/// A value with no spaces, so each line splits on whitespace.
pub fn word(text: &str) -> String {
   if text.is_empty() {
      "-".into()
   } else {
      text.replace(char::is_whitespace, "-")
   }
}

fn path(state: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
   let absolute = |path: PathBuf| path.is_absolute().then_some(path);
   state
      .map(PathBuf::from)
      .and_then(absolute)
      .or_else(|| {
         home
            .map(|home| PathBuf::from(home).join(".local/state"))
            .and_then(absolute)
      })
      .map(|state| state.join("hiss").join("hiss.log"))
}

fn open(path: &Path) -> std::io::Result<File> {
   if let Some(dir) = path.parent() {
      fs::create_dir_all(dir)?;
   }
   OpenOptions::new().create(true).append(true).open(path)
}

/// Moves the log at `path` to `<path>.1` when it is larger than `keep`.
fn rotate(path: &Path, keep: u64) {
   if fs::metadata(path).is_ok_and(|meta| meta.len() > keep) {
      let mut old = path.as_os_str().to_owned();
      old.push(".1");
      let _ = fs::rename(path, old);
   }
}

/// UTC, as `2026-11-10T14:03:22Z`.
fn stamp(time: SystemTime) -> String {
   let secs = time
      .duration_since(UNIX_EPOCH)
      .unwrap_or_default()
      .as_secs();
   let (year, month, day) = civil((secs / 86_400) as i64);
   let rest = secs % 86_400;
   format!(
      "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
      rest / 3_600,
      rest / 60 % 60,
      rest % 60
   )
}

/// Days since 1970-01-01 as a date (Hinnant's `civil_from_days`).
fn civil(days: i64) -> (i64, u32, u32) {
   let z = days + 719_468;
   let era = z.div_euclid(146_097);
   let doe = z.rem_euclid(146_097);
   let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
   let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
   let mp = (5 * doy + 2) / 153;
   let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
   let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
   (yoe + era * 400 + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn path_prefers_an_absolute_state_home() {
      let some = |s: &str| Some(OsString::from(s));
      assert_eq!(
         path(some("/s"), some("/h")),
         Some(PathBuf::from("/s/hiss/hiss.log"))
      );
      assert_eq!(
         path(some("relative"), some("/h")),
         Some(PathBuf::from("/h/.local/state/hiss/hiss.log"))
      );
      assert_eq!(
         path(None, some("/h")),
         Some(PathBuf::from("/h/.local/state/hiss/hiss.log"))
      );
      assert_eq!(path(None, None), None);
   }

   #[test]
   fn a_large_log_moves_aside() {
      let dir = std::env::temp_dir().join(format!("hiss-log-test-{}", std::process::id()));
      let log = dir.join("hiss.log");
      let old = dir.join("hiss.log.1");
      let mut file = open(&log).unwrap();
      file.write_all(b"0123456789").unwrap();
      rotate(&log, 10);
      assert!(log.exists() && !old.exists());
      file.write_all(b"!").unwrap();
      rotate(&log, 10);
      assert!(!log.exists() && fs::read(&old).unwrap().len() == 11);
      let _ = fs::remove_dir_all(&dir);
   }

   #[test]
   fn stamp_is_utc() {
      assert_eq!(stamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
      let leap = UNIX_EPOCH + Duration::from_secs(951_782_400 + 3_661);
      assert_eq!(stamp(leap), "2000-02-29T01:01:01Z");
      let later = UNIX_EPOCH + Duration::from_secs(1_794_312_000);
      assert_eq!(stamp(later), "2026-11-10T12:00:00Z");
   }

   #[test]
   fn fields_split_on_whitespace() {
      let timing = Timing {
         first_text: Some(Duration::from_millis(540)),
         total: Duration::from_millis(839),
         stop_reason: "blank line".into(),
         chars: 12,
         retried: true,
         ..Timing::default()
      };
      assert_eq!(
         timing_fields(&timing),
         "ttft=540 total=839 headers=0 in=0 cached=0 out=0 chars=12 stop=blank-line retried"
      );
      assert_eq!(word(""), "-");
   }
}
