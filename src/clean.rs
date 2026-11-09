//! Shaping text on the way in (context cuts) and on the way out (cleaning).

use crate::quality;

const CURSOR: &str = "<CURSOR/>";
const FENCE: &str = "```";

/// What the cleaning needs to know about a request, worked out once.
#[derive(Debug, Clone)]
pub struct Shape<'a> {
   pub suffix: &'a str,
   pub indent: Indent,
   /// Whether prose is cut: not in prose files, comments or strings.
   pub prose_guard: bool,
}

impl<'a> Shape<'a> {
   pub fn new(prefix: &str, suffix: &'a str, language: &str) -> Shape<'a> {
      Shape {
         suffix,
         indent: Indent {
            aligned: ALIGNED.contains(&language),
            ..indent_of(prefix)
         },
         prose_guard: quality::prose_applies(prefix, language),
      }
   }
}

/// The reply as it may be shown, and whether prose was cut from it, which
/// ends the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cleaned {
   pub text: String,
   pub prose: bool,
}

/// Cleans the reply so far for display (spec 04/08). It runs on the whole
/// text each time, so it may be called on partial replies.
pub fn clean(raw: &str, shape: &Shape) -> Cleaned {
   clean_as(raw, shape, false)
}

/// Cleans the whole reply once the stream has ended, when its last line is
/// complete too and may turn out to be a sentence.
pub fn clean_end(raw: &str, shape: &Shape) -> Cleaned {
   clean_as(raw, shape, true)
}

fn clean_as(raw: &str, shape: &Shape, complete: bool) -> Cleaned {
   let text = raw.replace(CURSOR, "");
   let text = strip_fence(&text);
   let cut = shape
      .prose_guard
      .then(|| quality::prose(text, complete))
      .flatten();
   let text = cut.map_or(text, |at| text[..at].trim_end());
   let text = &text[..text.len() - suffix_overlap(text, shape.suffix)];
   Cleaned {
      text: if text.trim().is_empty() {
         String::new()
      } else {
         follow_indent(text, &shape.indent)
      },
      prose: cut.is_some(),
   }
}

/// How many spaces a tab stands for when a tab file gets spaces.
const TAB_WIDTH: usize = 4;

/// Brings the reply's indentation in line with the file, on lines that have
/// content (a line still streaming keeps its whitespace until text arrives):
/// - at the start of a line, a reply that opens with a line break loses it,
///   since it would leave the cursor's line empty and start one lower;
/// - a first line that repeats the indent the cursor already sits after loses
///   it, since `insert_string` would put it in twice;
/// - later lines use the file's unit, a tab standing for one unit;
/// - in a file indented with spaces, when no line fits the file's width but
///   all fit another (4 in a 3-space file), the depth is rescaled.
///
/// When the unit is unknown, only the first rule applies.
fn follow_indent(text: &str, indent: &Indent) -> String {
   let text = if indent.at_line_start {
      text.trim_start_matches('\n')
   } else {
      text
   };
   let mut lines: Vec<String> = text.split('\n').map(String::from).collect();
   if indent.at_line_start && !indent.line.is_empty() {
      if let Some(rest) = lines[0].strip_prefix(indent.line.as_str()) {
         lines[0] = rest.to_string();
      }
   }
   let Some(unit) = indent.unit else {
      return lines.join("\n");
   };
   let tab = match unit {
      Unit::Tabs => TAB_WIDTH,
      Unit::Spaces(n) => n,
   };
   let columns = |lead: &str| {
      lead
         .chars()
         .map(|c| if c == '\t' { tab } else { 1 })
         .sum::<usize>()
   };
   let widths: Vec<usize> = lines
      .iter()
      .skip(1)
      .filter(|line| !line.trim().is_empty())
      .map(|line| columns(leading(line)))
      .collect();
   let scale = match unit {
      Unit::Spaces(n) if !indent.aligned && widths.iter().any(|w| w % n != 0) => [4, 2, 8]
         .into_iter()
         .find(|&m| m != n && widths.iter().all(|w| w % m == 0))
         .map(|m| (n, m)),
      _ => None,
   };
   for line in lines.iter_mut().skip(1) {
      if line.trim().is_empty() {
         continue;
      }
      let lead = leading(line);
      let width = columns(lead);
      let width = scale.map_or(width, |(n, m)| width / m * n);
      let new_lead = match unit {
         Unit::Tabs => "\t".repeat((width + TAB_WIDTH / 2) / TAB_WIDTH),
         Unit::Spaces(_) => " ".repeat(width),
      };
      if new_lead != lead {
         *line = format!("{new_lead}{}", &line[lead.len()..]);
      }
   }
   lines.join("\n")
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

/// How the file indents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
   Tabs,
   Spaces(usize),
}

