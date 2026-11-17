//! One conversation: the person's messages and the assistant's replies, and every
//! change to them as numbered events. The snapshot the panel reads and the
//! form that is stored are the same value.
use crate::ipc::{
    PageAssistAttachmentV1 as Attachment, PageAssistBlockV1 as Block,
    PageAssistChangedV1 as Changed, PageAssistConversationV1 as View, PageAssistEventV1 as Event,
    PageAssistMessageV1 as Message, PageAssistReplyStateV1 as ReplyState,
    PageAssistSuggestionV1 as Suggestion,
};
use crate::text;

/// Most messages kept; older ones leave in pairs.
pub const MAX_MESSAGES: usize = 160;
/// Most blocks one reply holds.
pub const MAX_BLOCKS: usize = 24;
/// Longest text block, in bytes.
pub const MAX_TEXT_BYTES: usize = 24 * 1024;
/// Longest title, in characters.
pub const MAX_TITLE_CHARS: usize = 60;

/// A conversation and its event counter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Conversation {
    space: String,
    view: View,
    next: u32,
}

impl Conversation {
    /// An empty conversation in `space`.
    pub fn new(id: String, space: String) -> Self {
        Self {
            space,
            view: View {
                id,
                title: None,
                messages: Vec::new(),
                seq: 0,
            },
            next: 1,
        }
    }

    /// A stored conversation, opened again: nothing in it is still running,
    /// and no earlier reply jumps to a page any more.
    pub fn restore(mut view: View, space: String) -> Self {
        let mut next = 1;
        for message in &mut view.messages {
            match message {
                Message::Person { id, .. } => next = next.max(*id + 1),
                Message::Reply {
                    id,
                    state,
                    phase,
                    inert,
                    ..
                } => {
                    next = next.max(*id + 1);
                    *phase = None;
                    *inert = true;
                    if matches!(
                        state,
                        ReplyState::Working | ReplyState::Waiting | ReplyState::Paused
                    ) {
                        *state = ReplyState::Cancelled;
                    }
                }
            }
        }
        Self { space, view, next }
    }

    /// Its id.
    pub fn id(&self) -> &str {
        &self.view.id
    }

    /// The Space it belongs to.
    pub fn space(&self) -> &str {
        &self.space
    }

    /// The snapshot.
    pub fn view(&self) -> &View {
        &self.view
    }

    /// Whether nothing was said yet.
    pub fn is_empty(&self) -> bool {
        self.view.messages.is_empty()
    }

    /// The messages, oldest first.
    pub fn messages(&self) -> &[Message] {
        &self.view.messages
    }

    /// One reply's blocks.
    pub fn blocks(&self, reply: u32) -> Option<&[Block]> {
        self.view.messages.iter().find_map(|message| match message {
            Message::Reply { id, blocks, .. } if *id == reply => Some(blocks.as_slice()),
            _ => None,
        })
    }

