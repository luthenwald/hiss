//! libhiss: the native half of hiss.
//!
//! Loaded by Steel inside Helix. It keeps one HTTP/2 connection to the
//! Anthropic API warm, streams completions, and cancels them on request.
//! The design is in `.claude/spec.md` (03/08 and 04/08); nothing is built yet.
