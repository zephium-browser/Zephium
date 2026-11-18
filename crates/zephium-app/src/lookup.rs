//! Read-only lookups of the person's own tasks and notes by the words an
//! agent used for them. The stores narrow by each word; the title decides.

use std::sync::Arc;

use jiff::civil::Date;
use jiff::ToSpan;
use zephium_core::ids::ProfileId;
use zephium_core::notes::{NoteCall, NoteQuery, NoteReply, NoteResponse, NoteSummary};
use zephium_core::resources::{
    ResourceCall, ResourceReply, ResourceResponse, ResourceSummary, TaskMetadata, TaskQuery,
    TaskStatus, TaskView,
};

use crate::{reply, Command, Handle, NoteCompletion, ResourceCompletion};

/// Words weighed per lookup, and rows read back for each.
const MAX_WORDS: usize = 6;
const HITS_PER_WORD: u16 = 50;
/// What a store is asked for: enough of a word to reach every title that
/// shares its stem.
const NEEDLE_CHARS: usize = 4;
const IGNORED: &[&str] = &[
    "the", "and", "for", "with", "from", "that", "this", "into", "about", "your", "you", "our",
    "task", "tasks", "note", "notes", "todo",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskMatch {
    pub id: String,
    pub title: String,
    /// "Overdue", "Today", "Tomorrow", "This week", "Next week" or "Later";
    /// none for a task without a date.
    pub due: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteMatch {
    pub id: String,
    pub title: String,
}

fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The same word, allowing a short ending such as a plural or "-ing".
fn same_word(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (short, long) = if a.chars().count() <= b.chars().count() {
        (a, b)
    } else {
        (b, a)
    };
    short.chars().count() >= NEEDLE_CHARS
        && long.starts_with(short)
        && long.chars().count() - short.chars().count() <= 3
}

/// The words of a request that say anything: no filler, no repeats.
struct Words(Vec<String>);

impl Words {
    fn new(text: &str) -> Option<Self> {
        let mut words: Vec<String> = Vec::new();
        for token in tokens(text) {
            let says_something = (token.chars().count() >= 3
                || token.chars().any(|c| c.is_ascii_digit()))
                && !IGNORED.contains(&token.as_str());
            if says_something && !words.contains(&token) {
                words.push(token);
            }
        }
        words.truncate(MAX_WORDS);
        (!words.is_empty()).then_some(Self(words))
    }

    fn needles(&self) -> Vec<String> {
        let mut needles: Vec<String> = Vec::new();
        for word in &self.0 {
            let needle: String = word.chars().take(NEEDLE_CHARS).collect();
            if !needles.contains(&needle) {
                needles.push(needle);
            }
        }
        needles
    }

    /// How many of the words the title has, when that is at least half.
    fn score(&self, title: &str) -> Option<usize> {
        let title = tokens(title);
        let matched = self
            .0
            .iter()
            .filter(|word| title.iter().any(|token| same_word(word, token)))
            .count();
        (matched >= self.0.len().div_ceil(2)).then_some(matched)
    }
}

/// The day a task is next answerable for: its planned day or its deadline,
/// whichever comes first.
fn day_due<'a>(item: &'a ResourceSummary, meta: Option<&'a TaskMetadata>) -> Option<&'a str> {
    let planned = item.due_date.as_deref();
    let deadline = meta.and_then(|meta| meta.deadline.as_deref());
    match (planned, deadline) {
        (Some(planned), Some(deadline)) => Some(planned.min(deadline)),
        (planned, deadline) => planned.or(deadline),
    }
}

fn due_word(today: Date, due: &str) -> Option<String> {
    let due = Date::strptime("%Y-%m-%d", due).ok()?;
    let days = due.since(today).ok()?.get_days();
    let word = if days < 0 {
        "Overdue"
    } else if days == 0 {
        "Today"
    } else if days == 1 {
        "Tomorrow"
    } else if week(due) == week(today) {
        "This week"
    } else if week(due) == week(today.saturating_add(7.days())) {
        "Next week"
    } else {
        "Later"
    };
    Some(word.to_owned())
}

fn week(date: Date) -> (i16, i8) {
    let week = date.iso_week_date();
    (week.year(), week.week())
}

/// The open task whose title has the most of the words; of equals, the one
/// due soonest.
fn best_task(
    words: &Words,
    today: Date,
    items: &[ResourceSummary],
    metadata: &[TaskMetadata],
) -> Option<TaskMatch> {
    items
        .iter()
        .filter(|task| task.completed != Some(true) && task.status != Some(TaskStatus::Done))
        .filter_map(|task| {
            let meta = metadata.iter().find(|meta| meta.id == task.id);
            let due = day_due(task, meta);
            let key = (
                std::cmp::Reverse(words.score(&task.title)?),
                due.unwrap_or("9999-12-31"),
                task.id.as_str(),
            );
            Some((key, task, due))
        })
        .min_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, task, due)| TaskMatch {
            id: task.id.clone(),
            title: task.title.clone(),
            due: due.and_then(|due| due_word(today, due)),
        })
}

