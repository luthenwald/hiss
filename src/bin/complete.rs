//! Completes fixtures through the API without Helix, for timings and for
//! judging the replies.
//!
//! usage: complete [--check] [--show] [--no-guard] [--runs n]
//!                 [--before a,b] [--after a,b] [--max-tokens a,b] [--cache off,on]
//!                 <fixture>...
//!
//! A fixture marks the cursor with `<|>`, the last one when the file has
//! several (a source file may contain the marker itself). Without `--check`, the first run's
//! completion is printed between rules, every run prints its timing, and the
//! medians follow. With `--check`, every reply is judged (`hiss::quality`):
//! the failing ones are printed (all of them with `--show`), then a table of
//! clean counts per fixture. `--no-guard` leaves prose in, to study what
//! the model writes.
//!
//! `--before`, `--after`, `--max-tokens` and `--cache` take lists, and every
//! combination is a variant. Each run asks once per variant, in turn, since
//! the API's speed drifts over minutes and only interleaved runs compare;
//! the medians per variant follow, with the first run's time to first text
//! apart, since the first request after the warm-up can be slower.

use std::time::Duration;

use hiss::RUNTIME;
use hiss::api::{Client, Request, Settings, Timing};
use hiss::clean::Shape;
use hiss::quality::{first_column_problem, indent_problem, prose};

const CURSOR: &str = "<|>";

fn language(path: &str) -> &'static str {
   match path.rsplit('.').next().unwrap_or("") {
      "rs" => "rust",
      "py" => "python",
      "scm" => "scheme",
      "go" => "go",
      "ts" => "typescript",
      "zig" => "zig",
      "hs" => "haskell",
      "md" => "markdown",
      _ => "text",
   }
}

fn median(mut values: Vec<Duration>) -> Option<Duration> {
   values.sort();
   values.get(values.len() / 2).copied()
}

fn ms(duration: Option<Duration>) -> String {
   duration.map_or("-".into(), |d| format!("{} ms", d.as_millis()))
}

struct Options {
   check: bool,
   show: bool,
   guard: bool,
   runs: usize,
   before: Vec<usize>,
   after: Vec<usize>,
   max_tokens: Vec<u32>,
   cache: Vec<bool>,
   fixtures: Vec<String>,
}

/// A comma-separated list of values.
fn list<T: std::str::FromStr>(flag: &str, text: Option<String>) -> Result<Vec<T>, String> {
   let text = text.ok_or(format!("{flag} needs a list"))?;
   text
      .split(',')
      .map(|item| {
         item
            .trim()
            .parse()
            .map_err(|_| format!("{flag}: cannot read {item}"))
      })
      .collect()
}

/// Every combination of the listed values, each named by the values that
/// vary.
fn variants(options: &Options, base: &Settings) -> Vec<(String, Settings)> {
   let o = options;
   o.before
      .iter()
      .flat_map(|&b| o.after.iter().map(move |&a| (b, a)))
      .flat_map(|(b, a)| o.max_tokens.iter().map(move |&m| (b, a, m)))
      .flat_map(|(b, a, m)| o.cache.iter().map(move |&c| (b, a, m, c)))
      .map(|(before, after, max_tokens, cache)| {
         let label = [
            (o.before.len() > 1).then(|| format!("before={before}")),
            (o.after.len() > 1).then(|| format!("after={after}")),
            (o.max_tokens.len() > 1).then(|| format!("max={max_tokens}")),
            (o.cache.len() > 1).then(|| format!("cache={}", if cache { "on" } else { "off" })),
         ]
         .into_iter()
         .flatten()
         .collect::<Vec<_>>()
         .join(" ");
         let settings = Settings {
            prefix_chars: before,
            suffix_chars: after,
            max_tokens,
            cache_file: cache,
            ..base.clone()
         };
         (label, settings)
      })
      .collect()
}

fn options() -> Result<Options, String> {
   let base = Settings::default();
   let mut options = Options {
      check: false,
      show: false,
      guard: true,
      runs: 5,
      before: vec![base.prefix_chars],
      after: vec![base.suffix_chars],
      max_tokens: vec![base.max_tokens],
      cache: vec![false],
      fixtures: Vec::new(),
   };
   let mut args = std::env::args().skip(1);
   while let Some(arg) = args.next() {
      match arg.as_str() {
         "--check" => options.check = true,
         "--show" => options.show = true,
         "--no-guard" => options.guard = false,
         "--runs" => {
            let n = args.next().ok_or("--runs needs a number")?;
            options.runs = n.parse().map_err(|e| format!("--runs: {e}"))?;
         }
         "--before" => options.before = list(&arg, args.next())?,
         "--after" => options.after = list(&arg, args.next())?,
         "--max-tokens" => options.max_tokens = list(&arg, args.next())?,
         "--cache" => {
            options.cache = list::<String>(&arg, args.next())?
               .iter()
               .map(|word| match word.as_str() {
                  "on" => Ok(true),
                  "off" => Ok(false),
                  _ => Err(format!("--cache: on or off, not {word}")),
               })
               .collect::<Result<_, _>>()?
         }
         _ => options.fixtures.push(arg),
      }
   }
   if options.fixtures.is_empty() {
      return Err("usage: complete [--check] [--show] [--no-guard] [--runs n] [--before a,b] [--after a,b] [--max-tokens a,b] [--cache off,on] <fixture>...".into());
   }
   Ok(options)
}

fn request(path: &str) -> Result<Request, String> {
   let source = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
   let (prefix, suffix) = source
      .rsplit_once(CURSOR)
      .ok_or(format!("{path} has no <|> marker"))?;
   Ok(Request {
      prefix: prefix.into(),
      suffix: suffix.into(),
      language: language(path).into(),
      path: path.into(),
   })
}

