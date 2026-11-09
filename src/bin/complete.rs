//! Completes a fixture through the API and prints the timings: libhiss's core
//! without Helix (roadmap 02/06).
//!
//! usage: complete <fixture> [runs]
//!
//! The fixture marks the cursor with `<|>`. The first run's completion is
//! printed between rules; every run prints its timing, then the medians.

use std::time::Duration;

use hiss::RUNTIME;
use hiss::api::{Client, Request, Settings};

const CURSOR: &str = "<|>";

fn language(path: &str) -> &'static str {
   match path.rsplit('.').next().unwrap_or("") {
      "rs" => "rust",
      "py" => "python",
      "scm" => "scheme",
      "zig" => "zig",
      "md" => "markdown",
      _ => "text",
   }
}

fn median(mut values: Vec<Duration>) -> Option<Duration> {
   values.sort();
   values.get(values.len() / 2).copied()
}

fn main() -> Result<(), String> {
   let mut args = std::env::args().skip(1);
   let path = args.next().ok_or("usage: complete <fixture> [runs]")?;
   let runs: usize = args
      .next()
      .map_or(Ok(5), |n| n.parse())
      .map_err(|e| format!("runs: {e}"))?;
   let source = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
   let (prefix, suffix) = source
      .split_once(CURSOR)
      .ok_or("the fixture has no <|> marker")?;
   let request = Request {
      prefix: prefix.into(),
      suffix: suffix.into(),
      language: language(&path).into(),
      path: path.clone(),
   };
   let settings = Settings::default();
   let client = Client::from_env()?;

   RUNTIME.block_on(async {
      let warm = client.warm().await?;
      println!("warm-up {} ms", warm.as_millis());
      let (mut ttfts, mut totals) = (Vec::new(), Vec::new());
      for run in 1..=runs {
         let mut last = String::new();
         let timing = client
            .complete(&request, &settings, |text| last = text.to_string())
            .await?;
         if run == 1 {
            println!("{}\n{last}\n{}", "─".repeat(40), "─".repeat(40));
         }
         println!("{run:>2}  {timing}");
         ttfts.extend(timing.first_text);
         totals.push(timing.total);
      }
      if let (Some(ttft), Some(total)) = (median(ttfts), median(totals)) {
         println!(
            "median ttft {} ms, total {} ms",
            ttft.as_millis(),
            total.as_millis()
         );
      }
      Ok(())
   })
}
