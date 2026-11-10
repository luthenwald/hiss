//! Recent suggestions, keyed on the text around the cursor, so that undoing
//! an edit or retyping what was deleted shows the suggestion again at once,
//! without asking (spec 06/08). Held in memory only.

use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hash, Hasher};

/// How many suggestions are kept. Each is a hash and a few lines of text.
pub const CAPACITY: usize = 64;

/// A suggestion: how many chars before the cursor it starts, and its text.
pub type Ghost = (usize, String);

/// The text a request sends (the window around the cursor, after the cut
/// at line boundaries), its language and its path, hashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key(u64);

impl Key {
   pub fn new(prefix: &str, suffix: &str, language: &str, path: &str) -> Key {
      let mut hasher = DefaultHasher::new();
      (prefix, suffix, language, path).hash(&mut hasher);
      Key(hasher.finish())
   }
}

/// The most recently used suggestion first.
#[derive(Debug)]
pub struct Cache {
   entries: VecDeque<(Key, Ghost)>,
   capacity: usize,
}

impl Cache {
   pub fn new(capacity: usize) -> Cache {
      Cache {
         entries: VecDeque::with_capacity(capacity + 1),
         capacity,
      }
   }

   /// The suggestion for `key`, which becomes the most recent one.
   pub fn get(&mut self, key: Key) -> Option<Ghost> {
      let at = self.entries.iter().position(|(k, _)| *k == key)?;
      let entry = self.entries.remove(at)?;
      self.entries.push_front(entry.clone());
      Some(entry.1)
   }

   /// Keeps `ghost` for `key`, in place of what was kept for it before, and
   /// lets the least recently used suggestion go when the cache is full.
   pub fn put(&mut self, key: Key, ghost: Ghost) {
      self.entries.retain(|(k, _)| *k != key);
      self.entries.push_front((key, ghost));
      self.entries.truncate(self.capacity);
   }
}

#[cfg(test)]
mod tests {
   use super::*;

   fn key(prefix: &str) -> Key {
      Key::new(prefix, ";\n", "rust", "a.rs")
   }

   #[test]
   fn hit_and_miss() {
      let mut cache = Cache::new(4);
      cache.put(key("let x = "), (0, "1".into()));
      assert_eq!(cache.get(key("let x = ")), Some((0, "1".into())));
      assert_eq!(cache.get(key("let y = ")), None);
   }

   #[test]
   fn every_part_of_the_key_counts() {
      let base = Key::new("a", "b", "rust", "a.rs");
      assert_ne!(base, Key::new("a", "c", "rust", "a.rs"));
      assert_ne!(base, Key::new("a", "b", "python", "a.rs"));
      assert_ne!(base, Key::new("a", "b", "rust", "b.rs"));
      // The parts are hashed apart, so moving text across the cursor misses.
      assert_ne!(
         Key::new("ab", "", "rust", "a.rs"),
         Key::new("a", "b", "rust", "a.rs")
      );
   }

   #[test]
   fn the_least_recently_used_goes_first() {
      let mut cache = Cache::new(2);
      cache.put(key("a"), (0, "1".into()));
      cache.put(key("b"), (0, "2".into()));
      assert!(cache.get(key("a")).is_some());
      cache.put(key("c"), (0, "3".into()));
      assert!(cache.get(key("b")).is_none());
      assert!(cache.get(key("a")).is_some());
      assert!(cache.get(key("c")).is_some());
   }

   #[test]
   fn a_new_suggestion_replaces_the_old() {
      let mut cache = Cache::new(2);
      cache.put(key("a"), (0, "1".into()));
      cache.put(key("a"), (2, "2".into()));
      cache.put(key("b"), (0, "3".into()));
      assert_eq!(cache.get(key("a")), Some((2, "2".into())));
      assert!(cache.get(key("b")).is_some());
   }
}