    /// The latest reply's id.
    pub fn latest_reply(&self) -> Option<u32> {
        self.view
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::Reply { id, .. } => Some(*id),
                Message::Person { .. } => None,
            })
    }

    fn changed(&mut self, reply: u32, kind: Event) -> Changed {
        self.view.seq = self.view.seq.wrapping_add(1);
        Changed {
            space: self.space.clone(),
            conversation: self.view.id.clone(),
            reply,
            seq: self.view.seq,
            kind,
        }
    }

    fn trim(&mut self) {
        while self.view.messages.len() > MAX_MESSAGES {
            self.view.messages.remove(0);
            if matches!(self.view.messages.first(), Some(Message::Reply { .. })) {
                self.view.messages.remove(0);
            }
        }
    }

    /// The person's message joins; the first one names the conversation.
    pub fn say(
        &mut self,
        words: &str,
        suggestion: Option<Suggestion>,
        attachments: Vec<Attachment>,
        title: Option<&str>,
    ) -> Vec<Changed> {
        let id = self.next;
        self.next += 1;
        let message = Message::Person {
            id,
            text: words.to_owned(),
            suggestion,
            attachments,
        };
        self.view.messages.push(message.clone());
        self.trim();
        let mut events = vec![self.changed(
            id,
            Event::Message {
                message: Box::new(message),
            },
        )];
        if self.view.title.is_none() {
            let title = text::bounded(&text::clean(title.unwrap_or(words)), MAX_TITLE_CHARS);
            if !title.is_empty() {
                self.view.title = Some(title.clone());
                events.push(self.changed(0, Event::Titled { title }));
            }
        }
        events
    }

    /// A reply begins, working.
    pub fn begin_reply(&mut self) -> (u32, Changed) {
        let id = self.next;
        self.next += 1;
        let message = Message::Reply {
            id,
            state: ReplyState::Working,
            phase: None,
            blocks: Vec::new(),
            cost: None,
            fault: None,
            inert: false,
            end: None,
        };
        self.view.messages.push(message.clone());
        self.trim();
        let event = self.changed(
            id,
            Event::Message {
                message: Box::new(message),
            },
        );
        (id, event)
    }

    /// Folds `kind` into reply `reply`; none when it does not apply.
    pub fn apply(&mut self, reply: u32, kind: Event) -> Option<Changed> {
        if let Event::Inert = kind {
            let mut changed = false;
            for message in &mut self.view.messages {
                if let Message::Reply { id, inert, .. } = message {
                    if *id <= reply && !*inert {
                        *inert = true;
                        changed = true;
                    }
                }
            }
            return changed.then(|| self.changed(reply, kind));
        }
        let message = self
            .view
            .messages
            .iter_mut()
            .find(|message| matches!(message, Message::Reply { id, .. } if *id == reply))?;
        let Message::Reply {
            state,
            phase,
            blocks,
            cost,
            fault,
            end,
            ..
        } = message
        else {
            return None;
        };
        match &kind {
            Event::Phase { phase: next } => *phase = next.clone(),
            Event::Text { block, delta } => {
                let index = usize::from(*block);
                if index == blocks.len() && index < MAX_BLOCKS {
                    blocks.push(Block::Text {
                        text: String::new(),
                        cites: Vec::new(),
                    });
                }
                let Some(Block::Text { text, .. }) = blocks.get_mut(index) else {
                    return None;
                };
                if text.len() + delta.len() > MAX_TEXT_BYTES {
                    return None;
                }
                text.push_str(delta);
            }
            Event::Cites { block, cites: next } => match blocks.get_mut(usize::from(*block)) {
                Some(Block::Text { cites, .. }) => *cites = next.clone(),
                _ => return None,
            },
            Event::Block { block, value } => {
                let index = usize::from(*block);
                if matches!(value, Block::Text { .. }) {
                    return None;
                }
                match index.cmp(&blocks.len()) {
                    std::cmp::Ordering::Less => blocks[index] = value.clone(),
                    std::cmp::Ordering::Equal if index < MAX_BLOCKS => blocks.push(value.clone()),
                    _ => return None,
                }
            }
            Event::State { state: next } => *state = *next,
            Event::Done {
                cost: next,
                end: ended,
            } => {
                *state = ReplyState::Done;
                *phase = None;
                *cost = next.clone();
                *end = *ended;
            }
            Event::Cost { cost: next } => *cost = Some(next.clone()),
            Event::Failed { fault: next } => {
                *state = ReplyState::Failed;
                *phase = None;
                *fault = Some(*next);
            }
            Event::Cancelled => {
                *state = ReplyState::Cancelled;
                *phase = None;
            }
            Event::Message { .. } | Event::Titled { .. } | Event::Invalidated | Event::Inert => {
                return None;
            }
        }
        Some(self.changed(reply, kind))
    }

    /// The next block index of `reply`.
    pub fn next_block(&self, reply: u32) -> u16 {
        self.blocks(reply)
            .map_or(0, |blocks| u16::try_from(blocks.len()).unwrap_or(u16::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{PageAssistCostV1 as Cost, PageAssistFaultV1 as Fault};

    fn conversation() -> Conversation {
        Conversation::new("c1".into(), "space".into())
    }

    #[test]
    fn a_message_and_its_reply_fold_their_events_in_order() {
        let mut c = conversation();
        let said = c.say("What's worth reading?", None, Vec::new(), None);
        assert_eq!(said.len(), 2, "the first message names the conversation");
        assert_eq!(c.view().title.as_deref(), Some("What's worth reading?"));
        let (reply, started) = c.begin_reply();
        assert_eq!(started.seq, 3);
        let text = |delta: &str| Event::Text {
            block: 0,
            delta: delta.into(),
        };
        assert!(c.apply(reply, text("Three ")).is_some());
        assert!(c.apply(reply, text("stand out")).is_some());
        assert!(c
            .apply(
                reply,
                Event::Done {
                    cost: Some(Cost::Local { model: "m".into() }),
                    end: Some(crate::ipc::PageAssistEndV1::Asked),
                }
            )
            .is_some());
        let Message::Reply {
            blocks, state, end, ..
        } = &c.messages()[1]
        else {
            panic!()
        };
        assert_eq!(*state, ReplyState::Done);
        assert_eq!(*end, Some(crate::ipc::PageAssistEndV1::Asked));
        assert_eq!(
            blocks[0],
            Block::Text {
                text: "Three stand out".into(),
                cites: vec![]
            }
        );
        assert_eq!(c.view().seq, 6);
        assert_eq!(c.say("Open all three", None, Vec::new(), None).len(), 1);
    }

    #[test]
    fn blocks_only_append_in_order_and_text_stays_text() {
        let mut c = conversation();
        let (reply, _) = c.begin_reply();
        let gap = Event::Text {
            block: 3,
            delta: "x".into(),
        };
        assert!(c.apply(reply, gap).is_none());
        let paused = Event::Block {
            block: 0,
            value: Block::Paused {
                reason: crate::ipc::PageAssistPauseV1::TookOver,
            },
        };
        assert!(c.apply(reply, paused).is_some());
        let into_paused = Event::Text {
            block: 0,
            delta: "x".into(),
        };
        assert!(c.apply(reply, into_paused).is_none());
        let forged = Event::Block {
            block: 1,
            value: Block::Text {
                text: "x".into(),
                cites: vec![],
            },
        };
        assert!(c.apply(reply, forged).is_none());
        assert!(c.apply(99, Event::Cancelled).is_none());
    }

    #[test]
    fn restore_leaves_nothing_running_and_every_reply_inert() {
        let mut c = conversation();
        c.say("hi", None, Vec::new(), None);
        let (reply, _) = c.begin_reply();
        c.apply(
            reply,
            Event::Failed {
                fault: Fault::NoAnswer,
            },
        );
        let (_, _) = c.begin_reply();
        let restored = Conversation::restore(c.view().clone(), "space".into());
        assert!(restored.messages().iter().all(|message| match message {
            Message::Reply { state, inert, .. } => *inert && !matches!(state, ReplyState::Working),
            Message::Person { .. } => true,
        }));
        let mut restored = restored;
        let (next, _) = restored.begin_reply();
        assert_eq!(next, 4);
    }

    #[test]
    fn old_messages_leave_in_pairs() {
        let mut c = conversation();
        for _ in 0..MAX_MESSAGES {
            c.say("q", None, Vec::new(), None);
            c.begin_reply();
        }
        assert!(c.messages().len() <= MAX_MESSAGES);
        assert!(matches!(c.messages()[0], Message::Person { .. }));
    }

    #[test]
    fn inert_marks_earlier_replies_once() {
        let mut c = conversation();
        let (first, _) = c.begin_reply();
        let (second, _) = c.begin_reply();
        assert!(c.apply(first, Event::Inert).is_some());
        assert!(c.apply(first, Event::Inert).is_none());
        assert!(c.apply(second, Event::Inert).is_some());
    }
}
