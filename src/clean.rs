//! Shaping text on the way in (context cuts) and on the way out (cleaning).

use crate::quality;

const CURSOR: &str = "<CURSOR/>";
const FENCE: &str = "```";

/// What the cleaning needs to know about a request, worked out once.
#[derive(Debug, Clone)]
pub struct Shape<'a> {
   /// The text before the cursor, as the user has it.
   pub prefix: &'a str,
   pub suffix: &'a str,
   pub indent: Indent,
   /// Whether prose is cut: not in prose files, comments or strings.
   pub prose_guard: bool,
   /// What the request left out before the cursor (`heal`).
   pub typed: Typed,
}

impl<'a> Shape<'a> {
   pub fn new(prefix: &'a str, suffix: &'a str, language: &str) -> Shape<'a> {
      Shape {
         prefix,
         suffix,
         indent: Indent {
            aligned: ALIGNED.contains(&language),
            ..indent_of(prefix)
         },
         prose_guard: quality::prose_applies(prefix, language),
         typed: heal(prefix, language).1,
      }
   }
}

/// The text just before the cursor that the request leaves out, so that the
/// model writes it whole. A model that sees half a word often gets the joint
/// wrong (`impo` then `ort`, or `instance` then `Functor` with no space), and
/// one that sees stray spaces on an empty line writes its line after them,
/// even where it belongs further left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Typed {
   /// The cursor follows a space or punctuation: nothing is left out.
   Nothing,
   /// The cursor ends a word, which every reply must start with.
   Word(String),
   /// Only whitespace sits before the cursor on its line, and it is not a
   /// whole number of the file's units (two stray spaces in a three-space
   /// file). The reply brings its own indentation, which decides where it
   /// is drawn. Whitespace that fits the unit is where Helix's auto-indent
   /// put the cursor, and is kept: the model, shown a line start there,
   /// often writes no indentation at all.
   Lead(String),
}

/// Splits the text before the cursor into what the request sends and what it
/// leaves out.
pub fn heal<'p>(prefix: &'p str, language: &str) -> (&'p str, Typed) {
   let line_start = prefix.rfind('\n').map_or(0, |i| i + 1);
   let line = &prefix[line_start..];
   if line.trim().is_empty() {
      let off_unit = match indent_of(prefix).unit {
         Some(Unit::Spaces(n)) => line.contains('\t') || line.len() % n != 0,
         Some(Unit::Tabs) => line.contains(' '),
         None => false,
      };
      return match off_unit {
         true => (&prefix[..line_start], Typed::Lead(line.into())),
         false => (prefix, Typed::Nothing),
      };
   }
   let lisp = ALIGNED.contains(&language);
   let is_word = |c: char| c.is_alphanumeric() || c == '_' || (lisp && "-?!*<>=/".contains(c));
   let start = prefix
      .char_indices()
      .rev()
      .take_while(|&(_, c)| is_word(c))
      .last()
      .map_or(prefix.len(), |(i, _)| i);
   match &prefix[start..] {
      "" => (prefix, Typed::Nothing),
      word => (&prefix[..start], Typed::Word(word.into())),
   }
}

/// The reply as it may be shown, and whether prose was cut from it, which
/// ends the stream. `back` counts the chars before the cursor that the ghost
/// takes the place of: the reply's line starts that far left of the cursor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cleaned {
   pub text: String,
   pub prose: bool,
   pub back: usize,
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
   // A reply that does not start with the word the cursor is in completes
   // something else, and one that is still shorter has nothing to show yet.
   let text = match &shape.typed {
      Typed::Word(word) => match unquote(text, word).strip_prefix(word.as_str()) {
         Some(rest) => rest,
         None => return Cleaned::default(),
      },
      _ => text,
   };
   let cut = shape
      .prose_guard
      .then(|| quality::prose(text, complete))
      .flatten();
   let text = cut.map_or(text, |at| text[..at].trim_end());
   let held = (shape.prose_guard && !complete && cut.is_none())
      .then(|| quality::pending(text))
      .flatten();
   let text = held.map_or(text, |at| text[..at].trim_end());
   let text = &text[..text.len() - suffix_overlap(text, shape.suffix)];
   let prose = cut.is_some();
   if text.trim().is_empty() {
      return Cleaned {
         prose,
         ..Cleaned::default()
      };
   }
   let healed = matches!(shape.typed, Typed::Lead(_));
   let text = follow_indent(text, &shape.indent, healed);
   let (back, text) = match &shape.typed {
      Typed::Lead(lead) => split_lead(text, lead),
      _ => (0, text),
   };
   let text = align(&text, shape, back);
   Cleaned { text, prose, back }
}

