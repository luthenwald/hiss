//! Judging a reply: does it turn into prose, and does it follow the file's
//! indentation. The cleaning uses `prose` to cut a reply short; the harness
//! (`complete --check`) uses both to count clean replies.

use crate::clean::{Indent, Unit};

/// Chars that end a piece of code, after which a capital starts a sentence.
const CLOSERS: [char; 4] = ['}', ')', ']', ';'];

/// Line starts that make a line a comment or a string, never prose.
const QUOTED: [&str; 9] = ["//", "#", ";", "--", "/*", "*", "\"", "'", "`"];

const PUNCTUATION: [char; 6] = [',', '.', ':', ';', '!', '?'];

/// A word of prose: letters, perhaps an apostrophe or hyphen inside, and at
/// most a mark of punctuation at the end.
fn is_word(token: &str) -> bool {
   let core = token.trim_end_matches(PUNCTUATION);
   core.chars().next().is_some_and(char::is_alphabetic)
      && core
         .chars()
         .all(|c| c.is_alphabetic() || matches!(c, '\'' | '’' | '-'))
}

/// A word that opens a sentence: a capital, then lowercase ("The", "I",
/// "Nothing"), but not an all-caps keyword such as SQL's SELECT.
fn is_opening_word(token: &str) -> bool {
   let core = token.trim_end_matches(PUNCTUATION);
   is_word(token)
      && core.chars().next().is_some_and(char::is_uppercase)
      && (core.chars().count() == 1 || core.chars().skip(1).any(char::is_lowercase))
}

fn is_lower_word(token: &str) -> bool {
   is_word(token) && token.chars().next().is_some_and(char::is_lowercase)
}

/// Replaces `code spans`, which prose about code is full of, with a word.
fn without_code_spans(line: &str) -> String {
   line
      .split('`')
      .enumerate()
      .map(|(i, part)| if i % 2 == 1 { "code" } else { part })
      .collect::<Vec<_>>()
      .join(" ")
}

/// Text that opens like a sentence: a capitalised word, then two lowercase
/// words ("The cursor sits", "Nothing to insert", "The `if` block").
fn opens_sentence(text: &str) -> bool {
   let line = without_code_spans(text.split('\n').next().unwrap_or(""));
   let tokens: Vec<&str> = line.split_whitespace().take(3).collect();
   matches!(tokens[..], [a, b, c] if is_opening_word(a) && is_lower_word(b) && is_lower_word(c))
}

/// A whole line that reads as a sentence: not a comment or a string, five
/// words or more, nearly all of them plain words, no code punctuation, and
/// ending like a sentence (or running on for eight words).
fn is_sentence(line: &str) -> bool {
   let trimmed = line.trim();
   if QUOTED.iter().any(|q| trimmed.starts_with(q)) {
      return false;
   }
   let plain = without_code_spans(trimmed);
   let tokens: Vec<&str> = plain.split_whitespace().collect();
   let words = tokens.iter().filter(|t| is_word(t)).count();
   tokens.len() >= 5
      && words * 5 >= tokens.len() * 4
      && !plain.contains(['{', '}', '(', ')', '[', ']', '=', '<', '>', ';'])
      && trimmed.chars().next().is_some_and(char::is_uppercase)
      && (trimmed.ends_with(['.', ':']) || tokens.len() >= 8)
}

/// Inside a docstring the reply itself opened, prose is content.
fn in_docstring(before: &str) -> bool {
   (before.matches("\"\"\"").count() + before.matches("\'\'\'").count()) % 2 == 1
}

/// Languages written in prose, where a sentence is the content.
const PROSE_LANGUAGES: [&str; 6] = ["", "text", "markdown", "plaintext", "git-commit", "mail"];

/// Whether the prose guard applies at this cursor: not in a prose file, a
/// comment, a string or a docstring, judged from the text before the cursor.
pub fn prose_applies(prefix: &str, language: &str) -> bool {
   let line = prefix.rsplit('\n').next().unwrap_or("").trim_start();
   !PROSE_LANGUAGES.contains(&language)
      && !QUOTED[..6].iter().any(|q| line.starts_with(q))
      && !line.contains("//")
      && line.matches('"').count() % 2 == 0
      && line.matches('`').count() % 2 == 0
      && !in_docstring(prefix)
}

/// The start of a line, or of words glued to code, that has fewer than
/// three words so far and could still open a sentence: a capitalised word,
/// then lowercase ones. Shown only once it is clearly one or the other, so
/// a sentence never flashes on screen before the cut.
fn may_open_sentence(text: &str) -> bool {
   let line = without_code_spans(text);
   let tokens: Vec<&str> = line.split_whitespace().collect();
   match tokens[..] {
      [] => false,
      [first, ref rest @ ..] if rest.len() < 2 => {
         first.chars().next().is_some_and(char::is_uppercase)
            && is_word(first)
            && (first.chars().count() <= 2 || is_opening_word(first))
            && rest.iter().all(|t| is_lower_word(t))
      }
      _ => false,
   }
}

