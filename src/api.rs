//! The Anthropic Messages API: one warm HTTP/2 client and a streaming
//! completion (spec 04/08). Nothing here logs; timings come back as values.

use std::fmt;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::clean;
use crate::sse::{self, Message};

const MESSAGES: &str = "https://api.anthropic.com/v1/messages";
const MODELS: &str = "https://api.anthropic.com/v1/models?limit=1";
const VERSION: &str = "2023-06-01";

/// Where a completion ends. The API refuses whitespace-only stop sequences,
/// so libhiss stops here itself and drops the stream, which resets it on the
/// server too.
const BLANK_LINE: &str = "\n\n";

/// The instructions. The rules answer what went wrong in the quality check
/// (`just quality`): commentary on broken files, and indentation that did not
/// follow the file.
const SYSTEM: &str = "\
You are the inline completion engine of a code editor. The user sends the file they are editing, with <CURSOR/> where the cursor is. You reply with the text to insert at the cursor, and nothing else.

Rules:
- Reply with code only, exactly as it should appear in the file. Never explain, comment on, or describe the code, and never talk to the user, even when the file looks broken or unfinished: then complete what the author is typing at the cursor and leave the rest alone.
- Never repeat text that already follows the cursor.
- Indent with the unit in the file's indent attribute. Each new line of your reply starts at column zero, so write its full indentation.
- When the cursor ends a line that opens a block, start your reply with a line break, then the block's first line at one level deeper.
- Stop at the end of the current statement or block. When nothing sensible fits, reply with nothing.";

/// Everything that shapes a request, with the defaults of spec 04/08 and 06/08.
#[derive(Debug, Clone)]
pub struct Settings {
   pub model: String,
   pub max_tokens: u32,
   /// About 2,000 tokens before the cursor, at roughly four characters each.
   pub prefix_chars: usize,
   /// About 500 tokens after the cursor.
   pub suffix_chars: usize,
}

impl Default for Settings {
   fn default() -> Self {
      Settings {
         model: "claude-haiku-5-5".into(),
         max_tokens: 96,
         prefix_chars: 8_000,
         suffix_chars: 2_000,
      }
   }
}

/// The text around the cursor, as the editor sends it.
#[derive(Debug, Clone, Default)]
pub struct Request {
   pub prefix: String,
   pub suffix: String,
   pub language: String,
   pub path: String,
}

/// What one request cost in time and size. Never any text.
#[derive(Debug, Clone, Default)]
pub struct Timing {
   /// Until the response headers arrived.
   pub headers: Duration,
   /// Until the first piece of text arrived, if any did.
   pub first_text: Option<Duration>,
   pub total: Duration,
   pub input_tokens: u64,
   pub cache_read_tokens: u64,
   pub output_tokens: u64,
   pub stop_reason: String,
   pub chars: usize,
}

impl fmt::Display for Timing {
   fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
      let ms = |d: Duration| d.as_millis();
      match self.first_text {
         Some(ttft) => write!(f, "ttft {} ms", ms(ttft))?,
         None => write!(f, "ttft -")?,
      }
      write!(
         f,
         ", total {} ms, headers {} ms, in {} (cached {}), out {}, {} chars, {}",
         ms(self.total),
         ms(self.headers),
         self.input_tokens,
         self.cache_read_tokens,
         self.output_tokens,
         self.chars,
         if self.stop_reason.is_empty() {
            "no stop"
         } else {
            &self.stop_reason
         },
      )
   }
}