/// Where the reply reaches a blank line, which ends it: a line break, then a
/// line with nothing on it but spaces or tabs, then another break.
pub fn blank_line(text: &str) -> Option<usize> {
   text.match_indices('\n').map(|(at, _)| at).find(|&at| {
      text[at + 1..]
         .trim_start_matches([' ', '\t'])
         .starts_with('\n')
   })
}

/// A reply that writes its line breaks as `\n` escapes, all on one line
/// (`\n\ninstance Monad List where\n ...`), with the breaks put back. Only
/// one that starts with two escapes and has no real break, outside strings
/// and comments, since `\n` is code as well (a Haskell lambda, a string).
pub fn unescape(text: &str, shape: &Shape) -> Option<String> {
   let escaped = shape.prose_guard
      && text.starts_with("\\n")
      && text.matches("\\n").count() >= 2
      && !text.contains('\n');
   escaped.then(|| text.replace("\\n", "\n"))
}

/// A reply that wraps the word and the rest in backticks, as inline code,
/// loses them. Only right before the word, since a reply may well start with
/// a backtick of its own (Haskell's `div`).
fn unquote<'t>(text: &'t str, word: &str) -> &'t str {
   match text.strip_prefix('`') {
      Some(rest) if rest.starts_with(word) && !rest.starts_with('`') => {
         rest.strip_suffix('`').unwrap_or(rest)
      }
      _ => text,
   }
}

/// A reply at a line start carries its own indentation, and the cursor sits
/// after `lead`. The part of the reply's indentation the line already has is
/// dropped; when the reply starts further left, it is drawn from there, and
/// `back` says how many chars of `lead` it replaces.
fn split_lead(text: String, lead: &str) -> (usize, String) {
   if let Some(rest) = text.strip_prefix(lead) {
      return (0, rest.to_string());
   }
   let common: usize = text
      .chars()
      .zip(lead.chars())
      .take_while(|(a, b)| a == b)
      .map(|(a, _)| a.len_utf8())
      .sum();
   let back = lead[common..].chars().count();
   (back, text[common..].to_string())
}

/// How many spaces a tab stands for when a tab file gets spaces.
const TAB_WIDTH: usize = 4;

