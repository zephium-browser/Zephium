//! Posts that arrived before the frame records they claim. The module sends
//! frame records in batches, so a document's first posts usually wait here.

use std::collections::VecDeque;

use zephium_webkit_web_process_protocol::protocol::Post;

use super::ReplyToken;

/// Posts one view may hold.
pub const MAX_BUFFERED_POSTS_PER_VIEW: usize = 64;
/// Payload bytes one view may hold.
pub const MAX_BUFFERED_BYTES_PER_VIEW: usize = 16 * 1024 * 1024;
/// Payload bytes one profile may hold across its views.
pub const MAX_BUFFERED_BYTES_PER_PROFILE: usize = 64 * 1024 * 1024;
/// How long a post may wait for its frame record.
pub const BUFFER_TIMEOUT_MS: u64 = 2_000;

#[derive(Debug)]
pub(crate) struct Buffered {
    pub post: Post,
    pub reply: ReplyToken,
    pub deadline_ms: u64,
}

#[derive(Debug, Default)]
pub(crate) struct PostBuffer {
    entries: VecDeque<Buffered>,
    bytes: usize,
}

impl PostBuffer {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Holds `entry` if both this view and the profile (`profile_bytes`
    /// held elsewhere) stay within bounds; otherwise hands it back.
    pub fn push(&mut self, entry: Buffered, profile_bytes: usize) -> Result<(), Buffered> {
        let size = entry.post.bytes.len();
        if self.entries.len() >= MAX_BUFFERED_POSTS_PER_VIEW
            || self.bytes.saturating_add(size) > MAX_BUFFERED_BYTES_PER_VIEW
            || profile_bytes.saturating_add(size) > MAX_BUFFERED_BYTES_PER_PROFILE
        {
            return Err(entry);
        }
        self.bytes += size;
        self.entries.push_back(entry);
        Ok(())
    }

    /// Everything held, oldest first, for re-evaluation.
    pub fn take(&mut self) -> Vec<Buffered> {
        self.bytes = 0;
        self.entries.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(size: usize) -> Buffered {
        Buffered {
            post: Post {
                generation: 1,
                post_id: 1,
                frame_id: 1,
                document_seq: 1,
                world: String::new(),
                uri: String::new(),
                bytes: vec![0; size],
            },
            reply: ReplyToken(1),
            deadline_ms: 0,
        }
    }

    #[test]
    fn count_and_bytes_are_bounded_per_view_and_profile() {
        let mut buffer = PostBuffer::default();
        for _ in 0..MAX_BUFFERED_POSTS_PER_VIEW {
            buffer.push(entry(1), 0).expect("room");
        }
        assert!(buffer.push(entry(1), 0).is_err());
        assert_eq!(buffer.take().len(), MAX_BUFFERED_POSTS_PER_VIEW);

        buffer
            .push(entry(MAX_BUFFERED_BYTES_PER_VIEW), 0)
            .expect("exactly the view budget");
        assert!(buffer.push(entry(1), 0).is_err());
        buffer.take();
        assert!(buffer
            .push(entry(2), MAX_BUFFERED_BYTES_PER_PROFILE - 1)
            .is_err());
        assert!(buffer.is_empty());
    }
}
