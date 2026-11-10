//! The settings file: `~/.config/hiss/config.toml`, or
//! `$XDG_CONFIG_HOME/hiss/config.toml` when that is set. Read once, when
//! libhiss loads; a missing file means the defaults.
//!
//! The file is a flat TOML subset: `key = value` lines, `#` comments,
//! booleans, integers and double-quoted strings. A line that cannot be read
//! keeps that setting's default and is reported once in the status line.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::LazyLock;

use crate::api::Settings;

/// Every setting, with its default.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
   /// Suggestions are on when the editor starts.
   pub enabled: bool,
   /// Milliseconds to wait after an edit before asking.
   pub debounce: u64,
   pub log: bool,
   pub request: Settings,
}

impl Default for Config {
   fn default() -> Self {
      Config {
         enabled: true,
         debounce: 250,
         log: true,
         request: Settings::default(),
      }
   }
}

/// The settings, and what was wrong with the file, if anything.
pub static CONFIG: LazyLock<(Config, Option<String>)> = LazyLock::new(|| {
   let Some(path) = path(
      std::env::var_os("XDG_CONFIG_HOME"),
      std::env::var_os("HOME"),
   ) else {
      return (Config::default(), None);
   };
   match std::fs::read_to_string(&path) {
      Ok(text) => parse(&text),
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => (Config::default(), None),
      Err(error) => (
         Config::default(),
         Some(format!("hiss: cannot read config.toml: {error}")),
      ),
   }
});

pub fn config() -> &'static Config {
   &CONFIG.0
}

fn path(config: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
   let absolute = |path: PathBuf| path.is_absolute().then_some(path);
   config
      .map(PathBuf::from)
      .and_then(absolute)
      .or_else(|| {
         home
            .map(|home| PathBuf::from(home).join(".config"))
            .and_then(absolute)
      })
      .map(|dir| dir.join("hiss").join("config.toml"))
}

#[derive(Debug, Clone, PartialEq)]
enum Value {
   Bool(bool),
   Int(u64),
   Text(String),
}

fn value(text: &str) -> Option<Value> {
   match text {
      "true" => Some(Value::Bool(true)),
      "false" => Some(Value::Bool(false)),
      _ if text.starts_with('"') => {
         let inner = text.strip_prefix('"')?.strip_suffix('"')?;
         (!inner.contains(['"', '\\'])).then(|| Value::Text(inner.into()))
      }
      _ => text.replace('_', "").parse().ok().map(Value::Int),
   }
}

/// The line without its comment. A `#` inside a string is not a comment.
fn uncomment(line: &str) -> &str {
   let mut quoted = false;
   for (i, c) in line.char_indices() {
      match c {
         '"' => quoted = !quoted,
         '#' if !quoted => return &line[..i],
         _ => {}
      }
   }
   line
}

/// Reads the file, keeping the default for every line it cannot use. The
/// message names the first such line.
pub fn parse(text: &str) -> (Config, Option<String>) {
   let mut config = Config::default();
   let mut problems = Vec::new();
   for (number, line) in text.lines().enumerate() {
      let line = uncomment(line).trim();
      if line.is_empty() {
         continue;
      }
      let problem = match line.split_once('=') {
         None => Some("expected `key = value`".to_string()),
         Some((key, raw)) => match value(raw.trim()) {
            None => Some(format!("cannot read the value of `{}`", key.trim())),
            Some(value) => set(&mut config, key.trim(), value).err(),
         },
      };
      if let Some(problem) = problem {
         problems.push(format!("line {}: {problem}", number + 1));
      }
   }
   let message = match problems.as_slice() {
      [] => None,
      [one] => Some(format!("hiss: config.toml {one}")),
      [first, rest @ ..] => Some(format!(
         "hiss: config.toml {first} (and {} more)",
         rest.len()
      )),
   };
   (config, message)
}

fn set(config: &mut Config, key: &str, value: Value) -> Result<(), String> {
   let request = &mut config.request;
   let int = |value: &Value, low: u64, high: u64| match value {
      Value::Int(n) if (low..=high).contains(n) => Ok(*n),
      _ => Err(format!("`{key}` takes a number from {low} to {high}")),
   };
   let bool = |value: &Value| match value {
      Value::Bool(b) => Ok(*b),
      _ => Err(format!("`{key}` takes true or false")),
   };
   match key {
      "enabled" => config.enabled = bool(&value)?,
      "debounce" => config.debounce = int(&value, 0, 5_000)?,
      "log" => config.log = bool(&value)?,
      "retries" => request.retries = int(&value, 0, 5)? as u32,
      "prose_retries" => request.prose_retries = int(&value, 0, 5)? as u32,
      "max_tokens" => request.max_tokens = int(&value, 8, 1_024)? as u32,
      "context_before" => request.prefix_chars = int(&value, 0, 100_000)? as usize,
      "context_after" => request.suffix_chars = int(&value, 0, 100_000)? as usize,
      "model" => match value {
         Value::Text(model) if !model.trim().is_empty() => request.model = model,
         _ => return Err("`model` takes a model name in quotes".into()),
      },
      _ => return Err(format!("unknown setting `{key}`")),
   }
   Ok(())
}

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn the_example_reads_without_problems() {
      let (config, problem) = parse(include_str!("../examples/config.toml"));
      assert_eq!(problem, None);
      assert_eq!(config, Config::default());
   }

   #[test]
   fn values_are_read() {
      let (config, problem) = parse(
         "# comment\nlog = false # off\ndebounce = 1_000\nmodel = \"claude-sonnet-5-5\"\nretries = 0\n",
      );
      assert_eq!(problem, None);
      assert!(!config.log);
      assert_eq!(config.debounce, 1_000);
      assert_eq!(config.request.model, "claude-sonnet-5-5");
      assert_eq!(config.request.retries, 0);
   }

   #[test]
   fn a_bad_line_keeps_the_default_and_is_named() {
      let (config, problem) = parse("debounce = fast\nlog = false\nretrys = 2\n");
      assert_eq!(config.debounce, 250);
      assert!(!config.log);
      assert_eq!(
         problem.as_deref(),
         Some("hiss: config.toml line 1: cannot read the value of `debounce` (and 1 more)")
      );
      let (_, problem) = parse("retries = 9");
      assert_eq!(
         problem.as_deref(),
         Some("hiss: config.toml line 1: `retries` takes a number from 0 to 5")
      );
   }

   #[test]
   fn path_prefers_an_absolute_config_home() {
      let some = |s: &str| Some(OsString::from(s));
      assert_eq!(
         path(some("/c"), some("/h")),
         Some(PathBuf::from("/c/hiss/config.toml"))
      );
      assert_eq!(
         path(None, some("/h")),
         Some(PathBuf::from("/h/.config/hiss/config.toml"))
      );
   }
}
