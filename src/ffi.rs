//! The functions Steel sees (spec 03/08).
//!
//! `hiss-start!` starts a completion and returns its id at once.
//! `hiss-next` returns a future of the whole cleaned ghost text so far, or of
//! `#f` once the stream has ended, failed or been cancelled. `hiss-cancel!`
//! aborts a request. Only one request runs at a time: starting one aborts the
//! others, since only the newest can still be shown. `hiss-cached` gives a
//! suggestion already made for the same text, without asking.
//!
//! A refusal that every request would meet (no key, a key or model the API
//! does not take) halts the automatic requests: it is shown once, and
//! `hiss-start!` asks nothing more until a request the user made, or
//! `hiss-resume!`.
//!
//! Futures handed to Steel are polled on Helix's runtime, so they only wait on
//! channels, and each poll runs under `catch_unwind`.

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use abi_stable::std_types::{RBoxError, RResult};
use steel::steel_vm::ffi::{FFIValue, FfiFuture, FfiFutureExt};
use tokio::sync::watch;

use crate::api::{Client, Failure, Request, Timing};
use crate::cache::{self, Cache, Ghost, Key};
use crate::{RUNTIME, config, exclude, guarded, log};

struct Entry {
   text: Arc<tokio::sync::Mutex<watch::Receiver<(usize, String)>>>,
   task: tokio::task::AbortHandle,
}

static CLIENT: LazyLock<Result<Arc<Client>, String>> =
   LazyLock::new(|| Client::from_env().map(Arc::new));
static REQUESTS: LazyLock<Mutex<HashMap<usize, Entry>>> = LazyLock::new(Default::default);
static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(Cache::new(cache::CAPACITY)));
static HALTED: AtomicBool = AtomicBool::new(false);

/// When libhiss last turned to the API (a request or a warm-up started).
static CONTACT: Mutex<Option<Instant>> = Mutex::new(None);

/// How long the API may be left alone before `hiss-warm!` warms again. The
/// pool drops an idle connection after ten minutes, and the far end may do
/// so sooner; a warm-up over a live connection costs one round trip.
const QUIET: Duration = Duration::from_secs(60);

fn contact() -> Option<Instant> {
   CONTACT
      .lock()
      .unwrap_or_else(|e| e.into_inner())
      .replace(Instant::now())
}

/// Keeps `failure` for the status line, and halts the automatic requests
/// when it would come back with every one of them.
fn failed(failure: Failure) {
   if failure.lasting {
      HALTED.store(true, Ordering::Relaxed);
   }
   with_last(|last| last.error = Some(failure.message));
}

fn cached<T>(f: impl FnOnce(&mut Cache) -> T) -> T {
   f(&mut CACHE.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Whether the buffer at `path` in `language` must not be sent (`exclude`).
fn excluded(path: &str, language: &str) -> bool {
   let home = std::env::var("HOME").ok();
   exclude::excluded(path, language, &config::config().exclude, home.as_deref())
}

/// The request the editor's text makes, cut to the window that is sent.
fn request(prefix: String, suffix: String, language: String, path: &str) -> Request {
   let request = Request {
      prefix,
      suffix,
      language,
      path: shown(path),
   };
   request.cut(&config::config().request)
}

fn key(request: &Request) -> Key {
   Key::new(
      &request.prefix,
      &request.suffix,
      &request.language,
      &request.path,
   )
}

fn chars(text: &str) -> usize {
   text.chars().count()
}

/// The path a request names: relative to the working directory (Helix's,
/// usually the project) when the file is under it, else the file name alone,
/// so that no home folder or user name leaves the machine.
fn shown_path(path: &str, cwd: Option<&Path>) -> String {
   let path = Path::new(path);
   cwd.and_then(|cwd| path.strip_prefix(cwd).ok())
      .filter(|relative| !relative.as_os_str().is_empty())
      .or_else(|| path.file_name().map(Path::new))
      .map_or(String::new(), |shown| shown.to_string_lossy().into_owned())
}

fn shown(path: &str) -> String {
   shown_path(path, std::env::current_dir().ok().as_deref())
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
         contact();
         let client = Arc::clone(client);
         RUNTIME.spawn(async move {
            match client.warm(&config::config().request.model).await {
               Ok(took) => log::write(&format!("warm after={}", took.as_millis())),
               Err(failure) => {
                  log::write(&format!("warm error {}", log::word(&failure.message)));
                  failed(failure);
               }
            }
         });
      }
   });
}

/// `(hiss-warm!)`: opens the connection again when the API has been left
/// alone for a while, so that the next request does not pay for it. Says
/// whether it did.
pub fn warm_again() -> Result<bool, String> {
   guarded("hiss-warm!", || {
      let quiet = CONTACT
         .lock()
         .unwrap_or_else(|e| e.into_inner())
         .is_none_or(|last| last.elapsed() >= QUIET);
      let warm = quiet && !HALTED.load(Ordering::Relaxed);
      if warm {
         warm_up();
      }
      warm
   })
}

