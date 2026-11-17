//! A minimal Discord IPC client: just enough to show and clear one activity.
//!
//! Reads block, so they only happen straight after a write (the handshake and SET_ACTIVITY both get
//! a reply). Any I/O error means the pipe is dead and the caller should drop the client and reconnect.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};

use serde_json::{Value, json};

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;

/// Discord's own frames are far smaller; this only stops a corrupt length from allocating gigabytes.
const MAX_FRAME: usize = 1 << 20;
/// Events can arrive between a request and its reply, so skip a few before giving up.
const MAX_SKIPPED_FRAMES: usize = 16;

#[derive(Debug)]
pub enum DiscordError {
    /// No `discord-ipc-N` pipe could be opened.
    NotRunning,
    /// The pipe broke or Discord closed it.
    Disconnected(String),
    /// Discord answered, but refused the request. The connection is still fine.
    Rejected(String),
}

impl fmt::Display for DiscordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning => write!(f, "Discord isn't running"),
            Self::Disconnected(m) => write!(f, "Discord connection lost: {m}"),
            Self::Rejected(m) => write!(f, "Discord refused the request: {m}"),
        }
    }
}

impl std::error::Error for DiscordError {}

impl From<io::Error> for DiscordError {
    fn from(e: io::Error) -> Self {
        Self::Disconnected(e.to_string())
    }
}

pub fn encode_frame(op: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

pub fn read_frame(r: &mut impl Read) -> io::Result<(u32, Vec<u8>)> {
    let mut header = [0u8; 8];
    r.read_exact(&mut header)?;
    let op = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("frame of {len} bytes is too large")));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok((op, payload))
}

pub struct Activity<'a> {
    pub name: &'a str,
    pub state: &'a str,
    pub start_ms: u64,
    pub image: Option<&'a str>,
}

pub fn activity_json(a: &Activity) -> Value {
    let mut activity = json!({
        "type": 0,
        "details": a.name,
        "state": a.state,
        "timestamps": { "start": a.start_ms },
        // 2 shows the game name (details) instead of "Xbox" in the member list
        "status_display_type": 2,
    });
    if let Some(image) = a.image {
        activity["assets"] = json!({ "large_image": image, "large_text": a.name });
    }
    activity
}

pub struct Discord<T: Read + Write = File> {
    pipe: T,
    pub username: String,
    counter: u64,
}

impl Discord {
    pub fn connect(client_id: &str) -> Result<Self, DiscordError> {
        let pipe = (0..10)
            .find_map(|i| {
                OpenOptions::new().read(true).write(true).open(format!(r"\\.\pipe\discord-ipc-{i}")).ok()
            })
            .ok_or(DiscordError::NotRunning)?;
        Self::handshake(pipe, client_id)
    }
}

impl<T: Read + Write> Discord<T> {
    fn handshake(pipe: T, client_id: &str) -> Result<Self, DiscordError> {
        let mut d = Self { pipe, username: String::new(), counter: 0 };
        d.send(OP_HANDSHAKE, &json!({ "v": 1, "client_id": client_id }))?;
        for _ in 0..MAX_SKIPPED_FRAMES {
            let msg = d.next_message()?;
            match msg["evt"].as_str() {
                Some("READY") => {
                    d.username = msg["data"]["user"]["username"].as_str().unwrap_or("unknown user").to_string();
                    return Ok(d);
                }
                Some("ERROR") => return Err(DiscordError::Rejected(error_message(&msg))),
                _ => {}
            }
        }
        Err(DiscordError::Disconnected("no ready message from Discord".into()))
    }

    /// `None` clears the activity.
    pub fn set_activity(&mut self, activity: Option<&Activity>) -> Result<(), DiscordError> {
        self.counter += 1;
        let pid = std::process::id();
        let nonce = format!("{pid}-{}", self.counter);
        let request = json!({
            "cmd": "SET_ACTIVITY",
            "args": { "pid": pid, "activity": activity.map(activity_json) },
            "nonce": nonce,
        });
        self.send(OP_FRAME, &request)?;
        for _ in 0..MAX_SKIPPED_FRAMES {
            let msg = self.next_message()?;
            if msg["nonce"].as_str() != Some(&nonce) {
                continue;
            }
            return match msg["evt"].as_str() {
                Some("ERROR") => Err(DiscordError::Rejected(error_message(&msg))),
                _ => Ok(()),
            };
        }
        Err(DiscordError::Disconnected("no reply from Discord".into()))
    }

    fn send(&mut self, op: u32, payload: &Value) -> Result<(), DiscordError> {
        let bytes = serde_json::to_vec(payload).map_err(|e| DiscordError::Rejected(e.to_string()))?;
        self.pipe.write_all(&encode_frame(op, &bytes))?;
        self.pipe.flush()?;
        Ok(())
    }

    /// The next JSON message, answering pings along the way. A close frame ends the connection.
    fn next_message(&mut self) -> Result<Value, DiscordError> {
        loop {
            let (op, payload) = read_frame(&mut self.pipe)?;
            match op {
                OP_CLOSE => {
                    let msg: Value = serde_json::from_slice(&payload).unwrap_or(Value::Null);
                    return Err(DiscordError::Disconnected(error_message(&msg)));
                }
                OP_PING => {
                    self.pipe.write_all(&encode_frame(OP_PONG, &payload))?;
                    self.pipe.flush()?;
                }
                OP_PONG => {}
                _ => {
                    if let Ok(v) = serde_json::from_slice(&payload) {
                        return Ok(v);
                    }
                }
            }
        }
    }
}