/// One fixture's tally under `--check`.
#[derive(Default)]
struct Tally {
   name: String,
   runs: usize,
   clean: usize,
   prose: usize,
   indent: usize,
   joint: usize,
   empty: usize,
   cut: usize,
   retried: usize,
   ttfts: Vec<Duration>,
}

fn rule() -> String {
   "─".repeat(60)
}

fn main() -> Result<(), String> {
   let options = options()?;
   let settings = Settings {
      prose_guard: options.guard,
      ..Settings::default()
   };
   let client = Client::from_env()?;

   RUNTIME.block_on(async {
      let warm = client.warm().await?;
      println!("warm-up {} ms", warm.as_millis());
      if !options.check {
         return bench(&client, &options, &settings).await;
      }
      let mut tallies = Vec::new();
      for path in &options.fixtures {
         let request = request(path)?;
         let indent = Shape::new(&request.prefix, &request.suffix, &request.language).indent;
         let name = path.rsplit('/').next().unwrap_or(path).to_string();
         let mut tally = Tally {
            name: name.clone(),
            ..Tally::default()
         };
         // What a clean reply must start with, for fixtures where the cursor
         // splits or ends a word: `tests/expect/<fixture>`.
         let expect = std::path::Path::new(path)
            .parent()
            .map(|dir| dir.join("../expect").join(&name))
            .and_then(|file| std::fs::read_to_string(file).ok());
         for run in 1..=options.runs {
            let mut last = String::new();
            let mut back = 0;
            let timing: Timing = client
               .complete(&request, &settings, |text, b| {
                  last = text.to_string();
                  back = b;
               })
               .await?;
            tally.runs += 1;
            tally.ttfts.extend(timing.first_text);
            if timing.stop_reason == "prose" {
               tally.cut += 1;
            }
            if timing.retried {
               tally.retried += 1;
            }
            let problem = if last.trim().is_empty() {
               tally.empty += 1;
               None
            } else if prose(&last, true).is_some() {
               tally.prose += 1;
               Some("prose")
            } else if let Some(problem) =
               indent_problem(&last, &indent).or(first_column_problem(&last, back, &indent))
            {
               tally.indent += 1;
               Some(problem)
            } else if expect
               .as_ref()
               .is_some_and(|start| !last.starts_with(start.as_str()))
            {
               tally.joint += 1;
               Some("joint")
            } else {
               None
            };
            if problem.is_none() {
               tally.clean += 1;
            }
            if problem.is_some() || options.show {
               let verdict = problem.unwrap_or("clean");
               println!(
                  "{name} run {run}: {verdict}, {}, back {back}\n{}",
                  timing.stop_reason,
                  rule()
               );
               println!("{last}\n{}", rule());
            }
         }
         tallies.push(tally);
      }
      println!(
         "\n{:<14} {:>7} {:>6} {:>7} {:>6} {:>6} {:>4} {:>6} {:>9}",
         "fixture", "clean", "prose", "indent", "joint", "empty", "cut", "retry", "ttft"
      );
      for t in &tallies {
         println!(
            "{:<14} {:>7} {:>6} {:>7} {:>6} {:>6} {:>4} {:>6} {:>9}",
            t.name,
            format!("{}/{}", t.clean, t.runs),
            t.prose,
            t.indent,
            t.joint,
            t.empty,
            t.cut,
            t.retried,
            ms(median(t.ttfts.clone()))
         );
      }
      let sum = |f: fn(&Tally) -> usize| tallies.iter().map(f).sum::<usize>();
      let ttfts = tallies.iter().flat_map(|t| t.ttfts.clone()).collect();
      println!(
         "{:<14} {:>7} {:>6} {:>7} {:>6} {:>6} {:>4} {:>6} {:>9}",
         "all",
         format!("{}/{}", sum(|t| t.clean), sum(|t| t.runs)),
         sum(|t| t.prose),
         sum(|t| t.indent),
         sum(|t| t.joint),
         sum(|t| t.empty),
         sum(|t| t.cut),
         sum(|t| t.retried),
         ms(median(ttfts))
      );
      Ok(())
   })
}

/// Times every variant on every fixture, run by run in turn, and prints the
/// first run's completion, each timing, and the medians per variant.
async fn bench(client: &Client, options: &Options, base: &Settings) -> Result<(), String> {
   let variants = variants(options, base);
   for path in &options.fixtures {
      let request = request(path)?;
      println!("{path}");
      let mut times: Vec<(Vec<Duration>, Vec<Duration>)> = vec![Default::default(); variants.len()];
      for run in 1..=options.runs {
         for ((label, settings), (ttfts, totals)) in variants.iter().zip(&mut times) {
            let mut last = String::new();
            let timing = client
               .complete(&request, settings, |text, _| last = text.to_string())
               .await?;
            if run == 1 {
               println!("{}\n{last}\n{}", rule(), rule());
            }
            println!("{run:>2}  {label:<14} {timing}");
            ttfts.push(timing.first_text.unwrap_or(timing.total));
            totals.push(timing.total);
         }
      }
      println!(
         "\n{:<24} {:>9} {:>9} {:>9}",
         "variant", "ttft 2..", "total", "ttft 1"
      );
      for ((label, _), (ttfts, totals)) in variants.iter().zip(times) {
         let first = ttfts.first().copied();
         let rest = median(ttfts.iter().skip(1).copied().collect());
         println!(
            "{:<24} {:>9} {:>9} {:>9}",
            if label.is_empty() { "default" } else { label },
            ms(rest.or(first)),
            ms(median(totals)),
            ms(first),
         );
      }
   }
   Ok(())
}
