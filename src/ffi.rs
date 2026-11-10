//! The functions Steel sees (spec 03/08).
//!
//! `hiss-start!` starts a completion and returns its id at once.
//! `hiss-next` returns a future of the whole cleaned ghost text so far, or of
//! `#f` once the stream has ended, failed or been cancelled. `hiss-cancel!`
//! aborts a request. Only one request runs at a time: starting one aborts the
//! others, since only the newest can still be shown.
//!
//! Futures handed to Steel are polled on Helix's runtime, so they only wait on
//! channels, and each poll runs under `catch_unwind`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use abi_stable::std_types::{RBoxError, RResult};
use steel::steel_vm::ffi::{FFIValue, FfiFuture, FfiFutureExt};
use tokio::sync::watch;

use crate::api::{Client, Request, Settings, Timing};
use crate::{RUNTIME, clean, guarded, log};

struct Entry {
   text: Arc<tokio::sync::Mutex<watch::Receiver<String>>>,
   task: tokio::task::AbortHandle,
}

static CLIENT: LazyLock<Result<Arc<Client>, String>> =
   LazyLock::new(|| Client::from_env().map(Arc::new));
static REQUESTS: LazyLock<Mutex<HashMap<usize, Entry>>> = LazyLock::new(Default::default);
static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

/// The last error and the last timing, for the status line and the log.
#[derive(Default)]
struct Last {
   error: Option<String>,
   timing: Option<String>,
}
static LAST: LazyLock<Mutex<Last>> = LazyLock::new(Default::default);

fn with_last<T>(f: impl FnOnce(&mut Last) -> T) -> T {
   f(&mut LAST.lock().unwrap_or_else(|e| e.into_inner()))
}