impl std::fmt::Display for Unit {
   fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
      match self {
         Unit::Tabs => write!(f, "tabs"),
         Unit::Spaces(1) => write!(f, "1 space"),
         Unit::Spaces(n) => write!(f, "{n} spaces"),
      }
   }
}

/// The indentation around the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Indent {
   /// None when the text before the cursor shows no indentation to go by.
   pub unit: Option<Unit>,
   /// The cursor line's leading whitespace.
   pub line: String,
   /// Only whitespace sits before the cursor on its line.
   pub at_line_start: bool,
   /// Lines align with brackets rather than step by units (Lisps), so their
   /// depth is never rescaled.
   pub aligned: bool,
}

/// Languages whose lines align with brackets rather than step by a unit.
const ALIGNED: [&str; 9] = [
   "scheme",
   "racket",
   "lisp",
   "common-lisp",
   "clojure",
   "fennel",
   "elisp",
   "janet",
   "hy",
];

fn leading(line: &str) -> &str {
   &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// Reads the indentation from the text before the cursor, the cursor's own
/// line included. Tabs win when more lines start with a tab than with a
/// space; otherwise the unit is the most common step between neighbouring
/// lines (the smaller one on a tie), or unknown when nothing steps in.
pub fn indent_of(prefix: &str) -> Indent {
   let last = prefix.rsplit('\n').next().unwrap_or("");
   let lines: Vec<&str> = prefix
      .split('\n')
      .filter(|l| !l.trim().is_empty())
      .chain((last.trim().is_empty() && !last.is_empty()).then_some(last))
      .collect();
   let tabs = lines.iter().filter(|l| l.starts_with('\t')).count();
   let spaces = lines.iter().filter(|l| l.starts_with(' ')).count();
   let unit = if tabs > spaces {
      Some(Unit::Tabs)
   } else {
      let widths: Vec<usize> = lines
         .iter()
         .map(|l| leading(l))
         .filter(|w| !w.contains('\t'))
         .map(str::len)
         .collect();
      let mut steps = [0usize; 9];
      for pair in widths.windows(2) {
         let step = pair[1].saturating_sub(pair[0]);
         if (1..=8).contains(&step) {
            steps[step] += 1;
         }
      }
      (1..=8)
         .rev()
         .max_by_key(|&n| steps[n])
         .filter(|&n| steps[n] > 0)
         .map(Unit::Spaces)
   };
   Indent {
      unit,
      line: leading(last).to_string(),
      at_line_start: last.trim().is_empty(),
      aligned: false,
   }
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

   /// Cleans with the prose guard off and no indentation to follow.
   fn clean(raw: &str, suffix: &str) -> String {
      super::clean(raw, &Shape::new("", suffix, "text")).text
   }

   const USER_REPLY: &str = "let sum: f64 = self.values.iter().sum();\n      Some(sum / self.values.len() as f64)} else if false {}The mean method is broken around the cursor: the `if` block is closed before the `let sum` line.";

   fn follow(text: &str, unit: Unit, line: &str) -> String {
      let indent = Indent {
         unit: Some(unit),
         line: line.into(),
         at_line_start: line.trim().is_empty(),
         aligned: false,
      };
      follow_indent(text, &indent)
   }

   #[test]
   fn a_repeated_first_indent_is_dropped() {
      assert_eq!(
         follow("      let a = 1;", Unit::Spaces(3), "      "),
         "let a = 1;"
      );
      assert_eq!(
         follow("   let a = 1;", Unit::Spaces(3), "      "),
         "   let a = 1;"
      );
   }

   #[test]
   fn a_leading_line_break_at_a_line_start_is_dropped() {
      assert_eq!(
         follow("\n      let a = 1;\n      a", Unit::Spaces(3), "      "),
         "let a = 1;\n      a"
      );
      assert_eq!(follow("\n   a", Unit::Spaces(3), "x {"), "\n   a");
   }

   #[test]
   fn later_lines_take_the_files_unit() {
      assert_eq!(
         follow("a\n\tb\n\t\tc", Unit::Spaces(3), "x"),
         "a\n   b\n      c"
      );
      assert_eq!(
         follow("a\n    b\n        c", Unit::Tabs, "x"),
         "a\n\tb\n\t\tc"
      );
      assert_eq!(follow("a\n\tb", Unit::Tabs, "x"), "a\n\tb");
   }

   #[test]
   fn depth_is_rescaled_only_when_nothing_fits() {
      assert_eq!(
         follow("a\n    b\n        c", Unit::Spaces(3), "x"),
         "a\n   b\n      c"
      );
      assert_eq!(
         follow("a\n      b\n   c", Unit::Spaces(3), "x"),
         "a\n      b\n   c"
      );
      assert_eq!(follow("a\n    b", Unit::Spaces(2), "x"), "a\n    b");
      assert_eq!(follow("a\n       b", Unit::Spaces(3), "x"), "a\n       b");
   }

   #[test]
   fn aligned_lines_keep_their_depth() {
      let shape = Shape::new("(define (f)\n   (let* ([a 1]\n          ", "", "scheme");
      let reply = "[b 2])\n       (+ a b))";
      assert_eq!(super::clean(reply, &shape).text, reply);
   }

   #[test]
   fn a_line_still_streaming_keeps_its_whitespace() {
      assert_eq!(follow("a\n  ", Unit::Tabs, "x"), "a\n  ");
   }

   #[test]
   fn the_users_reply_loses_its_prose_and_the_suffix_echo() {
      let prefix = "   fn mean(&self) -> Option<f64> {\n      ";
      let shape = Shape::new(prefix, "} else if false {}\n   }\n", "rust");
      let cleaned = super::clean(USER_REPLY, &shape);
      assert!(cleaned.prose);
      assert_eq!(
         cleaned.text,
         "let sum: f64 = self.values.iter().sum();\n      Some(sum / self.values.len() as f64)"
      );
   }

   #[test]
   fn a_last_sentence_line_goes_when_the_stream_ends() {
      let shape = Shape::new("fn f() {\n   ", "\n}\n", "rust");
      let raw = "let a = 1;\nFirst Thing worth saying about this code here.";
      assert_eq!(super::clean(raw, &shape).text, raw);
      let end = super::clean_end(raw, &shape);
      assert_eq!(end.text, "let a = 1;");
      assert!(end.prose);
   }

   #[test]
   fn a_reply_that_explains_itself_shows_nothing() {
      let shape = Shape::new("fn f() {\n   x\n", "}\n", "rust");
      let cleaned = super::clean("The cursor sits after the call, so", &shape);
      assert_eq!(cleaned.text, "");
      assert!(cleaned.prose);
   }

   #[test]
   fn prose_is_kept_where_prose_belongs() {
      let reply = "The cursor sits here.";
      for (prefix, language) in [
         ("# Notes\n", "markdown"),
         ("fn f() {\n   // ", "rust"),
         ("let s = \"", "rust"),
         ("def f():\n    \"\"\"", "python"),
      ] {
         let cleaned = super::clean(reply, &Shape::new(prefix, "", language));
         assert_eq!(cleaned.text, reply, "{language}: {prefix}");
      }
   }

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
   fn indent_is_read_from_the_prefix() {
      let three = indent_of("fn f() {\n   if x {\n      y\n   }\n   ");
      assert_eq!(three.unit, Some(Unit::Spaces(3)));
      assert_eq!(three.line, "   ");
      assert!(three.at_line_start);
      let tabs = indent_of("func f() {\n\tif x {\n\t\ty\n\t}\n\tz := ");
      assert_eq!(tabs.unit, Some(Unit::Tabs));
      assert_eq!(tabs.line, "\t");
      assert!(!tabs.at_line_start);
      let two = indent_of("a {\n  b {\n    c\n  }\n\n  d {\n    ");
      assert_eq!(two.unit, Some(Unit::Spaces(2)));
      assert_eq!(two.line, "    ");
      assert_eq!(indent_of("a\nb\n").unit, None);
      assert_eq!(indent_of("fn f() {\n   ").unit, Some(Unit::Spaces(3)));
      assert_eq!(indent_of("").line, "");
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
