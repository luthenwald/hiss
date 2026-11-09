//! Completes fixtures through the API without Helix, for timings and for
//! judging the replies.
//!
//! usage: complete [--check] [--show] [--no-guard] [--compare] [--runs n] <fixture>...
//!
//! A fixture marks the cursor with `<|>`. Without `--check`, the first run's
//! completion is printed between rules, every run prints its timing, and the
//! medians follow. With `--check`, every reply is judged (`hiss::quality`):
//! the failing ones are printed (all of them with `--show`), then a table of
//! clean counts per fixture. `--no-guard` leaves prose in, to study what
//! the model writes. `--compare` alternates runs with and without the
//! worked examples and prints both medians, so drift in the API's speed
//! falls on both alike.

use std::time::Duration;

use hiss::RUNTIME;
use hiss::api::{Client, Request, Settings, Timing};
use hiss::clean::indent_of;
use hiss::quality::{indent_problem, prose};

const CURSOR: &str = "<|>";

fn language(path: &str) -> &'static str {
   match path.rsplit('.').next().unwrap_or("") {
      "rs" => "rust",
      "py" => "python",
      "scm" => "scheme",
      "go" => "go",
      "ts" => "typescript",
      "zig" => "zig",
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
   compare: bool,
   runs: usize,
   fixtures: Vec<String>,
}

fn options() -> Result<Options, String> {
   let mut options = Options {
      check: false,
      show: false,
      guard: true,
      compare: false,
      runs: 5,
      fixtures: Vec::new(),
   };
   let mut args = std::env::args().skip(1);
   while let Some(arg) = args.next() {
      match arg.as_str() {
         "--check" => options.check = true,
         "--show" => options.show = true,
         "--no-guard" => options.guard = false,
         "--compare" => options.compare = true,
         "--runs" => {
            let n = args.next().ok_or("--runs needs a number")?;
            options.runs = n.parse().map_err(|e| format!("--runs: {e}"))?;
         }
         _ => options.fixtures.push(arg),
      }
   }
   if options.fixtures.is_empty() {
      return Err(
         "usage: complete [--check] [--show] [--no-guard] [--compare] [--runs n] <fixture>..."
            .into(),
      );
   }
   Ok(options)
}

fn request(path: &str) -> Result<Request, String> {
   let source = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
   let (prefix, suffix) = source
      .split_once(CURSOR)
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
   empty: usize,
   cut: usize,
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
      if options.compare {
         let without = Settings {
            examples: false,
            ..settings.clone()
         };
         for path in &options.fixtures {
            let request = request(path)?;
            let (mut a, mut b) = (Vec::new(), Vec::new());
            for _ in 0..options.runs {
               let timing = client.complete(&request, &without, |_| {}).await?;
               a.extend(timing.first_text);
               let timing = client.complete(&request, &settings, |_| {}).await?;
               b.extend(timing.first_text);
            }
            println!(
               "{path}: median ttft without examples {}, with {}",
               ms(median(a)),
               ms(median(b))
            );
         }
         return Ok(());
      }
      let mut tallies = Vec::new();
      for path in &options.fixtures {
         let request = request(path)?;
         let indent = indent_of(&request.prefix);
         let name = path.rsplit('/').next().unwrap_or(path).to_string();
         let mut tally = Tally {
            name: name.clone(),
            ..Tally::default()
         };
         let mut totals = Vec::new();
         if !options.check {
            println!("{path}");
         }
         for run in 1..=options.runs {
            let mut last = String::new();
            let timing: Timing = client
               .complete(&request, &settings, |text| last = text.to_string())
               .await?;
            tally.runs += 1;
            tally.ttfts.extend(timing.first_text);
            totals.push(timing.total);
            if !options.check {
               if run == 1 {
                  println!("{}\n{last}\n{}", rule(), rule());
               }
               println!("{run:>2}  {timing}");
               continue;
            }
            if timing.stop_reason == "prose" {
               tally.cut += 1;
            }
            let problem = if last.trim().is_empty() {
               tally.empty += 1;
               None
            } else if prose(&last, true).is_some() {
               tally.prose += 1;
               Some("prose")
            } else if let Some(problem) = indent_problem(&last, &indent) {
               tally.indent += 1;
               Some(problem)
            } else {
               None
            };
            if problem.is_none() {
               tally.clean += 1;
            }
            if problem.is_some() || options.show {
               let verdict = problem.unwrap_or("clean");
               println!(
                  "{name} run {run}: {verdict}, {}\n{}",
                  timing.stop_reason,
                  rule()
               );
               println!("{last}\n{}", rule());
            }
         }
         if !options.check {
            println!(
               "median ttft {}, total {}",
               ms(median(tally.ttfts.clone())),
               ms(median(totals))
            );
         }
         tallies.push(tally);
      }
      if options.check {
         println!(
            "\n{:<12} {:>7} {:>6} {:>7} {:>6} {:>4} {:>9}",
            "fixture", "clean", "prose", "indent", "empty", "cut", "ttft"
         );
         for t in &tallies {
            println!(
               "{:<12} {:>7} {:>6} {:>7} {:>6} {:>4} {:>9}",
               t.name,
               format!("{}/{}", t.clean, t.runs),
               t.prose,
               t.indent,
               t.empty,
               t.cut,
               ms(median(t.ttfts.clone()))
            );
         }
         let sum = |f: fn(&Tally) -> usize| tallies.iter().map(f).sum::<usize>();
         let ttfts = tallies.iter().flat_map(|t| t.ttfts.clone()).collect();
         println!(
            "{:<12} {:>7} {:>6} {:>7} {:>6} {:>4} {:>9}",
            "all",
            format!("{}/{}", sum(|t| t.clean), sum(|t| t.runs)),
            sum(|t| t.prose),
            sum(|t| t.indent),
            sum(|t| t.empty),
            sum(|t| t.cut),
            ms(median(ttfts))
         );
      }
      Ok(())
   })
}