/// The key from `HISS_ANTHROPIC_API_KEY`, else `ANTHROPIC_API_KEY`.
fn api_key() -> Option<String> {
   ["HISS_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]
      .iter()
      .filter_map(|name| std::env::var(name).ok())
      .find(|key| !key.trim().is_empty())
}

pub struct Client {
   http: reqwest::Client,
   key: String,
}

impl Client {
   pub fn from_env() -> Result<Client, String> {
      let key = api_key().ok_or("hiss: no api key")?;
      let http = reqwest::Client::builder()
         .connect_timeout(Duration::from_secs(5))
         .read_timeout(Duration::from_secs(20))
         .pool_idle_timeout(Duration::from_secs(600))
         .http2_keep_alive_interval(Duration::from_secs(30))
         .http2_keep_alive_timeout(Duration::from_secs(10))
         .http2_keep_alive_while_idle(true)
         .tcp_nodelay(true)
         .build()
         .map_err(|e| format!("hiss: no http client: {e}"))?;
      Ok(Client { http, key })
   }

   fn post(&self, url: &str) -> reqwest::RequestBuilder {
      self
         .http
         .post(url)
         .header("x-api-key", &self.key)
         .header("anthropic-version", VERSION)
   }

   /// Opens the connection (TCP, TLS, HTTP/2) with a cheap request, so the
   /// first completion does not pay for it. Returns how long that took.
   pub async fn warm(&self) -> Result<Duration, String> {
      let start = Instant::now();
      let response = self
         .http
         .get(MODELS)
         .header("x-api-key", &self.key)
         .header("anthropic-version", VERSION)
         .send()
         .await
         .map_err(|e| format!("hiss: {e}"))?;
      let status = response.status();
      let _ = response.bytes().await;
      if status.is_success() {
         Ok(start.elapsed())
      } else {
         Err(format!("hiss: api {}", status.as_u16()))
      }
   }

   /// Streams a completion. `on_text` gets the whole cleaned text each time
   /// it changes. Returns the timing, or an error for the status line.
   pub async fn complete(
      &self,
      request: &Request,
      settings: &Settings,
      mut on_text: impl FnMut(&str),
   ) -> Result<Timing, String> {
      let shape = clean::Shape::new(
         clean::tail_lines(&request.prefix, settings.prefix_chars),
         clean::head_lines(&request.suffix, settings.suffix_chars),
         &request.language,
      );
      let body = body(request, settings);
      let start = Instant::now();
      let mut response = self
         .post(MESSAGES)
         .header("content-type", "application/json")
         .body(body.to_string())
         .send()
         .await
         .map_err(|e| format!("hiss: {e}"))?;
      let mut timing = Timing {
         headers: start.elapsed(),
         ..Timing::default()
      };
      let status = response.status();
      if !status.is_success() {
         let text = response.text().await.unwrap_or_default();
         let detail = serde_json::from_str::<Value>(&text).map(|v| sse::error_text(&v));
         return Err(format!(
            "hiss: api {}: {}",
            status.as_u16(),
            detail.unwrap_or_else(|_| "no detail".into())
         ));
      }

      let mut parser = sse::Parser::default();
      let mut decoder = sse::Decoder::default();
      let mut raw = String::new();
      let mut shown = String::new();
      'stream: while let Some(chunk) = response.chunk().await.map_err(|e| format!("hiss: {e}"))? {
         for event in parser.push(&chunk) {
            match decoder.decode(&event) {
               Some(Message::Text(piece)) => {
                  timing.first_text.get_or_insert_with(|| start.elapsed());
                  raw.push_str(&piece);
                  let blank_line = raw.find(BLANK_LINE);
                  if let Some(at) = blank_line {
                     raw.truncate(at);
                  }
                  let cleaned = clean::clean(&raw, &shape);
                  if cleaned.text != shown {
                     shown = cleaned.text;
                     on_text(&shown);
                  }
                  if cleaned.prose {
                     timing.stop_reason = "prose".into();
                     break 'stream;
                  }
                  if blank_line.is_some() {
                     timing.stop_reason = "blank line".into();
                     break 'stream;
                  }
               }
               Some(Message::InputTokens { input, cache_read }) => {
                  timing.input_tokens = input;
                  timing.cache_read_tokens = cache_read;
               }
               Some(Message::Stop {
                  reason,
                  output_tokens,
               }) => {
                  timing.stop_reason = reason;
                  timing.output_tokens = output_tokens;
               }
               Some(Message::Error(text)) => return Err(format!("hiss: api error: {text}")),
               Some(Message::End) => break 'stream,
               None => {}
            }
         }
      }
      timing.total = start.elapsed();
      timing.chars = shown.chars().count();
      Ok(timing)
   }
}

/// The request body of spec 04/08. No sampling parameters and no prefill:
/// both return a 400 on the 5.5 models. No stop sequence either (see
/// `BLANK_LINE`).
pub fn body(request: &Request, settings: &Settings) -> Value {
   let prefix = clean::tail_lines(&request.prefix, settings.prefix_chars);
   let suffix = clean::head_lines(&request.suffix, settings.suffix_chars);
   let path = request.path.replace('"', "&quot;");
   let language = request.language.replace('"', "&quot;");
   let indent = clean::indent_of(prefix).unit;
   let file = format!(
      "<file path=\"{path}\" lang=\"{language}\" indent=\"{indent}\">\n{prefix}<CURSOR/>{suffix}\n</file>"
   );
   json!({
      "model": settings.model,
      "max_tokens": settings.max_tokens,
      "stream": true,
      "thinking": { "type": "disabled" },
      "output_config": { "effort": "low" },
      "system": [{ "type": "text", "text": SYSTEM }],
      "messages": [{ "role": "user", "content": file }],
   })
}

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn body_has_no_sampling_and_ends_with_the_user() {
      let request = Request {
         prefix: "fn main() {\n   let x = ".into(),
         suffix: ";\n}\n".into(),
         language: "rust".into(),
         path: "src/\"main\".rs".into(),
      };
      let body = body(&request, &Settings::default());
      for key in ["temperature", "top_p", "top_k"] {
         assert!(body.get(key).is_none());
      }
      assert_eq!(body["thinking"]["type"], "disabled");
      assert_eq!(body["output_config"]["effort"], "low");
      let messages = body["messages"].as_array().unwrap();
      assert_eq!(messages.last().unwrap()["role"], "user");
      assert_eq!(
         messages[0]["content"],
         "<file path=\"src/&quot;main&quot;.rs\" lang=\"rust\" indent=\"3 spaces\">\nfn main() {\n   let x = <CURSOR/>;\n}\n\n</file>"
      );
   }

   #[test]
   fn body_cuts_context_at_lines() {
      let request = Request {
         prefix: "a\n".repeat(10_000),
         suffix: "b\n".repeat(10_000),
         ..Request::default()
      };
      let body = body(&request, &Settings::default());
      let content = body["messages"][0]["content"].as_str().unwrap();
      assert!(content.len() < 10_200);
      assert!(content.contains("\na\n<CURSOR/>b\n"));
   }
}