/// Brings the reply's indentation in line with the file, on lines that have
/// content (a line still streaming keeps its whitespace until text arrives):
/// - at the start of a line, a reply that opens with blank lines loses them,
///   since they would leave the cursor's line empty and start lower;
/// - lines use the file's unit, a tab standing for one unit;
/// - in a file indented with spaces, when no line fits the file's width but
///   all fit another (4 in a 3-space file), the depth is rescaled.
///
/// The first line counts only when the request left the line's whitespace
/// out (`healed`) and the reply writes its indentation whole. Otherwise, at a
/// line start, a first line that repeats the indent the cursor already sits
/// after loses it, since `insert_string` would put it in twice. When the unit
/// is unknown, only the first rule and that one apply.
fn follow_indent(text: &str, indent: &Indent, healed: bool) -> String {
   let text = if indent.at_line_start {
      let blank = text.len() - text.trim_start().len();
      match text[..blank].rfind('\n') {
         Some(newline) => &text[newline + 1..],
         None => text,
      }
   } else {
      text
   };
   let mut lines: Vec<String> = text.split('\n').map(String::from).collect();
   let base = match (indent.at_line_start, healed) {
      (true, false) => indent.line.len(),
      _ => 0,
   };
   if base > 0 {
      if let Some(rest) = lines[0].strip_prefix(indent.line.as_str()) {
         lines[0] = rest.to_string();
      }
   }
   let Some(unit) = indent.unit else {
      return lines.join("\n");
   };
   let skip = usize::from(!healed);
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
      .skip(skip)
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
   for line in lines.iter_mut().skip(skip) {
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
   // At a line start the model sometimes indents its first line by a width
   // the file never uses (two spaces in a three-space file) while the rest
   // keeps to the unit; that line moves to the nearest whole unit, counted
   // from the line start, without going left of the cursor.
   if let (true, Unit::Spaces(n), false) = (indent.at_line_start, unit, indent.aligned) {
      let on_unit = |l: &String| l.trim().is_empty() || leading(l).len() % n == 0;
      let first = &lines[0];
      let width = leading(first).len();
      let column = base + width;
      if !first.trim().is_empty()
         && column % n != 0
         && !indent.line.contains('\t')
         && !first[..width].contains('\t')
         && lines[1..].iter().all(on_unit)
      {
         let rounded = ((column + n / 2) / n * n).max(base);
         lines[0] = format!("{}{}", " ".repeat(rounded - base), &first[width..]);
      }
   }
   lines.join("\n")
}

/// The words of a line outside brackets, after its indentation, as
/// (column, word, preceded by two spaces or more). Tabs inside a line make
/// columns uncertain, so such a line gives nothing.
fn words(line: &str) -> Vec<(usize, &str, bool)> {
   let lead = leading(line).len();
   if line[lead..].contains('\t') {
      return Vec::new();
   }
   let mut found = Vec::new();
   let (mut depth, mut gap, mut column) = (0i32, 0usize, 0usize);
   let mut start = None;
   for (i, c) in line.char_indices() {
      if c == ' ' {
         if let Some(s) = start.take() {
            found.push(s);
         }
         gap += 1;
      } else {
         if start.is_none() {
            let word_end = line[i..].find(' ').map_or(line.len(), |e| i + e);
            if i >= lead && depth == 0 {
               start = Some((column, &line[i..word_end], gap >= 2 && i > lead));
            } else {
               start = Some((column, "", false));
            }
         }
         gap = 0;
         depth += match c {
            '(' | '[' | '{' => 1,
            ')' | ']' | '}' => -1,
            _ => 0,
         };
      }
      column += 1;
   }
   found.extend(start);
   found.retain(|(_, word, _)| !word.is_empty());
   found
}

/// Lines up the reply with the lines above it: where those lines put a word
/// at a column after a run of spaces (`(>>=)  :: m a     -> m b`), the same
/// word in a reply line at the same depth is moved right to that column.
/// Only spaces are added, only outside brackets, and the words keep their
/// order. Nothing changes when the lines above show no such run.
fn align(text: &str, shape: &Shape, back: usize) -> String {
   let prefix_lines: Vec<&str> = shape.prefix.split('\n').collect();
   let (above, cursor_line) = prefix_lines.split_at(prefix_lines.len() - 1);
   let cursor_line = cursor_line[0];
   let kept = cursor_line.chars().count().saturating_sub(back);
   let start: String = cursor_line.chars().take(kept).collect();
   let mut done: Vec<String> = Vec::new();
   for (i, piece) in text.split('\n').enumerate() {
      let head = if i == 0 { start.as_str() } else { "" };
      let line = format!("{head}{piece}");
      let depth = leading(&line).len();
      let block = done
         .iter()
         .rev()
         .map(String::as_str)
         .chain(above.iter().rev().copied())
         .take_while(|l| !l.trim().is_empty() && leading(l).len() == depth);
      let marks: Vec<(usize, String)> = block
         .flat_map(|l| {
            words(l)
               .into_iter()
               .filter(|&(_, _, run)| run)
               .map(|(col, word, _)| (col, word.to_string()))
               .collect::<Vec<_>>()
         })
         .collect();
      let aligned = if marks.is_empty() {
         line
      } else {
         let head_len = head.chars().count();
         let mut out = line.clone();
         let (mut shift, mut last) = (0usize, 0usize);
         for (col, word, _) in words(&line).into_iter().skip(1) {
            let at = col + shift;
            let target = marks
               .iter()
               .filter(|(k, w)| w == word && *k >= at && *k > last)
               .map(|(k, _)| *k)
               .min();
            if let Some(k) = target {
               if k > at && at >= head_len {
                  let byte = out.char_indices().nth(at).map_or(out.len(), |(b, _)| b);
                  out.insert_str(byte, &" ".repeat(k - at));
                  shift += k - at;
               }
               last = k;
            }
         }
         out
      };
      done.push(aligned);
   }
   let mut joined = done.join("\n");
   joined.drain(..start.len());
   joined
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
      follow_indent(text, &indent, false)
   }

   /// Cleans a reply at the end of `prefix` in a Rust file.
   fn at(prefix: &str, raw: &str) -> Cleaned {
      super::clean_end(raw, &Shape::new(prefix, "\n}\n", "rust"))
   }

   const BLOCK: &str = "fn f() {\n   if x {\n      y\n   }\n";

   #[test]
   fn the_lines_own_indent_is_split_from_the_reply() {
      let prefix = format!("{BLOCK}      ");
      let same = at(&prefix, "      let a = 1;");
      assert_eq!((same.back, same.text.as_str()), (0, "let a = 1;"));
      let bare = at(&prefix, "let a = 1;\nb");
      assert_eq!((bare.back, bare.text.as_str()), (0, "let a = 1;\nb"));
      let deeper = at(&format!("{BLOCK}   "), "      a\n   b");
      assert_eq!((deeper.back, deeper.text.as_str()), (0, "   a\n   b"));
      let top = at(&format!("{BLOCK}}}\n  "), "fn g() {}");
      assert_eq!((top.back, top.text.as_str()), (2, "fn g() {}"));
   }

   #[test]
   fn a_first_line_off_the_unit_takes_the_nearest_unit() {
      let prefix = "class C a where\n   f :: a\n";
      let shape = Shape::new(prefix, "", "haskell");
      assert_eq!(
         super::clean_end("  g :: a\n   g = f", &shape).text,
         "   g :: a\n   g = f"
      );
      assert_eq!(super::clean_end(" g :: a", &shape).text, "g :: a");
      assert_eq!(super::clean_end("  g\n     h", &shape).text, "  g\n     h");
   }

   #[test]
   fn the_word_at_the_cursor_is_left_out_and_put_back() {
      assert_eq!(
         heal("x\nimpo", "haskell"),
         ("x\n", Typed::Word("impo".into()))
      );
      assert_eq!(
         heal("a {\n   b\n  ", "rust"),
         ("a {\n   b\n", Typed::Lead("  ".into()))
      );
      assert_eq!(heal("a {\n   b\n   ", "rust").1, Typed::Nothing);
      assert_eq!(heal("x\n", "rust"), ("x\n", Typed::Nothing));
      assert_eq!(heal("f(a, ", "rust"), ("f(a, ", Typed::Nothing));
      assert_eq!(
         heal("(hiss-con", "scheme").1,
         Typed::Word("hiss-con".into())
      );
      let shape = Shape::new("x = 1\nimpo", "", "haskell");
      let clean = |raw| super::clean(raw, &shape).text;
      assert_eq!(clean("import Prelude"), "rt Prelude");
      assert_eq!(clean("imp"), "");
      assert_eq!(clean("module X"), "");
      assert_eq!(clean("`import Data.List`"), "rt Data.List");
      let shape = Shape::new("x = 1\ninstance", "", "haskell");
      assert_eq!(
         super::clean("instance Functor Maybe where", &shape).text,
         " Functor Maybe where"
      );
   }

   #[test]
   fn a_reply_lines_up_with_the_lines_above() {
      let class = "class Monad m where\n   (>>=)  :: m a     -> (a -> m b) -> m b\n   (>>)   :: m a     -> m b        -> m b\n";
      let shape = Shape::new(class, "", "haskell");
      assert_eq!(
         super::clean_end("   return :: a -> m a\n   join :: m (m a) -> m a", &shape).text,
         "   return :: a       -> m a\n   join   :: m (m a) -> m a"
      );
      let mid = format!("{class}   join :: m (m a) ");
      let shape = Shape::new(&mid, "", "haskell");
      assert_eq!(super::clean_end("-> m a", &shape).text, "  -> m a");
      let plain = "fn f() {\n   let a = 1;\n";
      let shape = Shape::new(plain, "", "rust");
      assert_eq!(
         super::clean_end("   let bb = 2;", &shape).text,
         "   let bb = 2;"
      );
   }

   #[test]
   fn a_line_of_spaces_is_a_blank_line() {
      assert_eq!(blank_line("a\n\nb"), Some(1));
      assert_eq!(blank_line("\n   \n   instance"), Some(0));
      assert_eq!(blank_line("a\n   b\n\t\nc"), Some(6));
      assert_eq!(blank_line("a\n   "), None);
   }

   #[test]
   fn escaped_line_breaks_are_put_back() {
      let shape = Shape::new("f = g ys)", "\n", "haskell");
      let raw = "\\n\\ninstance Monad List where\\n   Nil >>= _ = Nil";
      assert_eq!(
         unescape(raw, &shape).as_deref(),
         Some("\n\ninstance Monad List where\n   Nil >>= _ = Nil")
      );
      assert_eq!(unescape("\\n -> n + 1", &shape), None);
      assert_eq!(unescape("\\n\\n\nx", &shape), None);
      let string = Shape::new("s = \"a", "\n", "haskell");
      assert_eq!(unescape("\\n\\nb\"", &string), None);
   }

   #[test]
   fn a_leading_line_break_at_a_line_start_is_dropped() {
      let cleaned = at(&format!("{BLOCK}      "), "\n      let a = 1;\n      a");
      assert_eq!(cleaned.text, "let a = 1;\n      a");
      assert_eq!(follow("\n   a", Unit::Spaces(3), "x {"), "\n   a");
      assert_eq!(
         follow("  \n\npub fn f()", Unit::Spaces(3), ""),
         "pub fn f()"
      );
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
   fn a_sentence_does_not_flash_before_the_cut() {
      let shape = Shape::new("fn f() {\n   ", "\n}\n", "rust");
      assert_eq!(super::clean("The cur", &shape).text, "");
      assert_eq!(super::clean("let a = 1;\nThe", &shape).text, "let a = 1;");
      assert_eq!(super::clean("Some(a", &shape).text, "Some(a");
      assert_eq!(super::clean_end("None", &shape).text, "None");
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
