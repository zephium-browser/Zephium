//! The notes folder service, as the shell sees it.
use std::sync::Arc;

use crate::ids::ProfileId;
use crate::notes::{NoteCall, NoteDone};

pub trait Notes: Send + Sync {
    /// Bounded asynchronous access to a profile's notes. Caller owns authorization.
    fn call(&self, profile: ProfileId, call: NoteCall, done: NoteDone);
    /// Nonblocking lifecycle admission, independent of ordinary call capacity.
    /// `true` reserves a drain; its callback proves the profile can never write
    /// again in this process. `false` requires retry and never calls `done`.
    fn release(&self, profile: ProfileId, done: Box<dyn FnOnce() + Send>) -> bool;
}

pub type SharedNotes = Arc<dyn Notes>;
