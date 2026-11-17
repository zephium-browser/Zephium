//! Voice: what a spoken turn does, decided in one place. The session reducer
//! owns the key, microphone, transcript and speech rules; the cutter turns
//! reply text into speakable sentences; the phrase lists decide spoken
//! confirms; the closing line says only what typed step facts support.

pub mod closing;
pub mod cutter;
pub mod phrases;
pub mod session;

pub use closing::{closing, outcome, Closing, Outcome, ReplyFacts, StepFact, StepKind, StepRef};
pub use cutter::Cutter;
pub use phrases::{ack, decision, Ack, Language};
pub use session::{
    Config, Confirm, Earcon, Effect, Event, Fault, IdSource, Line, Press, Session, Speech, State,
    View, Work,
};
