//! The Anthropic Messages API: one warm HTTP/2 client and a streaming
//! completion (spec 04/08). Nothing here logs; timings come back as values.

use std::fmt;
use std::time::{Duration, Instant};

use reqwest::header::HeaderValue;
use serde_json::{Value, json};

use crate::clean;
use crate::sse::{self, Message};

const MESSAGES: &str = "https://api.anthropic.com/v1/messages";
/// Followed by a model name, it describes that model.
const MODELS: &str = "https://api.anthropic.com/v1/models/";
const VERSION: &str = "2023-06-01";

/// The stop reason of a reply whose stream ended before the API said it
/// was done: the connection closed in the middle. Such a reply is shown but
/// not kept (`cache`).
pub const CUT: &str = "cut";

/// The instructions. One sentence on purpose: a fuller prompt, with rules
/// or with worked examples, made the model explain ordinary code instead of
/// completing it (`just quality`, 2026-11-09). Broken files are left to the
/// cleaning, which cuts prose.
const SYSTEM: &str = "You are a code completion engine. Output only the exact text to insert at <CURSOR/>. No prose, no code fences, no repetition of text after the cursor.";

/// Everything that shapes a request, with the defaults of spec 04/08 and 06/08.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
   pub model: String,
   pub max_tokens: u32,
   /// About 2,000 tokens before the cursor, at roughly four characters each.
   pub prefix_chars: usize,
   /// About 500 tokens after the cursor.
   pub suffix_chars: usize,
   /// Cut prose from replies (`clean::Shape`). Off only to study raw replies.
   pub prose_guard: bool,
   /// Times to ask again after an API or network error that may pass
   /// (overloaded, rate limited, a lost connection), before any text showed.
   pub retries: u32,
   /// Times to ask again after a reply that was only prose.
   pub prose_retries: u32,
   /// Send the file's far part as its own block, marked for prompt caching
   /// (`cache_split`). For measuring with `just bench` only, for now.
   pub cache_file: bool,
}

