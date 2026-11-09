//! libhiss: the native half of hiss.
//!
//! Loaded by Steel inside Helix. It keeps one HTTP/2 connection to the
//! Anthropic API warm, streams completions, and cancels them on request.
//! The design is in `.claude/spec.md` (03/08 and 04/08).
//!
//! Two rules hold for every exported function: a panic never crosses the FFI
//! boundary (each body runs under `catch_unwind`), and nothing blocks Helix's
//! main thread. Futures handed to Steel are polled on Helix's runtime, which
//! is not ours, so they only wait on channels; the work itself runs on
//! libhiss's own runtime.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::LazyLock;
use std::time::Duration;

use abi_stable::std_types::{RBoxError, RResult};
use steel::declare_module;
use steel::steel_vm::ffi::{FFIModule, FFIValue, FfiFuture, FfiFutureExt, RegisterFFIFn};

/// libhiss's own runtime, started on first use.
static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
   tokio::runtime::Builder::new_multi_thread()
      .worker_threads(2)
      .thread_name("hiss")
      .enable_all()
      .build()
      .expect("hiss: cannot start the runtime")
});

/// Runs `body`, turning a panic into an error value for Steel.
fn guarded<T>(name: &str, body: impl FnOnce() -> T) -> Result<T, String> {
   catch_unwind(AssertUnwindSafe(body)).map_err(|_| format!("hiss: {name} panicked"))
}

/// `(hiss-version)`: the version of libhiss, as a string.
fn version() -> Result<String, String> {
   guarded("hiss-version", || env!("CARGO_PKG_VERSION").to_string())
}

/// `(hiss-ping ms)`: a future that resolves to `"pong"` after `ms` milliseconds,
/// timed on libhiss's runtime. It proves the async path end to end: Steel awaits
/// it through `helix-await-callback`, and the callback runs on the main thread.
fn ping(ms: usize) -> FFIValue {
   let spawned = guarded("hiss-ping", || {
      let (tx, rx) = tokio::sync::oneshot::channel();
      RUNTIME.spawn(async move {
         tokio::time::sleep(Duration::from_millis(ms as u64)).await;
         let _ = tx.send(());
      });
      rx
   });
   let future: FfiFuture<RResult<FFIValue, RBoxError>> = async move {
      match spawned {
         Ok(rx) => match rx.await {
            Ok(()) => RResult::ROk(FFIValue::StringV("pong".into())),
            Err(_) => RResult::ROk(FFIValue::BoolV(false)),
         },
         Err(message) => RResult::RErr(RBoxError::from_box(message.into())),
      }
   }
   .into_ffi();
   future.into()
}

fn create_module() -> FFIModule {
   let mut module = FFIModule::new("hiss/native");
   module
      .register_fn("hiss-version", version)
      .register_fn("hiss-ping", ping);
   module
}

declare_module!(create_module);
