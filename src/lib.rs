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

pub mod api;
pub mod cache;
pub mod clean;
pub mod config;
pub mod exclude;
mod ffi;
mod log;
pub mod quality;
pub mod sse;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::LazyLock;

use steel::declare_module;
use steel::steel_vm::ffi::{FFIModule, RegisterFFIFn};

/// libhiss's own runtime, started on first use.
pub static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
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

fn create_module() -> FFIModule {
   let _ = guarded("config", || log::set_on(config::config().log));
   ffi::warm_up();
   let mut module = FFIModule::new("hiss/native");
   module
      .register_fn("hiss-version", version)
      .register_fn("hiss-start!", ffi::start)
      .register_fn("hiss-next", ffi::next)
      .register_fn("hiss-cancel!", ffi::cancel)
      .register_fn("hiss-cached", ffi::cached_ghost)
      .register_fn("hiss-resume!", ffi::resume)
      .register_fn("hiss-warm!", ffi::warm_again)
      .register_fn("hiss-accepted!", ffi::accepted)
      .register_fn("hiss-config", ffi::config_value)
      .register_fn("hiss-config-problem", ffi::config_problem)
      .register_fn("hiss-error", ffi::error)
      .register_fn("hiss-timing", ffi::timing);
   module
}

declare_module!(create_module);