fn error_message(msg: &Value) -> String {
    msg["message"]
        .as_str()
        .or_else(|| msg["data"]["message"].as_str())
        .map_or_else(|| "closed by Discord".to_string(), str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct Mock {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Mock {
        fn replying(frames: &[(u32, Value)]) -> Self {
            let bytes = frames.iter().flat_map(|(op, v)| encode_frame(*op, v.to_string().as_bytes())).collect();
            Self { input: Cursor::new(bytes), output: Vec::new() }
        }
    }

    impl Read for Mock {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Mock {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn ready() -> (u32, Value) {
        (OP_FRAME, json!({ "cmd": "DISPATCH", "evt": "READY", "data": { "user": { "username": "sam" } } }))
    }

    fn reply_to(counter: u64, evt: Value) -> (u32, Value) {
        (OP_FRAME, json!({ "cmd": "SET_ACTIVITY", "evt": evt, "nonce": format!("{}-{counter}", std::process::id()) }))
    }

    #[test]
    fn frame_round_trip() {
        let bytes = encode_frame(1, br#"{"a":1}"#);
        assert_eq!(&bytes[..8], &[1, 0, 0, 0, 7, 0, 0, 0]);
        let (op, payload) = read_frame(&mut Cursor::new(bytes)).unwrap();
        assert_eq!(op, 1);
        assert_eq!(payload, br#"{"a":1}"#);
    }

    #[test]
    fn empty_frame() {
        let (op, payload) = read_frame(&mut Cursor::new(encode_frame(4, b""))).unwrap();
        assert_eq!((op, payload.len()), (4, 0));
    }

    #[test]
    fn truncated_and_oversized_frames_are_errors() {
        let mut bytes = encode_frame(1, b"hello");
        bytes.truncate(10);
        assert!(read_frame(&mut Cursor::new(bytes)).is_err());

        let mut huge = vec![1, 0, 0, 0];
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(read_frame(&mut Cursor::new(huge)).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn handshake_reads_username_and_sends_client_id() {
        let d = Discord::handshake(Mock::replying(&[ready()]), "123").unwrap();
        assert_eq!(d.username, "sam");
        let (op, payload) = read_frame(&mut Cursor::new(d.pipe.output)).unwrap();
        assert_eq!(op, OP_HANDSHAKE);
        let sent: Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(sent, json!({ "v": 1, "client_id": "123" }));
    }

    #[test]
    fn handshake_close_frame_is_an_error() {
        let close = (OP_CLOSE, json!({ "code": 4000, "message": "Invalid Client ID" }));
        let err = Discord::handshake(Mock::replying(&[close]), "x").err().unwrap();
        assert!(err.to_string().contains("Invalid Client ID"));
    }

    #[test]
    fn set_activity_sends_payload_and_answers_pings() {
        let frames = [ready(), (OP_PING, json!({ "n": 1 })), reply_to(1, Value::Null)];
        let mut d = Discord::handshake(Mock::replying(&frames), "1").unwrap();
        let activity = Activity { name: "Forza", state: "on PC", start_ms: 5, image: None };
        d.set_activity(Some(&activity)).unwrap();

        let mut out = Cursor::new(d.pipe.output.clone());
        read_frame(&mut out).unwrap(); // handshake
        let (op, payload) = read_frame(&mut out).unwrap();
        assert_eq!(op, OP_FRAME);
        let sent: Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(sent["cmd"], "SET_ACTIVITY");
        assert_eq!(sent["args"]["activity"]["details"], "Forza");
        let (op, _) = read_frame(&mut out).unwrap();
        assert_eq!(op, OP_PONG);
    }

    #[test]
    fn set_activity_error_event_is_rejected() {
        let frames = [ready(), reply_to(1, json!("ERROR"))];
        let mut d = Discord::handshake(Mock::replying(&frames), "1").unwrap();
        assert!(matches!(d.set_activity(None), Err(DiscordError::Rejected(_))));
    }

    #[test]
    fn dead_pipe_is_a_disconnect() {
        let mut d = Discord::handshake(Mock::replying(&[ready()]), "1").unwrap();
        assert!(matches!(d.set_activity(None), Err(DiscordError::Disconnected(_))));
    }

    #[test]
    fn clearing_sends_null_activity() {
        let frames = [ready(), reply_to(1, Value::Null)];
        let mut d = Discord::handshake(Mock::replying(&frames), "1").unwrap();
        d.set_activity(None).unwrap();
        let mut out = Cursor::new(d.pipe.output.clone());
        read_frame(&mut out).unwrap();
        let sent: Value = serde_json::from_slice(&read_frame(&mut out).unwrap().1).unwrap();
        assert!(sent["args"]["activity"].is_null());
    }

    #[test]
    fn activity_json_omits_missing_image() {
        let plain = activity_json(&Activity { name: "Halo", state: "on Xbox One", start_ms: 1000, image: None });
        assert_eq!(
            plain,
            json!({
                "type": 0, "details": "Halo", "state": "on Xbox One",
                "timestamps": { "start": 1000 }, "status_display_type": 2,
            })
        );
        let with = activity_json(&Activity { name: "Halo", state: "x", start_ms: 1, image: Some("http://i/p.png") });
        assert_eq!(with["assets"], json!({ "large_image": "http://i/p.png", "large_text": "Halo" }));
    }
}