fn requests<T>(f: impl FnOnce(&mut HashMap<usize, Entry>) -> T) -> T {
   f(&mut REQUESTS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Opens the connection in the background when the module loads.
pub fn warm_up() {
   let _ = guarded("warm-up", || {
      if let Ok(client) = CLIENT.as_ref() {
         let client = Arc::clone(client);
         RUNTIME.spawn(async move {
            match client.warm().await {
               Ok(took) => log::write(&format!("warm after={}", took.as_millis())),
               Err(error) => {
                  log::write(&format!("warm error {}", log::word(&error)));
                  with_last(|last| last.error = Some(error));
               }
            }
         });
      }
   });
}

/// `(hiss-start! prefix suffix language path)`: the new request's id, or `#f`
/// when there is no client (see `hiss-error`).
pub fn start(
   prefix: String,
   suffix: String,
   language: String,
   path: String,
) -> Result<Option<usize>, String> {
   guarded("hiss-start!", || {
      let client = match CLIENT.as_ref() {
         Ok(client) => Arc::clone(client),
         Err(error) => {
            with_last(|last| last.error = Some(error.clone()));
            return None;
         }
      };
      let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
      let (sender, receiver) = watch::channel(String::new());
      let settings = Settings::default();
      let mut trace = Trace::new(id, &prefix, &suffix, &settings);
      let request = Request {
         prefix,
         suffix,
         language,
         path,
      };
      let task = RUNTIME.spawn(async move {
         let start = trace.start;
         let result = client
            .complete(&request, &settings, |text| {
               if !text.is_empty() {
                  trace.first_text.get_or_insert_with(|| start.elapsed());
               }
               sender.send_replace(text.to_string());
            })
            .await;
         trace.finish(&result);
         with_last(|last| match result {
            Ok(timing) => last.timing = Some(timing.to_string()),
            Err(error) => last.error = Some(error),
         });
      });
      let entry = Entry {
         text: Arc::new(tokio::sync::Mutex::new(receiver)),
         task: task.abort_handle(),
      };
      requests(|table| {
         for (_, old) in table.drain() {
            old.task.abort();
         }
         table.insert(id, entry);
      });
      Some(id)
   })
}

/// `(hiss-next id)`: a future of the ghost text so far, or of `#f` at the end.
pub fn next(id: usize) -> FFIValue {
   let text = requests(|table| table.get(&id).map(|entry| Arc::clone(&entry.text)));
   let future = async move {
      let Some(text) = text else {
         return FFIValue::BoolV(false);
      };
      let mut receiver = text.lock().await;
      match receiver.changed().await {
         Ok(()) => FFIValue::StringV(receiver.borrow_and_update().as_str().into()),
         Err(_) => {
            requests(|table| table.remove(&id));
            FFIValue::BoolV(false)
         }
      }
   };
   let future: FfiFuture<RResult<FFIValue, RBoxError>> = Unwinding(Box::pin(future)).into_ffi();
   future.into()
}

/// `(hiss-cancel! id)`: aborts the request; a pending `hiss-next` gives `#f`.
pub fn cancel(id: usize) -> Result<bool, String> {
   guarded("hiss-cancel!", || {
      requests(|table| table.remove(&id))
         .map(|entry| entry.task.abort())
         .is_some()
   })
}

/// `(hiss-accepted! chars)`: notes in the log that a ghost was taken.
pub fn accepted(chars: usize) -> Result<bool, String> {
   guarded("hiss-accepted!", || {
      log::write(&format!("accept chars={chars}"));
      true
   })
}

/// `(hiss-log! on)`: turns the log on or off.
pub fn set_log(on: bool) -> Result<bool, String> {
   guarded("hiss-log!", || {
      log::set_on(on);
      on
   })
}

/// What the log gets about one request: sizes and times, never text. A
/// trace dropped before `finish` belongs to a request that was cancelled,
/// either by the editor or by a newer request.
struct Trace {
   id: usize,
   start: Instant,
   /// The chars of context sent before and after the cursor.
   context: (usize, usize),
   first_text: Option<Duration>,
   done: bool,
}

impl Trace {
   fn new(id: usize, prefix: &str, suffix: &str, settings: &Settings) -> Trace {
      let chars = |text: &str| text.chars().count();
      Trace {
         id,
         start: Instant::now(),
         context: (
            chars(clean::tail_lines(prefix, settings.prefix_chars)),
            chars(clean::head_lines(suffix, settings.suffix_chars)),
         ),
         first_text: None,
         done: false,
      }
   }

   fn finish(&mut self, result: &Result<Timing, String>) {
      self.done = true;
      let (before, after) = self.context;
      log::write(&match result {
         Ok(timing) => format!(
            "#{} {} {} ctx={before}+{after}",
            self.id,
            if timing.chars > 0 { "done" } else { "empty" },
            log::timing_fields(timing),
         ),
         Err(error) => format!(
            "#{} error after={} ctx={before}+{after} {}",
            self.id,
            self.start.elapsed().as_millis(),
            log::word(error),
         ),
      });
   }
}

impl Drop for Trace {
   fn drop(&mut self) {
      if !self.done {
         let (before, after) = self.context;
         log::write(&format!(
            "#{} cancelled after={} ttft={} ctx={before}+{after}",
            self.id,
            self.start.elapsed().as_millis(),
            log::ms_or_dash(self.first_text),
         ));
      }
   }
}

/// `(hiss-error)`: the last error, once, or `#f`.
pub fn error() -> Result<Option<String>, String> {
   guarded("hiss-error", || with_last(|last| last.error.take()))
}

/// `(hiss-timing)`: the timing of the last finished request, or `#f`.
pub fn timing() -> Result<Option<String>, String> {
   guarded("hiss-timing", || with_last(|last| last.timing.clone()))
}

/// A future whose polls cannot unwind into Helix: a panic ends it with `#f`.
struct Unwinding(Pin<Box<dyn Future<Output = FFIValue> + Send>>);

impl Future for Unwinding {
   type Output = RResult<FFIValue, RBoxError>;

   fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
      let inner = &mut self.0;
      match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| inner.as_mut().poll(cx))) {
         Ok(Poll::Ready(value)) => Poll::Ready(RResult::ROk(value)),
         Ok(Poll::Pending) => Poll::Pending,
         Err(_) => {
            with_last(|last| last.error = Some("hiss: hiss-next panicked".into()));
            Poll::Ready(RResult::ROk(FFIValue::BoolV(false)))
         }
      }
   }
}

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn last_value_survives_a_closed_sender() {
      RUNTIME.block_on(async {
         let (sender, mut receiver) = watch::channel(String::new());
         sender.send_replace("a".into());
         sender.send_replace("ab".into());
         drop(sender);
         assert!(receiver.changed().await.is_ok());
         assert_eq!(*receiver.borrow_and_update(), "ab");
         assert!(receiver.changed().await.is_err());
      });
   }

   #[test]
   fn unknown_id_ends_at_once() {
      let polled = RUNTIME.block_on(Unwinding(Box::pin(async { FFIValue::BoolV(false) })));
      assert!(matches!(polled, RResult::ROk(FFIValue::BoolV(false))));
      assert!(matches!(cancel(usize::MAX), Ok(false)));
   }

   #[test]
   fn a_panicking_poll_gives_false() {
      let polled = RUNTIME.block_on(Unwinding(Box::pin(async {
         panic!("boom");
         #[allow(unreachable_code)]
         FFIValue::BoolV(true)
      })));
      assert!(matches!(polled, RResult::ROk(FFIValue::BoolV(false))));
   }
}
