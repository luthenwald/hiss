//! Shaping text on the way in (context cuts) and on the way out (cleaning).

const CURSOR: &str = "<CURSOR/>";
const FENCE: &str = "```";

/// Cleans the reply so far for display (spec 04/08). It runs on the whole
/// text each time, so it may be called on partial replies.
pub fn clean(raw: &str, suffix: &str) -> String {
   let text = raw.replace(CURSOR, "");
   let text = strip_fence(&text);
   let text = &text[..text.len() - suffix_overlap(text, suffix)];
   if text.trim().is_empty() {
      String::new()
   } else {
      text.to_string()
   }
}

/// Drops a code fence the model added anyway. A reply that opens with a fence
/// shows nothing until the fence line is complete, and stops at the closing
/// fence. A fence later in the reply is left alone, since it may be content.
fn strip_fence(text: &str) -> &str {
   let start = text.trim_start();
   if !start.starts_with(FENCE) {
      return text;
   }
   let Some(newline) = start.find('\n') else {
      return "";
   };
   let body = &start[newline + 1..];
   if body.starts_with(FENCE) {
      return "";
   }
   match body.find("\n```") {
      Some(end) => &body[..end],
      None => body,
   }
}

/// Length in bytes of the longest end of `text` that the suffix starts with,
/// when cutting it is safe: three characters or more, or nothing but
/// punctuation and whitespace. A one-letter overlap is often a coincidence.
fn suffix_overlap(text: &str, suffix: &str) -> usize {
   suffix
      .char_indices()
      .map(|(i, c)| i + c.len_utf8())
      .take_while(|&end| end <= text.len())
      .filter(|&end| text.ends_with(&suffix[..end]))
      .filter(|&end| {
         let overlap = &suffix[..end];
         overlap.chars().count() >= 3 || !overlap.chars().any(char::is_alphanumeric)
      })
      .last()
      .unwrap_or(0)
}

/// The end of `prefix`, at most `max_chars` long, starting at a line start
/// unless the whole prefix fits.
pub fn tail_lines(prefix: &str, max_chars: usize) -> &str {
   let count = prefix.chars().count();
   if count <= max_chars {
      return prefix;
   }
   let cut = prefix
      .char_indices()
      .nth(count - max_chars)
      .map_or(prefix.len(), |(i, _)| i);
   let rest = &prefix[cut..];
   if prefix[..cut].ends_with('\n') {
      return rest;
   }
   match rest.find('\n') {
      Some(newline) => &rest[newline + 1..],
      None => rest,
   }
}

/// The start of `suffix`, at most `max_chars` long, ending at a line end
/// unless the whole suffix fits.
pub fn head_lines(suffix: &str, max_chars: usize) -> &str {
   let Some((cut, _)) = suffix.char_indices().nth(max_chars) else {
      return suffix;
   };
   let head = &suffix[..cut];
   match head.rfind('\n') {
      Some(newline) => &head[..newline + 1],
      None => head,
   }
}

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn plain_reply_is_kept() {
      assert_eq!(clean("x + 1", ";\n}"), "x + 1");
   }

   #[test]
   fn fence_is_stripped_while_streaming_and_at_the_end() {
      assert_eq!(clean("```", ""), "");
      assert_eq!(clean("```rust", ""), "");
      assert_eq!(clean("```rust\nlet a", ""), "let a");
      assert_eq!(clean("```rust\nlet a = 1;\n```", ""), "let a = 1;");
      assert_eq!(clean("```\n```", ""), "");
   }

   #[test]
   fn fence_inside_the_reply_is_content() {
      assert_eq!(clean("text\n```rust", ""), "text\n```rust");
   }

   #[test]
   fn cursor_tag_is_removed() {
      assert_eq!(clean("a<CURSOR/>b", ""), "ab");
   }

   #[test]
   fn closing_punctuation_overlap_is_cut() {
      assert_eq!(clean("a, b)", ")"), "a, b");
      assert_eq!(clean("x = 1;\n}", "\n}\n"), "x = 1;");
   }

   #[test]
   fn long_overlap_is_cut() {
      assert_eq!(clean("foo(bar)", "bar)\n"), "foo(");
   }

   #[test]
   fn short_word_overlap_is_kept() {
      assert_eq!(clean("a + b", "b;"), "a + b");
   }

   #[test]
   fn whitespace_only_is_dropped() {
      assert_eq!(clean("  \n\t", ""), "");
   }

   #[test]
   fn reply_equal_to_suffix_start_vanishes() {
      assert_eq!(clean(")", ")"), "");
   }

   #[test]
   fn tail_lines_cuts_at_a_line_start() {
      assert_eq!(tail_lines("one\ntwo\nthree", 100), "one\ntwo\nthree");
      assert_eq!(tail_lines("one\ntwo\nthree", 8), "three");
      assert_eq!(tail_lines("one\ntwo\nthree", 9), "two\nthree");
      assert_eq!(tail_lines("one\ntwo\nthree", 12), "two\nthree");
      assert_eq!(tail_lines("one\ntwo\nthree", 13), "one\ntwo\nthree");
      assert_eq!(tail_lines("ééé\nü", 3), "ü");
   }

   #[test]
   fn head_lines_cuts_at_a_line_end() {
      assert_eq!(head_lines("one\ntwo\nthree", 100), "one\ntwo\nthree");
      assert_eq!(head_lines("one\ntwo\nthree", 9), "one\ntwo\n");
      assert_eq!(head_lines("one\ntwo\nthree", 8), "one\ntwo\n");
      assert_eq!(head_lines("one\ntwo\nthree", 7), "one\n");
      assert_eq!(head_lines("abcdef", 3), "abc");
   }
}
