//! Bounded frame buffer used while no client is attached.
//!
//! Frames are JSON-RPC lines. Notifications are expendable progress chatter; anything carrying
//! an `"id"` (an agent-to-client request such as a permission prompt, or a response) is not, so
//! eviction only ever drops the oldest notification.

use std::collections::VecDeque;

/// What a [`FrameBuffer::push`] did, so the caller can log without the buffer owning logging.
#[derive(Debug, PartialEq, Eq)]
pub enum PushOutcome {
    /// Stored within the limit.
    Stored,
    /// Stored after dropping the oldest notification to stay within the limit.
    EvictedNotification,
    /// Stored past the limit because only id-bearing frames remain. `first` is true once.
    OverLimit { first: bool },
}

struct Frame {
    text: String,
    droppable: bool,
}

/// FIFO of frames capped at `limit`, with the id-aware eviction rule.
pub struct FrameBuffer {
    frames: VecDeque<Frame>,
    limit: usize,
    warned: bool,
}

/// True for a JSON-RPC notification: has `"method"`, has no `"id"`. Unparseable lines are kept.
fn is_notification(line: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(line) {
        Ok(serde_json::Value::Object(map)) => map.contains_key("method") && !map.contains_key("id"),
        _ => false,
    }
}

impl FrameBuffer {
    /// Creates an empty buffer holding up to `limit` frames before it starts evicting.
    pub fn new(limit: usize) -> Self {
        Self {
            frames: VecDeque::new(),
            limit,
            warned: false,
        }
    }

    /// Number of buffered frames.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// True when nothing is buffered.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Appends a frame, evicting the oldest notification first when the buffer is full.
    pub fn push(&mut self, text: String) -> PushOutcome {
        let outcome = if self.frames.len() >= self.limit {
            self.make_room()
        } else {
            PushOutcome::Stored
        };
        let droppable = is_notification(&text);
        self.frames.push_back(Frame { text, droppable });
        outcome
    }

    /// Evicts the oldest notification, or reports the over-limit state when none exists.
    fn make_room(&mut self) -> PushOutcome {
        if let Some(pos) = self.frames.iter().position(|f| f.droppable) {
            self.frames.remove(pos);
            return PushOutcome::EvictedNotification;
        }
        let first = !self.warned;
        self.warned = true;
        PushOutcome::OverLimit { first }
    }

    /// Removes and returns the oldest frame (the next to deliver), re-arming the over-limit warning
    /// once the buffer is back under its limit.
    pub fn pop_front(&mut self) -> Option<String> {
        let frame = self.frames.pop_front().map(|f| f.text);
        if self.frames.len() < self.limit {
            self.warned = false;
        }
        frame
    }

    /// Puts back a frame that was popped but not delivered, ahead of everything else.
    /// Never evicts: the frame was already inside the limit when it was popped.
    pub fn push_front(&mut self, text: String) {
        let droppable = is_notification(&text);
        self.frames.push_front(Frame { text, droppable });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(buf: &mut FrameBuffer) -> Vec<String> {
        std::iter::from_fn(|| buf.pop_front()).collect()
    }

    const NOTE: &str = r#"{"jsonrpc":"2.0","method":"session/update","params":{}}"#;
    const REQUEST: &str = r#"{"jsonrpc":"2.0","id":"p1","method":"session/request_permission"}"#;
    const RESPONSE: &str = r#"{"jsonrpc":"2.0","id":3,"result":{}}"#;

    fn note(n: usize) -> String {
        format!(r#"{{"jsonrpc":"2.0","method":"n{n}"}}"#)
    }

    #[test]
    fn drains_in_fifo_order() {
        let mut buf = FrameBuffer::new(10);
        for n in 0..3 {
            buf.push(note(n));
        }
        assert_eq!(drain(&mut buf), vec![note(0), note(1), note(2)]);
        assert!(buf.is_empty());
    }

    #[test]
    fn push_front_restores_head_order() {
        let mut buf = FrameBuffer::new(10);
        buf.push(note(1));
        buf.push(note(2));
        let head = buf.pop_front().unwrap();
        buf.push_front(head);
        assert_eq!(drain(&mut buf), vec![note(1), note(2)]);
    }

    #[test]
    fn evicts_oldest_notification_first() {
        let mut buf = FrameBuffer::new(3);
        buf.push(REQUEST.into());
        buf.push(note(1));
        buf.push(note(2));
        assert_eq!(buf.push(note(3)), PushOutcome::EvictedNotification);
        assert_eq!(drain(&mut buf), vec![REQUEST.to_string(), note(2), note(3)]);
    }

    #[test]
    fn never_drops_id_bearing_frames() {
        let mut buf = FrameBuffer::new(2);
        buf.push(REQUEST.into());
        buf.push(RESPONSE.into());
        buf.push(NOTE.into());
        buf.push(note(9));
        let frames = drain(&mut buf);
        assert!(frames.contains(&REQUEST.to_string()));
        assert!(frames.contains(&RESPONSE.to_string()));
    }

    #[test]
    fn grows_past_limit_and_warns_once() {
        let mut buf = FrameBuffer::new(2);
        buf.push(REQUEST.into());
        buf.push(RESPONSE.into());
        assert_eq!(
            buf.push(REQUEST.into()),
            PushOutcome::OverLimit { first: true }
        );
        assert_eq!(
            buf.push(RESPONSE.into()),
            PushOutcome::OverLimit { first: false }
        );
        assert_eq!(buf.len(), 4);
    }

    #[test]
    fn unparseable_lines_are_kept() {
        let mut buf = FrameBuffer::new(1);
        buf.push("not json".into());
        assert_eq!(
            buf.push(NOTE.into()),
            PushOutcome::OverLimit { first: true }
        );
    }
}