/// Where the text still being written may turn out to be prose, as a byte
/// offset: its last line (which is not complete yet), or words glued to
/// closing code on it.
pub fn pending(text: &str) -> Option<usize> {
   let start = text.rfind('\n').map_or(0, |at| at + 1);
   let line = &text[start..];
   let indent = line.len() - line.trim_start().len();
   if !in_docstring(&text[..start]) && may_open_sentence(&line[indent..]) {
      return Some(start);
   }
   line.char_indices().find_map(|(i, c)| {
      let after = i + c.len_utf8();
      let rest = &line[after..];
      (CLOSERS.contains(&c)
         && rest.chars().next().is_some_and(char::is_uppercase)
         && may_open_sentence(rest))
      .then_some(start + after)
   })
}

/// Words the model puts in parentheses instead of a reply.
const PLACEHOLDERS: [&str; 7] = [
   "nothing",
   "empty",
   "no completion",
   "no code",
   "no change",
   "no changes",
   "blank",
];

/// A whole reply that only stands for no reply, like "(nothing)".
fn is_placeholder(text: &str) -> bool {
   text
      .trim()
      .strip_prefix('(')
      .and_then(|t| t.strip_suffix(')'))
      .is_some_and(|inner| PLACEHOLDERS.contains(&inner.trim().to_lowercase().as_str()))
}

/// Where prose starts in `text`, as a byte offset, if it does:
/// - a sentence glued to closing code (`}The mean ...`);
/// - a line that opens like a sentence (`The cursor sits ...`), which counts
///   as soon as its first three words are in;
/// - a whole line that reads as a sentence, once the line is complete, or
///   at once when `complete` says the whole reply is in.
pub fn prose(text: &str, complete: bool) -> Option<usize> {
   let glued = text.char_indices().find_map(|(i, c)| {
      let after = i + c.len_utf8();
      let rest = &text[after..];
      (CLOSERS.contains(&c)
         && rest.chars().next().is_some_and(char::is_uppercase)
         && opens_sentence(rest))
      .then_some(after)
   });
   let mut start = 0;
   let lines: Vec<(usize, &str)> = text
      .split('\n')
      .map(|line| {
         let at = start;
         start += line.len() + 1;
         (at, line)
      })
      .filter(|&(at, _)| !in_docstring(&text[..at]))
      .collect();
   let opening = lines
      .iter()
      .find(|(_, line)| opens_sentence(line.trim_start()))
      .map(|&(at, _)| at);
   let whole = lines
      .iter()
      .filter(|&&(at, line)| complete || at + line.len() < text.len())
      .find(|(_, line)| is_sentence(line))
      .map(|&(at, _)| at);
   let placeholder = is_placeholder(text).then_some(0);
   [placeholder, glued, opening, whole]
      .into_iter()
      .flatten()
      .min()
}

/// What is wrong with the reply's indentation, if anything: a first line
/// that repeats the indent the cursor already sits after, or a later line
/// whose leading whitespace is not a whole number of the file's units.
pub fn indent_problem(text: &str, indent: &Indent) -> Option<&'static str> {
   let mut lines = text.split('\n');
   let first = lines.next().unwrap_or("");
   if indent.at_line_start && text.starts_with('\n') {
      return Some("starts a line lower");
   }
   if indent.at_line_start && !indent.line.is_empty() && first.starts_with([' ', '\t']) {
      return Some("doubled indent");
   }
   lines
      .filter(|line| !line.trim().is_empty())
      .map(|line| &line[..line.len() - line.trim_start_matches([' ', '\t']).len()])
      .find_map(|lead| match indent.unit? {
         Unit::Tabs if lead.contains(' ') => Some("spaces in a tab file"),
         Unit::Spaces(_) if lead.contains('\t') => Some("tabs in a space file"),
         Unit::Spaces(n) if !indent.aligned && lead.len() % n != 0 => Some("off the unit"),
         _ => None,
      })
}