impl Default for Settings {
   fn default() -> Self {
      Settings {
         model: "claude-haiku-5-5".into(),
         max_tokens: 96,
         prefix_chars: 8_000,
         suffix_chars: 2_000,
         prose_guard: true,
         retries: 1,
         prose_retries: 1,
         cache_file: false,
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

impl Request {
   /// The window a request sends: at most `prefix_chars` before the cursor
   /// and `suffix_chars` after it, cut at line boundaries. `Client::complete`
   /// and `body` take a request cut this way.
   pub fn cut(self, settings: &Settings) -> Request {
      Request {
         prefix: clean::tail_lines(&self.prefix, settings.prefix_chars).to_string(),
         suffix: clean::head_lines(&self.suffix, settings.suffix_chars).to_string(),
         ..self
      }
   }
}

/// What one request cost in time and size. Never any text.
#[derive(Debug, Clone, Default)]
pub struct Timing {
   /// Until the response headers arrived.
   pub headers: Duration,
   /// Until the first text was shown (after cleaning), if any was.
   pub first_text: Option<Duration>,
   pub total: Duration,
   pub input_tokens: u64,
   pub cache_read_tokens: u64,
   pub output_tokens: u64,
   pub stop_reason: String,
   pub chars: usize,
   /// The first reply was cut down to nothing as prose, and hiss asked again.
   pub retried: bool,
   /// How many times hiss asked again after an error.
   pub error_retries: u32,
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
      )?;
      if self.retried {
         write!(f, ", retried")?;
      }
      if self.error_retries > 0 {
         write!(f, ", {} after errors", self.error_retries)?;
      }
      Ok(())
   }
}

/// The key from `HISS_ANTHROPIC_API_KEY`, else `ANTHROPIC_API_KEY`, without
/// the spaces or the line break a shell may have left around it.
fn api_key() -> Option<String> {
   first_key(
      ["HISS_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]
         .iter()
         .filter_map(|name| std::env::var(name).ok()),
   )
}

fn first_key(values: impl Iterator<Item = String>) -> Option<String> {
   values
      .map(|key| key.trim().to_string())
      .find(|key| !key.is_empty())
}

pub struct Client {
   http: reqwest::Client,
   /// Marked sensitive, so that no debug output shows it.
   key: HeaderValue,
}

impl Client {
   pub fn from_env() -> Result<Client, String> {
      let mut key = HeaderValue::from_str(&api_key().ok_or("hiss: no api key")?)
         .map_err(|_| "hiss: the api key has characters a header cannot hold")?;
      key.set_sensitive(true);
      // No redirects: reqwest keeps custom headers such as `x-api-key` on a
      // redirect to another host, so one would hand the key to that host.
      let http = reqwest::Client::builder()
         .redirect(reqwest::redirect::Policy::none())
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

   fn signed(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
      request
         .header("x-api-key", self.key.clone())
         .header("anthropic-version", VERSION)
   }

   /// Opens the connection (TCP, TLS, HTTP/2) with a cheap request, so the
   /// first completion does not pay for it, and asks for `model`, which
   /// checks the key and the model name on the way. Returns how long that
   /// took.
   pub async fn warm(&self, model: &str) -> Result<Duration, Failure> {
      let start = Instant::now();
      let response = self
         .signed(self.http.get(format!("{MODELS}{model}")))
         .send()
         .await
         .map_err(Failure::network)?;
      let status = response.status();
      if !status.is_success() {
         let text = response.text().await.unwrap_or_default();
         return Err(Failure::refused(status.as_u16(), &text));
      }
      let _ = response.bytes().await;
      Ok(start.elapsed())
   }

   /// Streams a completion. `on_text` gets the whole cleaned text each time
   /// it changes, with how many chars before the cursor it starts. When the
   /// reply is only prose, which the cleaning cuts to nothing, it asks again
   /// at once (up to `prose_retries` times), with the model's prose as its
   /// earlier turn and a note that only code is wanted. Returns the timing,
   /// or why it failed. `request` is cut already (`Request::cut`).
   pub async fn complete(
      &self,
      request: &Request,
      settings: &Settings,
      mut on_text: impl FnMut(&str, usize),
   ) -> Result<Timing, Failure> {
      let mut shape = clean::Shape::new(&request.prefix, &request.suffix, &request.language);
      shape.prose_guard &= settings.prose_guard;
      let mut body = body(request, &shape, settings);
      let start = Instant::now();
      let mut timing = Timing::default();
      let mut reply = self
         .persist(&body, &shape, settings, start, &mut timing, &mut on_text)
         .await?;
      for _ in 0..settings.prose_retries {
         if !(reply.prose && reply.shown.is_empty() && !reply.raw.trim().is_empty()) {
            break;
         }
         timing.retried = true;
         body = retry_body(&body, reply.raw.trim_end());
         reply = self
            .persist(&body, &shape, settings, start, &mut timing, &mut on_text)
            .await?;
      }
      timing.total = start.elapsed();
      Ok(timing)
   }

   /// One attempt, asked again up to `retries` times after an error that may
   /// pass, as long as nothing was shown yet, a little later each time.
   async fn persist(
      &self,
      body: &Value,
      shape: &clean::Shape<'_>,
      settings: &Settings,
      start: Instant,
      timing: &mut Timing,
      on_text: &mut impl FnMut(&str, usize),
   ) -> Result<Reply, Failure> {
      let mut tries = 0;
      loop {
         match self.attempt(body, shape, start, timing, on_text).await {
            Ok(reply) => return Ok(reply),
            Err(failure)
               if failure.transient && tries < settings.retries && timing.first_text.is_none() =>
            {
               tries += 1;
               timing.error_retries += 1;
               tokio::time::sleep(Duration::from_millis(250 * u64::from(tries))).await;
            }
            Err(failure) => return Err(failure),
         }
      }
   }

   /// One request and its stream. Adds what it costs to `timing`.
   async fn attempt(
      &self,
      body: &Value,
      shape: &clean::Shape<'_>,
      start: Instant,
      timing: &mut Timing,
      on_text: &mut impl FnMut(&str, usize),
   ) -> Result<Reply, Failure> {
      let mut response = self
         .signed(self.http.post(MESSAGES))
         .header("content-type", "application/json")
         .body(body.to_string())
         .send()
         .await
         .map_err(Failure::network)?;
      if timing.headers.is_zero() {
         timing.headers = start.elapsed();
      }
      let status = response.status();
      if !status.is_success() {
         let text = response.text().await.unwrap_or_default();
         return Err(Failure::refused(status.as_u16(), &text));
      }

      timing.stop_reason.clear();
      let mut parser = sse::Parser::default();
      let mut decoder = sse::Decoder::default();
      let mut wire = String::new();
      let mut raw = String::new();
      let mut shown = String::new();
      let mut shown_back = 0;
      let mut prose = false;
      let mut show = |cleaned: clean::Cleaned, timing: &mut Timing, shown: &mut String| {
         if cleaned.text != *shown || cleaned.back != shown_back {
            if !cleaned.text.is_empty() {
               timing.first_text.get_or_insert_with(|| start.elapsed());
            }
            *shown = cleaned.text;
            shown_back = cleaned.back;
            on_text(shown, shown_back);
         }
      };
      'stream: while let Some(chunk) = response.chunk().await.map_err(Failure::network)? {
         for event in parser.push(&chunk) {
            match decoder.decode(&event) {
               Some(Message::Text(piece)) => {
                  // A blank line ends the completion. The API refuses
                  // whitespace-only stop sequences, so libhiss stops there
                  // itself and drops the stream, which ends it on the server.
                  wire.push_str(&piece);
                  raw = clean::unescape(&wire, shape).unwrap_or_else(|| wire.clone());
                  let blank_line = clean::blank_line(&raw);
                  if let Some(at) = blank_line {
                     raw.truncate(at);
                  }
                  let cleaned = clean::clean(&raw, shape);
                  let cut = cleaned.prose;
                  show(cleaned, timing, &mut shown);
                  if cut {
                     prose = true;
                     timing.stop_reason = "prose".into();
                     break 'stream;
                  }
                  if blank_line.is_some() {
                     timing.stop_reason = "blank line".into();
                     break 'stream;
                  }
               }
               Some(Message::InputTokens { input, cache_read }) => {
                  timing.input_tokens += input;
                  timing.cache_read_tokens += cache_read;
               }
               Some(Message::Stop {
                  reason,
                  output_tokens,
               }) => {
                  timing.stop_reason = reason;
                  timing.output_tokens += output_tokens;
               }
               Some(Message::Error(text)) => {
                  return Err(Failure {
                     transient: ["overloaded", "rate_limit", "rate limit"]
                        .iter()
                        .any(|kind| text.to_lowercase().contains(kind)),
                     lasting: false,
                     message: format!("hiss: api error: {text}"),
                  });
               }
               Some(Message::End) => break 'stream,
               None => {}
            }
         }
      }
      match timing.stop_reason.as_str() {
         "" => timing.stop_reason = CUT.into(),
         // The token limit stops the reply anywhere, mid-word as well.
         "max_tokens" => raw.truncate(clean::whole_lines(&raw).len()),
         _ => {}
      }
      let cleaned = clean::clean_end(&raw, shape);
      let cut = cleaned.prose;
      show(cleaned, timing, &mut shown);
      if cut {
         prose = true;
         timing.stop_reason = "prose".into();
      }
      timing.chars = shown.chars().count();
      Ok(Reply { raw, shown, prose })
   }
}

/// Why a request failed, for the status line, and whether asking again may
/// help: at once after a lost connection, a rate limit or an overloaded
/// server (`transient`), and not before the user changes something after a
/// refusal that every request would meet (`lasting`): a key the API does not
/// take, a model it does not know or a setting it rejects. The request hiss
/// sends differs only in its text, so a 400 is a setting too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
   pub message: String,
   pub transient: bool,
   pub lasting: bool,
}

impl Failure {
   /// The address is left out, since it is always the API's, and the cause
   /// is added: reqwest's own line says only that sending failed.
   fn network(error: reqwest::Error) -> Failure {
      let transient = !error.is_builder() && !error.is_decode();
      Failure {
         message: format!("hiss: {}", with_cause(&error.without_url())),
         transient,
         lasting: false,
      }
   }