/// The note whose title has the most of the words; of equals, the one
/// changed last.
fn best_note(words: &Words, notes: &[NoteSummary]) -> Option<NoteMatch> {
    notes
        .iter()
        .filter(|note| !note.trashed)
        .filter_map(|note| {
            let key = (
                words.score(&note.title)?,
                note.modified_at.parse::<i64>().unwrap_or(0),
                std::cmp::Reverse(note.id.as_str()),
            );
            Some((key, note))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, note)| NoteMatch {
            id: note.id.clone(),
            title: note.title.clone(),
        })
}

impl Handle {
    /// The best open task whose title has at least half of `words`; of equal
    /// matches, the one due soonest. `None` when no task is a reasonable
    /// match or the profile is not the one in front. The wait is not
    /// bounded by time; callers that must not wait on a stalled store add
    /// their own timeout.
    pub async fn find_task(&self, profile: ProfileId, words: &str) -> Option<TaskMatch> {
        let words = Words::new(words)?;
        let today = jiff::Zoned::now().date();
        let asked: Vec<_> = words
            .needles()
            .into_iter()
            .map(|needle| {
                self.ask_resources(
                    profile,
                    ResourceCall::ListTasks {
                        query: TaskQuery {
                            list: None,
                            view: TaskView::All,
                            today: today.strftime("%Y-%m-%d").to_string(),
                            search: needle,
                            after: None,
                            limit: HITS_PER_WORD,
                        },
                    },
                )
            })
            .collect();
        let (mut items, mut metadata) = (Vec::new(), Vec::new());
        for answer in asked {
            if let Some(ResourceReply {
                profile: Some(owner),
                response:
                    ResourceResponse::TaskPage {
                        items: page,
                        metadata: meta,
                        ..
                    },
            }) = answer.await
            {
                if owner == profile.to_string() {
                    items.extend(page);
                    metadata.extend(meta);
                }
            }
        }
        best_task(&words, today, &items, &metadata)
    }

    /// The best note whose title has at least half of `words`; of equal
    /// matches, the one changed last. `None` when no note is a reasonable
    /// match or the profile is not the one in front. Unbounded in time like
    /// [`Self::find_task`].
    pub async fn find_note(&self, profile: ProfileId, words: &str) -> Option<NoteMatch> {
        let words = Words::new(words)?;
        let asked: Vec<_> = words
            .needles()
            .into_iter()
            .map(|needle| {
                self.ask_notes(
                    profile,
                    NoteCall::List {
                        query: NoteQuery {
                            search: needle,
                            trashed: false,
                            after: None,
                            limit: HITS_PER_WORD,
                        },
                    },
                )
            })
            .collect();
        let mut notes = Vec::new();
        for answer in asked {
            if let Some(NoteReply {
                profile: Some(owner),
                response: NoteResponse::Page { items, .. },
            }) = answer.await
            {
                if owner == profile.to_string() {
                    notes.extend(items);
                }
            }
        }
        best_note(&words, &notes)
    }