/// Whether the reply's first line, drawn `back` chars left of the cursor at a
/// line start, lands off the file's unit (two spaces in a three-space file).
pub fn first_column_problem(text: &str, back: usize, indent: &Indent) -> Option<&'static str> {
   let Some(Unit::Spaces(n)) = indent.unit else {
      return None;
   };
   if !indent.at_line_start || indent.aligned || indent.line.contains('\t') {
      return None;
   }
   let first = text.split('\n').next().unwrap_or("");
   let own = first.len() - first.trim_start_matches(' ').len();
   let column = indent.line.len().saturating_sub(back) + own;
   (column % n != 0).then_some("first line off the unit")
}

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn a_first_line_off_the_unit_is_seen() {
      let indent = Indent {
         unit: Some(Unit::Spaces(3)),
         line: "  ".into(),
         at_line_start: true,
         aligned: false,
      };
      assert_eq!(
         first_column_problem("x", 0, &indent),
         Some("first line off the unit")
      );
      assert_eq!(first_column_problem("x", 2, &indent), None);
      assert_eq!(first_column_problem(" x", 0, &indent), None);
   }

   const USER_REPLY: &str = "let sum: f64 = self.values.iter().sum();\n      Some(sum / self.values.len() as f64)} else if false {}The mean method is broken around the cursor: the `if` block is closed before the `let sum` line, and there's a stray `} else if false {}` after the cursor. The completion should fix the body at the cursor position, closing the method properly.";

   #[test]
   fn the_users_reply_turns_into_prose_after_the_code() {
      let at = prose(USER_REPLY, false).expect("prose");
      assert!(USER_REPLY[..at].ends_with("} else if false {}"));
   }

   #[test]
   fn the_start_of_a_sentence_is_pending() {
      assert_eq!(pending("let a = 1;\nThe"), Some(11));
      assert_eq!(pending("let a = 1;\n   The cursor"), Some(11));
      assert_eq!(pending("x}The mean"), Some(2));
      assert_eq!(pending("Some"), Some(0));
      assert_eq!(pending("Some(x"), None);
      assert_eq!(pending("let a"), None);
      assert_eq!(pending("SELECT name"), None);
      assert_eq!(pending("The cursor sits"), None);
      assert_eq!(pending("let a = 1;\n"), None);
   }

   #[test]
   fn a_placeholder_is_no_reply() {
      assert_eq!(prose("(nothing)", false), Some(0));
      assert_eq!(prose("  (No completion)\n", false), Some(0));
      assert_eq!(prose("(nothing-to-do x)", false), None);
      assert_eq!(prose("(display x)", false), None);
   }

   #[test]
   fn glued_prose_needs_three_words() {
      assert_eq!(prose("x}The mean", false), None);
      assert_eq!(prose("x}The mean is", false), Some(2));
   }

   #[test]
   fn code_is_not_prose() {
      for code in [
         "let total = Some(1);",
         "return counter.most_common(10)",
         "if x {\n   y\n}",
         "    counts[word] = counts.get(word, 0) + 1\n",
         "Foo::new(Bar)",
         "(foldl + 0 numbers)",
         "return foo if bar else baz and qux or quux\n",
      ] {
         assert_eq!(prose(code, true), None, "{code}");
      }
   }

   #[test]
   fn comments_are_not_prose() {
      assert_eq!(
         prose("x += 1\n// This keeps the total in range for now.\n", true),
         None
      );
      assert_eq!(
         prose("x += 1\n# This keeps the total in range for now.\n", true),
         None
      );
   }

   #[test]
   fn a_reply_that_opens_like_a_sentence_is_prose() {
      for reply in [
         "The cursor sits after a stray `} else if false {}` on the line.",
         "Nothing to insert: the cursor sits after a stray brace.",
         "The `if` block is closed early.",
         "x += 1\n   The rest of the method",
      ] {
         assert!(prose(reply, false).is_some(), "{reply}");
      }
      assert_eq!(prose("x += 1\n   The rest of", false), Some(7));
   }

   #[test]
   fn keywords_strings_and_docstrings_are_not_prose() {
      for reply in [
         "SELECT name FROM users WHERE id = 1",
         "    \"\"\"Return the mean.\n\n    Raises an error when values is empty.\n    \"\"\"",
         "Ok(value)",
         "Some(sum / count)",
         "println!(\"The mean is {}\", mean);",
      ] {
         assert_eq!(prose(reply, true), None, "{reply}");
      }
   }

   #[test]
   fn a_sentence_line_is_prose_once_complete() {
      let text = "x += 1\nTotal Count adds one to the counter.";
      assert_eq!(prose(text, false), None);
      assert_eq!(prose(text, true), Some(7));
      assert_eq!(prose(&format!("{text}\n"), false), Some(7));
   }

   fn indent(unit: Unit, line: &str, at_line_start: bool) -> Indent {
      Indent {
         unit: Some(unit),
         aligned: false,
         line: line.into(),
         at_line_start,
      }
   }

   #[test]
   fn indentation_problems() {
      let three = indent(Unit::Spaces(3), "      ", true);
      assert_eq!(indent_problem("let a = 1;\n      a", &three), None);
      assert_eq!(
         indent_problem("   let a = 1;", &three),
         Some("doubled indent")
      );
      assert_eq!(
         indent_problem("\n      let a = 1;", &three),
         Some("starts a line lower")
      );
      assert_eq!(
         indent_problem("let a = 1;\n    a", &three),
         Some("off the unit")
      );
      assert_eq!(
         indent_problem("let a = 1;\n\ta", &three),
         Some("tabs in a space file")
      );
      let tabs = indent(Unit::Tabs, "\t", true);
      assert_eq!(indent_problem("x := 1\n\treturn x", &tabs), None);
      assert_eq!(
         indent_problem("x := 1\n    return x", &tabs),
         Some("spaces in a tab file")
      );
      let none = indent(Unit::Spaces(4), "", true);
      assert_eq!(indent_problem("    x", &none), None);
   }
}
