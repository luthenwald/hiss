//! Server-sent events from the Messages API, in two layers.
//!
//! `Parser` turns bytes into raw events. Chunks may split anywhere, even
//! inside a UTF-8 sequence, so it buffers bytes and only decodes whole lines.
//! `Decoder` turns raw events into what hiss needs. It keeps text only from
//! blocks that started as `text`, so thinking blocks, which Haiku 5.5 may
//! send even with thinking off, never reach the ghost.

use std::collections::HashSet;

use serde_json::Value;

/// One raw event: its `event:` name and its joined `data:` lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
   pub name: String,
   pub data: String,
}

#[derive(Debug, Default)]
pub struct Parser {
   pending: Vec<u8>,
   name: String,
   data: Vec<String>,
}

impl Parser {
   /// Feeds a chunk and returns the events it completed.
   pub fn push(&mut self, chunk: &[u8]) -> Vec<Event> {
      self.pending.extend_from_slice(chunk);
      let mut events = Vec::new();
      while let Some(end) = self.pending.iter().position(|&b| b == b'\n') {
         let line: Vec<u8> = self.pending.drain(..=end).collect();
         let line = String::from_utf8_lossy(&line);
         let line = line.trim_end_matches(['\n', '\r']);
         if let Some(event) = self.line(line) {
            events.push(event);
         }
      }
      events
   }

   fn line(&mut self, line: &str) -> Option<Event> {
      if line.is_empty() {
         if self.name.is_empty() && self.data.is_empty() {
            return None;
         }
         let event = Event {
            name: std::mem::take(&mut self.name),
            data: std::mem::take(&mut self.data).join("\n"),
         };
         return Some(event);
      }
      let (field, value) = line.split_once(':').unwrap_or((line, ""));
      let value = value.strip_prefix(' ').unwrap_or(value);
      match field {
         "event" => self.name = value.to_string(),
         "data" => self.data.push(value.to_string()),
         _ => {}
      }
      None
   }
}

/// What a stream says, once decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
   Text(String),
   InputTokens { input: u64, cache_read: u64 },
   Stop { reason: String, output_tokens: u64 },
   Error(String),
   End,
}

#[derive(Debug, Default)]
pub struct Decoder {
   text_blocks: HashSet<u64>,
}

impl Decoder {
   pub fn decode(&mut self, event: &Event) -> Option<Message> {
      let data: Value = serde_json::from_str(&event.data).ok()?;
      let kind = data["type"].as_str().unwrap_or(&event.name);
      match kind {
         "message_start" => {
            let usage = &data["message"]["usage"];
            <|>
               input: usage["input_tokens"].as_u64().unwrap_or(0),
               cache_read: usage["cache_read_input_tokens"].as_u64().unwrap_or(0),
            })
         }
         "content_block_start" => {
            if data["content_block"]["type"] == "text" {
               self.text_blocks.insert(data["index"].as_u64()?);
            }
            None
         }
         "content_block_delta" => {
            let index = data["index"].as_u64()?;
            let delta = &data["delta"];
            if self.text_blocks.contains(&index) && delta["type"] == "text_delta" {
               Some(Message::Text(delta["text"].as_str()?.to_string()))
            } else {
               None
            }
         }
         "message_delta" => Some(Message::Stop {
            reason: data["delta"]["stop_reason"]
               .as_str()
               .unwrap_or("")
               .to_string(),
            output_tokens: data["usage"]["output_tokens"].as_u64().unwrap_or(0),
         }),
         "message_stop" => Some(Message::End),
         "error" => Some(Message::Error(error_text(&data))),
         _ => None,
      }
   }
}

/// The `type: message` of an API error body, for status lines.
pub fn error_text(body: &Value) -> String {
   let error = &body["error"];
   match (error["type"].as_str(), error["message"].as_str()) {
      (Some(kind), Some(message)) => format!("{kind}: {message}"),
      (Some(kind), None) => kind.to_string(),
      _ => "unknown error".to_string(),
   }
}

#[cfg(test)]
mod tests {
   use super::*;

   fn decode_all(chunks: &[&[u8]]) -> Vec<Message> {
      let mut parser = Parser::default();
      let mut decoder = Decoder::default();
      chunks
         .iter()
         .flat_map(|chunk| parser.push(chunk))
         .filter_map(|event| decoder.decode(&event))
         .collect()
   }

   const STREAM: &str = concat!(
      "event: message_start\n",
      "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":812,\"cache_read_input_tokens\":0}}}\n\n",
      "event: content_block_start\n",
      "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
      "event: content_block_delta\n",
      "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hmm\"}}\n\n",
      "event: content_block_start\n",
      "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
      "event: ping\n",
      "data: {\"type\":\"ping\"}\n\n",
      "event: content_block_delta\n",
      "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"let x = \"}}\n\n",
      "event: content_block_delta\n",
      "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"\u{e9}t\u{e9};\"}}\n\n",
      "event: message_delta\n",
      "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"stop_sequence\"},\"usage\":{\"output_tokens\":7}}\n\n",
      "event: message_stop\n",
      "data: {\"type\":\"message_stop\"}\n\n",
   );

   fn expected() -> Vec<Message> {
      vec![
         Message::InputTokens {
            input: 812,
            cache_read: 0,
         },
         Message::Text("let x = ".into()),
         Message::Text("\u{e9}t\u{e9};".into()),
         Message::Stop {
            reason: "stop_sequence".into(),
            output_tokens: 7,
         },
         Message::End,
      ]
   }

   #[test]
   fn whole_stream_keeps_only_text_blocks() {
      assert_eq!(decode_all(&[STREAM.as_bytes()]), expected());
   }

   #[test]
   fn every_split_point_gives_the_same_messages() {
      let bytes = STREAM.as_bytes();
      for at in 0..bytes.len() {
         assert_eq!(
            decode_all(&[&bytes[..at], &bytes[at..]]),
            expected(),
            "split at {at}"
         );
      }
   }

   #[test]
   fn byte_by_byte_and_crlf() {
      let crlf = STREAM.replace('\n', "\r\n");
      let chunks: Vec<&[u8]> = crlf.as_bytes().chunks(1).collect();
      assert_eq!(decode_all(&chunks), expected());
   }

   #[test]
   fn error_event() {
      let stream = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";
      assert_eq!(
         decode_all(&[stream.as_bytes()]),
         vec![Message::Error("overloaded_error: Overloaded".into())]
      );
   }
}