    // A refused command drops its completion, which resolves the reply.
    fn ask_resources(&self, profile: ProfileId, call: ResourceCall) -> reply::Reply<ResourceReply> {
        let (replier, answer) = reply::channel();
        let _ = self.queue.try_push(Command::ResourceCall {
            expected_profile: profile,
            call: Arc::new(call),
            done: ResourceCompletion::new(move |reply| replier.send(reply)),
        });
        answer
    }

    fn ask_notes(&self, profile: ProfileId, call: NoteCall) -> reply::Reply<NoteReply> {
        let (replier, answer) = reply::channel();
        let _ = self.queue.try_push(Command::NoteCall {
            expected_profile: profile,
            call: Arc::new(call),
            done: NoteCompletion::new(move |reply| replier.send(reply)),
        });
        answer
    }
}

#[cfg(test)]
pub(crate) fn task_row(
    id: &str,
    title: &str,
    due: Option<&str>,
    done: bool,
) -> (ResourceSummary, TaskMetadata) {
    (
        ResourceSummary {
            id: id.into(),
            revision: "r".into(),
            title: title.into(),
            pinned: false,
            updated_at: "0".into(),
            completed: Some(done),
            due_date: due.map(Into::into),
            due_time: None,
            status: Some(if done {
                TaskStatus::Done
            } else {
                TaskStatus::Open
            }),
            assignee: None,
            origin: None,
            context: None,
            sort_key: None,
            work: None,
        },
        TaskMetadata {
            id: id.into(),
            list: None,
            inbox: false,
            priority: Default::default(),
            steps: 0,
            steps_done: 0,
            completed_at: None,
            deadline: None,
            duration: None,
        },
    )
}