   /// A response with status `code` and an error `body`.
   fn refused(code: u16, body: &str) -> Failure {
      let detail = serde_json::from_str::<Value>(body).map(|v| sse::error_text(&v));
      Failure {
         message: format!(
            "hiss: api {code}: {}",
            detail.unwrap_or_else(|_| "no detail".into())
         ),
         transient: code == 429 || code >= 500,
         lasting: matches!(code, 400 | 401 | 403 | 404),
      }
   }
}

/// An error's own line, then its innermost cause when it names one, as
/// `error sending request: connection refused`.
fn with_cause(error: &dyn std::error::Error) -> String {
   let root = std::iter::successors(error.source(), |cause| cause.source()).last();
   let line = error.to_string();
   match root.map(|root| root.to_string()) {
      Some(root) if !line.contains(&root) => format!("{line}: {root}"),
      _ => line,
   }
}

impl From<Failure> for String {
   fn from(failure: Failure) -> String {
      failure.message
   }
}

/// What one attempt left behind.
struct Reply {
   /// The model's text, up to where the stream was stopped.
   raw: String,
   shown: String,
   prose: bool,
}

/// What the second request adds after a reply that was only prose.
/// It does not offer "nothing" as an answer: the model then wrote
/// "(nothing)".
const RETRY: &str =
   "That was a comment, not code. Reply with only the code to insert at <CURSOR/>.";

/// The first request, with the model's prose as its turn and the note.
fn retry_body(first: &Value, prose: &str) -> Value {
   let mut body = first.clone();
   if let Some(messages) = body["messages"].as_array_mut() {
      messages.push(json!({ "role": "assistant", "content": prose }));
      messages.push(json!({ "role": "user", "content": RETRY }));
   }
   body
}

/// Tells the model which word the cursor is in, since the file it sees has
/// that word left out (`clean::heal`). Measured on the word fixtures
/// (2026-11-10): with the word left in and no note, 12/28 replies joined it
/// wrongly (`impo` then `ort`, `instance` then `Functor`); shorter notes made
/// the model write the word alone, or explain; asking to finish the line
/// kept many replies to one line; this one, 28/28 over several lines.
fn word_note(word: &str) -> String {
   format!(
      "The text at <CURSOR/> starts with `{word}`; write that word out in full, then complete the code."
   )
}

/// The request body of spec 04/08, for a cut `request` and its `shape`.
/// No sampling parameters and no prefill: both return a 400 on the 5.5
/// models. No stop sequence either (see `clean::blank_line`).
pub fn body(request: &Request, shape: &clean::Shape, settings: &Settings) -> Value {
   let suffix = shape.suffix;
   let path = attribute(&request.path);
   let language = attribute(&request.language);
   let indent = shape
      .indent
      .unit
      .map(|unit| format!(" indent=\"{unit}\""))
      .unwrap_or_default();
   let sent = shape.sent();
   let note = match &shape.typed {
      clean::Typed::Word(word) => format!("\n{}", word_note(word)),
      _ => String::new(),
   };
   let open = format!("<file path=\"{path}\" lang=\"{language}\"{indent}>\n");
   let near = |text: &str| format!("{text}<CURSOR/>{suffix}\n</file>{note}");
   let content = match cache_split(sent) {
      Some(at) if settings.cache_file => json!([
         {
            "type": "text",
            "text": format!("{open}{}", &sent[..at]),
            "cache_control": { "type": "ephemeral" },
         },
         { "type": "text", "text": near(&sent[at..]) },
      ]),
      _ => json!(format!("{open}{}", near(sent))),
   };
   json!({
      "model": settings.model,
      "max_tokens": settings.max_tokens,
      "stream": true,
      "thinking": { "type": "disabled" },
      "output_config": { "effort": "low" },
      "system": [{ "type": "text", "text": SYSTEM }],
      "messages": [{ "role": "user", "content": content }],
   })
}

/// `text` as it may stand between the quotes of an attribute of the file
/// tag: markup escaped, and on one line.
fn attribute(text: &str) -> String {
   text
      .replace('&', "&amp;")
      .replace('<', "&lt;")
      .replace('>', "&gt;")
      .replace('"', "&quot;")
      .replace(['\n', '\r'], " ")
}

/// Where the cached part of the file ends: the last line start at least
/// `NEAR` bytes before the cursor, so edits near the cursor leave it alone.
/// `None` when the file before the cursor is shorter than that.
fn cache_split(sent: &str) -> Option<usize> {
   const NEAR: usize = 1_000;
   let mut end = sent.len().checked_sub(NEAR)?;
   while !sent.is_char_boundary(end) {
      end -= 1;
   }
   sent[..end].rfind('\n').map(|i| i + 1)
}

#[cfg(test)]
mod tests {
   use super::*;