/// `(hiss-start! prefix suffix language path auto)`: the new request's id,
/// or `#f` when there is no client (see `hiss-error`) or, for an automatic
/// request (`auto`), while a lasting refusal halts them.
pub fn start(
   prefix: String,
   suffix: String,
   language: String,
   path: String,
   auto: bool,
) -> Result<Option<usize>, String> {
   guarded("hiss-start!", || {
      if auto && HALTED.load(Ordering::Relaxed) {
         return None;
      }
      if excluded(&path, &language) {
         if !auto {
            with_last(|last| last.error = Some("hiss: this file is never sent".into()));
         }
         return None;
      }
      HALTED.store(false, Ordering::Relaxed);
      let client = match CLIENT.as_ref() {
         Ok(client) => Arc::clone(client),
         Err(error) => {
            failed(Failure {
               message: error.clone(),
               transient: false,
               lasting: true,
            });
            return None;
         }
      };
      contact();
      let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
      let (sender, receiver) = watch::channel((0, String::new()));
      let settings = config::config().request.clone();
      let request = request(prefix, suffix, language, &path);
      let mut trace = Trace::new(id, &request);
      let key = key(&request);
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
         match result {
            Ok(timing) => with_last(|last| last.timing = Some(timing.to_string())),
            Err(failure) => failed(failure),
         }
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
   guarded("hiss-next", || {
      let text = requests(|table| table.get(&id).map(|entry| Arc::clone(&entry.text)));
      steel_future(async move {
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
      })
   })
   .unwrap_or_else(|error| {
      with_last(|last| last.error = Some(error));
      steel_future(async { FFIValue::BoolV(false) })
   })
}

/// A future as Steel takes it, each poll under `catch_unwind`.
fn steel_future(future: impl Future<Output = FFIValue> + Send + 'static) -> FFIValue {
   let future: FfiFuture<RResult<FFIValue, RBoxError>> = Unwinding(Box::pin(future)).into_ffi();
   future.into()
}

/// `(hiss-cached prefix suffix language path)`: the suggestion a finished
/// request made for the same text, as `(back text)`, or `#f`.
pub fn cached_ghost(prefix: String, suffix: String, language: String, path: String) -> FFIValue {
   guarded("hiss-cached", || {
      if excluded(&path, &language) {
         return FFIValue::BoolV(false);
      }
      let request = request(prefix, suffix, language, &path);
      let ghost = cached(|cache| cache.get(key(&request)));
      match ghost {
         Some(ghost) => {
            log::write(&format!(
               "hit chars={} ctx={}+{}",
               chars(&ghost.1),
               chars(&request.prefix),
               chars(&request.suffix),
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

/// `(hiss-resume!)`: lets automatic requests ask again after a lasting
/// refusal (`:hiss-on`).
pub fn resume() -> Result<bool, String> {
   guarded("hiss-resume!", || {
      HALTED.store(false, Ordering::Relaxed);
      true
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
   let int = |n: usize| FFIValue::IntV(n as isize);
   guarded("hiss-config", || {
      let config = config::config();
      match name.as_str() {
         "enabled" => FFIValue::BoolV(config.enabled),
         "debounce" => int(config.debounce as usize),
         "context_before" => int(config.request.prefix_chars),
         "context_after" => int(config.request.suffix_chars),
         _ => FFIValue::BoolV(false),
      }
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
   fn new(id: usize, request: &Request) -> Trace {
      Trace {
         id,
         start: Instant::now(),
         context: (chars(&request.prefix), chars(&request.suffix)),
         first_text: None,
         done: false,
      }
   }

   fn finish(&mut self, result: &Result<Timing, Failure>) {
      self.done = true;
      let (before, after) = self.context;
      log::write(&match result {
         Ok(timing) => format!(
            "#{} {} {} ctx={before}+{after}",
            self.id,
            if timing.chars > 0 { "done" } else { "empty" },
            log::timing_fields(timing),
         ),
         Err(failure) => format!(
            "#{} error after={} ctx={before}+{after} {}",
            self.id,
            self.start.elapsed().as_millis(),
            log::word(&failure.message),
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
   fn paths_are_shown_from_the_working_directory() {
      let cwd = Some(Path::new("/home/someone/project"));
      assert_eq!(
         shown_path("/home/someone/project/src/a.rs", cwd),
         "src/a.rs"
      );
      assert_eq!(shown_path("/home/someone/project/a.rs", cwd), "a.rs");
      assert_eq!(shown_path("/home/someone/notes/b.md", cwd), "b.md");
      assert_eq!(shown_path("/home/someone/notes/b.md", None), "b.md");
      assert_eq!(shown_path("", cwd), "");
      // A buffer named after the directory itself still shows a name.
      assert_eq!(shown_path("/home/someone/project", cwd), "project");
   }

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
