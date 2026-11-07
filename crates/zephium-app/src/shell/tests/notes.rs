use super::*;

use zephium_core::notes::{NoteCall, NoteDone, NoteError, NoteReply, NoteResponse};

#[derive(Default)]
struct FakeNotes {
    calls: Mutex<Vec<ProfileId>>,
    released: Mutex<Vec<ProfileId>>,
}

impl zephium_core::ports::notes::Notes for FakeNotes {
    fn call(&self, profile: ProfileId, _call: NoteCall, done: NoteDone) {
        self.calls.lock().unwrap().push(profile);
        done(NoteResponse::Done);
    }

    fn release(&self, profile: ProfileId, done: Box<dyn FnOnce() + Send>) -> bool {
        self.released.lock().unwrap().push(profile);
        done();
        true
    }
}

type Answer = Arc<Mutex<Option<NoteReply>>>;

fn dispatch(shell: &mut Shell, profile: ProfileId) -> Answer {
    let slot: Answer = Arc::new(Mutex::new(None));
    let sink = slot.clone();
    shell.handle(Command::NoteCall {
        expected_profile: profile,
        call: Arc::new(NoteCall::Reveal { id: None }),
        done: crate::api::NoteCompletion::new(move |reply| {
            *sink.lock().unwrap() = Some(reply);
        }),
    });
    slot
}

fn unavailable(answer: &Answer) -> bool {
    matches!(
        answer.lock().unwrap().take(),
        Some(NoteReply {
            profile: None,
            response: NoteResponse::Error {
                error: NoteError::Unavailable
            }
        })
    )
}

#[test]
fn notes_answer_only_the_focused_persistent_profile_once_attached() {
    let (mut shell, _engine, _screen) = setup();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;

    assert!(unavailable(&dispatch(&mut shell, profile)));

    let notes = Arc::new(FakeNotes::default());
    shell.handle(Command::AttachNotes(crate::api::NotesAttachment(
        notes.clone(),
    )));
    let answer = dispatch(&mut shell, profile);
    assert!(matches!(
        answer.lock().unwrap().take(),
        Some(NoteReply { profile: Some(answered), response: NoteResponse::Done })
            if answered == profile.to_string()
    ));
    assert!(unavailable(&dispatch(&mut shell, ProfileId::from(9_999))));

    let mut private = shell.profiles.remove(profile).unwrap();
    private.kind = ProfileKind::Incognito;
    assert!(shell.profiles.insert(private));
    assert!(unavailable(&dispatch(&mut shell, profile)));
    assert_eq!(*notes.calls.lock().unwrap(), vec![profile]);
}