   /// The body for an uncut request, as `complete` would send it.
   fn body(request: &Request, settings: &Settings) -> Value {
      let request = request.clone().cut(settings);
      let shape = clean::Shape::new(&request.prefix, &request.suffix, &request.language);
      super::body(&request, &shape, settings)
   }

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
         messages.last().unwrap()["content"],
         "<file path=\"src/&quot;main&quot;.rs\" lang=\"rust\" indent=\"3 spaces\">\nfn main() {\n   let x = <CURSOR/>;\n}\n\n</file>"
      );
   }

   #[test]
   fn refusals_every_request_meets_last() {
      let body = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
      let key = Failure::refused(401, body);
      assert_eq!(
         key.message,
         "hiss: api 401: authentication_error: invalid x-api-key"
      );
      assert!(key.lasting && !key.transient);
      assert!(Failure::refused(404, "").lasting);
      let busy = Failure::refused(529, "");
      assert!(busy.transient && !busy.lasting);
      assert!(!Failure::refused(429, "").lasting);
   }

   #[test]
   fn the_key_is_trimmed() {
      let keys = |values: &[&str]| first_key(values.iter().map(|v| v.to_string()));
      assert_eq!(keys(&["  ", "sk-abc\n"]).as_deref(), Some("sk-abc"));
      assert_eq!(keys(&["", " "]), None);
   }

   #[test]
   fn an_error_names_its_innermost_cause() {
      #[derive(Debug)]
      struct Layer(&'static str, Option<Box<Layer>>);
      impl fmt::Display for Layer {
         fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.0)
         }
      }
      impl std::error::Error for Layer {
         fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.1.as_deref().map(|inner| inner as _)
         }
      }
      let layer = |text, inner: Option<Layer>| Layer(text, inner.map(Box::new));
      let refused = layer(
         "error sending request",
         Some(layer(
            "client error",
            Some(layer("connection refused", None)),
         )),
      );
      assert_eq!(
         with_cause(&refused),
         "error sending request: connection refused"
      );
      assert_eq!(with_cause(&layer("timed out", None)), "timed out");
   }

   #[test]
   fn the_file_tag_escapes_its_attributes() {
      assert_eq!(
         attribute("a<b>&\"c\"\nd.rs"),
         "a&lt;b&gt;&amp;&quot;c&quot; d.rs"
      );
      assert_eq!(attribute("src/main.rs"), "src/main.rs");
   }

   #[test]
   fn cached_body_splits_far_from_the_cursor() {
      let request = Request {
         prefix: format!("{}let x = ", "far line\n".repeat(200)),
         suffix: ";\n".into(),
         language: "rust".into(),
         path: "a.rs".into(),
      };
      let plain = body(&request, &Settings::default());
      let cached = body(
         &request,
         &Settings {
            cache_file: true,
            ..Settings::default()
         },
      );
      let blocks = cached["messages"][0]["content"].as_array().unwrap();
      assert_eq!(blocks[0]["cache_control"]["type"], "ephemeral");
      let far = blocks[0]["text"].as_str().unwrap();
      let near = blocks[1]["text"].as_str().unwrap();
      assert!(far.ends_with('\n') && near.len() >= 1_000);
      assert_eq!(format!("{far}{near}"), plain["messages"][0]["content"]);
      let short = Request {
         prefix: "let x = ".into(),
         ..request
      };
      assert!(
         body(
            &short,
            &Settings {
               cache_file: true,
               ..Settings::default()
            }
         )["messages"][0]["content"]
            .is_string()
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
      let content = body["messages"].as_array().unwrap().last().unwrap()["content"]
         .as_str()
         .unwrap();
      assert!(content.len() < 10_200);
      assert!(content.contains("\na\n<CURSOR/>b\n"));
   }
}
