//! The functions Steel sees (spec 03/08).
//!
//! `hiss-start!` starts a completion and returns its id at once.
//! `hiss-next` returns a future of the whole cleaned ghost text so far, or of
//! `#f` once the stream has ended, failed or been cancelled. `hiss-cancel!`
//! aborts a request. Only one request runs at a time: starting one aborts the
//! others, since only the newest can still be shown. `hiss-cached` gives a
//! suggestion already made for the same text, without asking.
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
use crate::cache::{self, Cache, Ghost, Key};
use crate::{RUNTIME, clean, config, guarded, log};

struct Entry {
   text: Arc<tokio::sync::Mutex<watch::Receiver<(usize, String)>>>,
   task: tokio::task::AbortHandle,
}

static CLIENT: LazyLock<Result<Arc<Client>, String>> =
   LazyLock::new(|| Client::from_env().map(Arc::new));
static REQUESTS: LazyLock<Mutex<HashMap<usize, Entry>>> = LazyLock::new(Default::default);
static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(Cache::new(cache::CAPACITY)));

fn cached<T>(f: impl FnOnce(&mut Cache) -> T) -> T {
   f(&mut CACHE.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The window a request sends: the text around the cursor, cut at lines.
fn window<'a>(prefix: &'a str, suffix: &'a str, settings: &Settings) -> (&'a str, &'a str) {
   (
      clean::tail_lines(prefix, settings.prefix_chars),
      clean::head_lines(suffix, settings.suffix_chars),
   )
}

fn key(prefix: &str, suffix: &str, language: &str, path: &str, settings: &Settings) -> Key {
   let (before, after) = window(prefix, suffix, settings);
   Key::new(before, after, language, path)
}

/// A ghost as Steel sees it: `(back text)`.
fn ghost_value((back, text): &Ghost) -> FFIValue {
   FFIValue::Vector(
      vec![
         FFIValue::IntV(*back as isize),
         FFIValue::StringV(text.as_str().into()),
      ]
      .into(),
   )
}

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
      let (sender, receiver) = watch::channel((0, String::new()));
      let settings = config::config().request.clone();
      let mut trace = Trace::new(id, &prefix, &suffix, &settings);
      let key = key(&prefix, &suffix, &language, &path, &settings);
      let request = Request {
         prefix,
         suffix,
         language,
         path,
      };
      let task = RUNTIME.spawn(async move {
         let start = trace.start;
         let mut last: Ghost = (0, String::new());
         let result = client
            .complete(&request, &settings, |text, back| {
               if !text.is_empty() {
                  trace.first_text.get_or_insert_with(|| start.elapsed());
               }
               last = (back, text.to_string());
               sender.send_replace(last.clone());
            })
            .await;
         trace.finish(&result);
         // Only a finished suggestion is kept; an empty one is asked for
         // again next time, since the model may answer then.
         if result.is_ok() && !last.1.is_empty() {
            cached(|cache| cache.put(key, last));
         }
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

/// `(hiss-next id)`: a future of `(back text)`, the ghost text so far and how
/// many chars before the cursor it starts, or of `#f` at the end.
pub fn next(id: usize) -> FFIValue {
   let text = requests(|table| table.get(&id).map(|entry| Arc::clone(&entry.text)));
   let future = async move {
      let Some(text) = text else {
         return FFIValue::BoolV(false);
      };
      let mut receiver = text.lock().await;
      match receiver.changed().await {
         Ok(()) => ghost_value(&receiver.borrow_and_update()),
         Err(_) => {
            requests(|table| table.remove(&id));
            FFIValue::BoolV(false)
         }
      }
   };
   let future: FfiFuture<RResult<FFIValue, RBoxError>> = Unwinding(Box::pin(future)).into_ffi();
   future.into()
}

/// `(hiss-cached prefix suffix language path)`: the suggestion a finished
/// request made for the same text, as `(back text)`, or `#f`.
pub fn cached_ghost(prefix: String, suffix: String, language: String, path: String) -> FFIValue {
   guarded("hiss-cached", || {
      let settings = &config::config().request;
      let ghost = cached(|cache| cache.get(key(&prefix, &suffix, &language, &path, settings)));
      match ghost {
         Some(ghost) => {
            let (before, after) = window(&prefix, &suffix, settings);
            log::write(&format!(
               "hit chars={} ctx={}+{}",
               ghost.1.chars().count(),
               before.chars().count(),
               after.chars().count(),
            ));
            ghost_value(&ghost)
         }
         None => FFIValue::BoolV(false),
      }
   })
   .unwrap_or(FFIValue::BoolV(false))
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

/// `(hiss-config name)`: a setting `hiss.scm` needs itself, from
/// `config.toml`: `"enabled"` (a boolean), `"debounce"` (milliseconds), or
/// `"context_before"` and `"context_after"` (chars). Any other name gives
/// `#f`.
pub fn config_value(name: String) -> FFIValue {
   let config = config::config();
   let int = |n: usize| FFIValue::IntV(n as isize);
   guarded("hiss-config", || match name.as_str() {
      "enabled" => FFIValue::BoolV(config.enabled),
      "debounce" => int(config.debounce as usize),
      "context_before" => int(config.request.prefix_chars),
      "context_after" => int(config.request.suffix_chars),
      _ => FFIValue::BoolV(false),
   })
   .unwrap_or(FFIValue::BoolV(false))
}

/// `(hiss-config-problem)`: what was wrong with `config.toml`, or `#f`.
pub fn config_problem() -> Result<Option<String>, String> {
   guarded("hiss-config-problem", || config::CONFIG.1.clone())
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
         context: {
            let (before, after) = window(prefix, suffix, settings);
            (chars(before), chars(after))
         },
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
         let (sender, mut receiver) = watch::channel((0, String::new()));
         sender.send_replace((0, "a".into()));
         sender.send_replace((2, "ab".into()));
         drop(sender);
         assert!(receiver.changed().await.is_ok());
         assert_eq!(*receiver.borrow_and_update(), (2, "ab".into()));
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