#[cfg(test)]
pub(crate) fn note_row(id: &str, title: &str, modified: i64) -> NoteSummary {
    NoteSummary {
        id: id.into(),
        revision: "0".repeat(32),
        title: title.into(),
        preview: String::new(),
        pinned: false,
        trashed: false,
        editable: true,
        created_at: "0".into(),
        modified_at: modified.to_string(),
        path: format!("{title}.md"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Words {
        Words::new(text).expect("words that say something")
    }

    fn day(text: &str) -> Date {
        Date::strptime("%Y-%m-%d", text).unwrap()
    }

    fn pick(
        text: &str,
        rows: Vec<(ResourceSummary, TaskMetadata)>,
    ) -> Option<(String, Option<String>)> {
        let (items, metadata): (Vec<_>, Vec<_>) = rows.into_iter().unzip();
        best_task(&words(text), day("2026-10-23"), &items, &metadata)
            .map(|found| (found.id, found.due))
    }

    #[test]
    fn only_the_words_that_say_something_count() {
        assert_eq!(words("Renew my passport").0, ["renew", "passport"]);
        assert_eq!(
            words("the NOTE about Q3 budget, budget").0,
            ["q3", "budget"]
        );
        assert_eq!(words("Réserver à Zürich").0, ["réserver", "zürich"]);
        assert!(Words::new("a to of the task").is_none());
        assert!(Words::new("  ").is_none());
        assert_eq!(words(&"word ".repeat(3)).0.len(), 1);
        let many: String = (0..20).map(|n| format!("word{n} ")).collect();
        assert_eq!(words(&many).0.len(), MAX_WORDS);
        assert_eq!(
            words("renewal renewing passport").needles(),
            ["rene", "pass"]
        );
    }

    #[test]
    fn a_title_matches_with_at_least_half_of_the_words_allowing_an_ending() {
        let both = words("renew passport");
        assert_eq!(both.score("Passport renewal"), Some(2));
        assert_eq!(both.score("Passports: renewing"), Some(2));
        assert_eq!(both.score("Passport photos"), Some(1));
        assert_eq!(both.score("Buy milk"), None);
        assert_eq!(both.score("Review the passage"), None);
        let three = words("renew passport photos");
        assert_eq!(three.score("Passport photos"), Some(2));
        assert_eq!(three.score("Passport"), None);
        assert!(same_word("book", "booking"));
        assert!(!same_word("tax", "taxes"));
        assert!(!same_word("port", "portugal"));
    }

    #[test]
    fn the_best_open_task_has_the_most_words_then_the_soonest_due() {
        let rows = vec![
            task_row("later", "Renew passport", Some("2026-11-20"), false),
            task_row("soon", "Renew passport", Some("2026-10-25"), false),
            task_row("undated", "Renew passport", None, false),
            task_row("both", "Renew passport and visa", Some("2026-12-01"), false),
            task_row("half", "Passport photos", Some("2026-10-01"), false),
        ];
        assert_eq!(
            pick("renew passport", rows.clone()),
            Some(("soon".into(), Some("This week".into())))
        );
        assert_eq!(
            pick("renew passport visa", rows.clone()),
            Some(("both".into(), Some("Later".into())))
        );
        assert_eq!(pick("buy milk", rows), None);
    }

    #[test]
    fn a_done_task_is_never_the_match() {
        let rows = vec![
            task_row("done", "Renew passport", Some("2026-10-20"), true),
            task_row("open", "Renew passport", None, false),
        ];
        assert_eq!(
            pick("renew passport", rows.clone()),
            Some(("open".into(), None))
        );
        assert_eq!(pick("renew passport", rows[..1].to_vec()), None);

        let mut finished = task_row("finished", "Renew passport", None, false);
        finished.0.completed = None;
        finished.0.status = Some(TaskStatus::Done);
        assert_eq!(pick("renew passport", vec![finished]), None);
    }

    #[test]
    fn a_deadline_before_the_planned_day_is_when_the_task_is_due() {
        let mut early = task_row("early", "Pay rent", Some("2026-11-05"), false);
        early.1.deadline = Some("2026-10-23".into());
        let planned = task_row("planned", "Pay rent", Some("2026-10-25"), false);
        assert_eq!(
            pick("pay rent", vec![planned, early]),
            Some(("early".into(), Some("Today".into())))
        );
    }

    #[test]
    fn due_dates_read_as_words() {
        let today = day("2026-10-23");
        let word = |due| due_word(today, due);
        assert_eq!(word("2026-10-22").as_deref(), Some("Overdue"));
        assert_eq!(word("2026-10-23").as_deref(), Some("Today"));
        assert_eq!(word("2026-10-24").as_deref(), Some("Tomorrow"));
        assert_eq!(word("2026-10-25").as_deref(), Some("This week"));
        assert_eq!(word("2026-10-26").as_deref(), Some("Next week"));
        assert_eq!(word("2026-11-01").as_deref(), Some("Next week"));
        assert_eq!(word("2026-11-02").as_deref(), Some("Later"));
        assert_eq!(word("someday"), None);
    }

    #[test]
    fn the_best_note_has_the_most_words_then_changed_last() {
        let mut trashed = note_row("trashed", "Trip to Lisbon", 9);
        trashed.trashed = true;
        let notes = [
            note_row("old", "Lisbon trip", 1),
            note_row("new", "Trip to Lisbon", 3),
            note_row("body", "Packing", 7),
            trashed,
            note_row("one", "Lisbon", 5),
        ];
        let found = |text: &str| best_note(&words(text), &notes).map(|note| note.id);
        assert_eq!(found("lisbon trip").as_deref(), Some("new"));
        assert_eq!(found("lisbon").as_deref(), Some("one"));
        assert_eq!(found("train timetable"), None);
    }
}
