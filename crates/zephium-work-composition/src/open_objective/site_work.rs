//! A page agent working on one site in the person's session: it navigates,
//! searches, filters, opens items and fills drafts. Rust classifies every
//! proposed step from the observed page. A step that would commit something
//! (send, post, pay, book, delete, share, save, submit) stops for the person
//! with a preview Rust builds from the page; only an approved step whose
//! target and preview are unchanged may then run, once. Credentials are
//! never typed.
use super::*;
use sha2::{Digest, Sha256};
use std::sync::Mutex;

/// What a held step commits; the person's card names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Consequence {
    Communication,
    Purchase,
    Destructive,
    Save,
    /// Typing into a document that saves as it is typed.
    Edit,
    /// Typing into any field of a site the person did not name, in a run
    /// that holds their own data: the text could carry it to that site.
    Type,
}

impl Consequence {
    pub(crate) const fn class(self) -> SemanticEffectClass {
        match self {
            Self::Communication => SemanticEffectClass::Communication,
            Self::Purchase => SemanticEffectClass::Purchase,
            Self::Destructive => SemanticEffectClass::Destructive,
            Self::Save | Self::Edit => SemanticEffectClass::ExternalWrite,
            Self::Type => SemanticEffectClass::LocalWrite,
        }
    }
    const fn declared(class: SemanticEffectClass) -> Option<Self> {
        match class {
            SemanticEffectClass::Communication => Some(Self::Communication),
            SemanticEffectClass::Purchase => Some(Self::Purchase),
            SemanticEffectClass::Destructive => Some(Self::Destructive),
            SemanticEffectClass::ExternalWrite => Some(Self::Save),
            _ => None,
        }
    }
}

/// What one proposed step would do, read from the page, never from the model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SiteEffect {
    Read,
    Draft,
    Commit(Consequence),
}

/// Words on a control or its region, by what they commit. Earlier lists win.
const PURCHASE_WORDS: [&str; 19] = [
    "buy",
    "purchase",
    "pay",
    "order",
    "book",
    "reserve",
    "subscribe",
    "donate",
    "bid",
    "request to book",
    "place order",
    "book now",
    "reserve now",
    "confirm and pay",
    "kup",
    "zapłać",
    "zamów",
    "zarezerwuj",
    "kupuję",
];
const DESTRUCTIVE_WORDS: [&str; 20] = [
    "delete",
    "remove",
    "archive",
    "trash",
    "erase",
    "unsubscribe",
    "leave",
    "deactivate",
    "disconnect",
    "revoke",
    "cancel subscription",
    "cancel order",
    "cancel booking",
    "cancel reservation",
    "cancel plan",
    "delete forever",
    "usuń",
    "anuluj subskrypcję",
    "opuść",
    "wyrzuć",
];
const COMMUNICATION_WORDS: [&str; 22] = [
    "send",
    "post",
    "publish",
    "reply",
    "comment",
    "share",
    "invite",
    "tweet",
    "join",
    "follow",
    "like",
    "react",
    "accept",
    "decline",
    "approve",
    "reject",
    "rsvp",
    "message",
    "wyślij",
    "opublikuj",
    "odpowiedz",
    "udostępnij",
];
const SAVE_WORDS: [&str; 16] = [
    "submit",
    "save",
    "update",
    "confirm",
    "transfer",
    "apply changes",
    "sign up",
    "change password",
    "change email",
    "sign out",
    "log out",
    "logout",
    "signout",
    "zapisz",
    "potwierdź",
    "wyloguj",
];
/// Controls that close or step back and so commit nothing.
const DISMISS_WORDS: [&str; 8] = [
    "close",
    "dismiss",
    "cancel",
    "not now",
    "back",
    "later",
    "skip",
    "no thanks",
];
const SIGN_IN_PHRASES: [&str; 8] = [
    "sign in",
    "log in",
    "login",
    "signin",
    "continue with google",
    "continue with apple",
    "zaloguj",
    "sign in with",
];
const CONFIRM_CONTEXT: [&str; 5] = [
    "are you sure",
    "cannot be undone",
    "confirm",
    "permanently",
    "czy na pewno",
];

fn words(text: &str) -> String {
    let words: Vec<_> = text
        .to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect();
    format!(" {} ", words.join(" "))
}

fn names_any(text: &str, phrases: &[&str]) -> bool {
    let text = words(text);
    phrases
        .iter()
        .any(|phrase| text.contains(&format!(" {} ", words(phrase).trim())))
}

fn label(node: &SemanticNode) -> String {
    let mut label = String::new();
    for part in node.name().into_iter().chain(node.text()) {
        label.push(' ');
        label.push_str(part.as_str());
    }
    label
}

fn ancestors<'a>(
    node: &SemanticNode,
    snapshot: &'a SemanticSnapshot,
) -> impl Iterator<Item = (usize, &'a SemanticNode)> {
    let mut parent = node.parent();
    std::iter::from_fn(move || {
        let index = usize::from(parent?);
        let ancestor = snapshot.nodes().get(index)?;
        parent = ancestor.parent();
        Some((index, ancestor))
    })
}

/// The nodes of the subtree rooted at `index`.
fn subtree(snapshot: &SemanticSnapshot, index: usize) -> &[SemanticNode] {
    let Some(root) = snapshot.nodes().get(index) else {
        return &[];
    };
    let end = snapshot
        .nodes()
        .iter()
        .enumerate()
        .skip(index + 1)
        .find(|(_, next)| next.depth() <= root.depth())
        .map_or(snapshot.nodes().len(), |(end, _)| end);
    &snapshot.nodes()[index..end]
}

/// Words in a status line that say something was just committed.
const DONE_WORDS: [&str; 18] = [
    "sent",
    "posted",
    "published",
    "booked",
    "ordered",
    "purchased",
    "paid",
    "deleted",
    "removed",
    "saved",
    "submitted",
    "confirmed",
    "reserved",
    "thank you for your order",
    "wysłano",
    "zapisano",
    "usunięto",
    "opublikowano",
];

fn commit_words(text: &str) -> Option<Consequence> {
    [
        (&PURCHASE_WORDS[..], Consequence::Purchase),
        (&DESTRUCTIVE_WORDS[..], Consequence::Destructive),
        (&COMMUNICATION_WORDS[..], Consequence::Communication),
        (&SAVE_WORDS[..], Consequence::Save),
    ]
    .into_iter()
    .find(|(words, _)| names_any(text, words))
    .map(|(_, consequence)| consequence)
}

/// A form the browser reports as a search, or a same-origin GET form: its
/// submission is a query, not a commit.
fn queries(node: &SemanticNode) -> bool {
    node.form().is_some_and(|form| {
        form.search() || (form.method() == SemanticFormMethod::Get && form.same_origin())
    })
}

fn in_search(node: &SemanticNode, snapshot: &SemanticSnapshot) -> bool {
    node.role() == SemanticRole::Searchbox
        || node.form().is_some_and(|form| form.search())
        || ancestors(node, snapshot).any(|(index, ancestor)| {
            ancestor.landmark_kind() == Some(SemanticLandmarkKind::Search)
                || (ancestor.landmark_kind() == Some(SemanticLandmarkKind::Form)
                    && subtree(snapshot, index)
                        .iter()
                        .any(|part| part.role() == SemanticRole::Searchbox))
        })
}

/// The region a step acts in: its dialog, form or named region, else the page.
fn region_index(node: &SemanticNode, snapshot: &SemanticSnapshot) -> usize {
    ancestors(node, snapshot)
        .find(|(_, ancestor)| {
            ancestor.role() == SemanticRole::Dialog
                || matches!(
                    ancestor.landmark_kind(),
                    Some(SemanticLandmarkKind::Form | SemanticLandmarkKind::Region)
                )
        })
        .or_else(|| {
            ancestors(node, snapshot)
                .find(|(_, ancestor)| ancestor.landmark_kind() == Some(SemanticLandmarkKind::Main))
        })
        .map_or(0, |(index, _)| index)
}

/// A message box ("Message to design", "Reply…", "Write a comment"): it
/// sends on Enter or its button, never as it is typed.
fn composer(node: &SemanticNode) -> bool {
    names_any(
        &label(node),
        &[
            "message",
            "reply",
            "comment",
            "write a",
            "chat",
            "napisz",
            "wiadomość",
        ],
    )
}

/// A dialog the person drafts something in (a new issue, a new page): it
/// holds a text field, and its own button creates what was drafted.
fn draft_dialog(node: &SemanticNode, snapshot: &SemanticSnapshot) -> Option<usize> {
    let (index, _) =
        ancestors(node, snapshot).find(|(_, ancestor)| ancestor.role() == SemanticRole::Dialog)?;
    let region = subtree(snapshot, index);
    let drafts = region
        .iter()
        .any(|part| matches!(part.role(), SemanticRole::Textbox | SemanticRole::Combobox));
    let creates = region
        .iter()
        .any(|part| part.role() == SemanticRole::Button && creates(&label(part)));
    (drafts && creates).then_some(index)
}

/// "Create issue", "Add task", "Save page": the control that makes it.
fn creates(text: &str) -> bool {
    let text = words(text);
    ["create", "add", "save", "submit", "publish", "post", "send"]
        .iter()
        .any(|word| text.starts_with(&format!(" {word} ")))
}

fn dialog_commits(node: &SemanticNode, snapshot: &SemanticSnapshot) -> Option<Consequence> {
    let (index, _) =
        ancestors(node, snapshot).find(|(_, ancestor)| ancestor.role() == SemanticRole::Dialog)?;
    let region = subtree(snapshot, index);
    region
        .iter()
        .any(|part| names_any(&label(part), &CONFIRM_CONTEXT))
        .then(|| {
            region
                .iter()
                .find_map(|part| commit_words(&label(part)))
                .unwrap_or(Consequence::Save)
        })
}

fn pays(node: &SemanticNode, snapshot: &SemanticSnapshot) -> bool {
    subtree(snapshot, region_index(node, snapshot))
        .iter()
        .any(|part| {
            part.sensitivity() == SemanticSensitivity::Secret
                && matches!(
                    part.role(),
                    SemanticRole::Textbox | SemanticRole::Spinbutton
                )
        })
}

fn composer_has_send(snapshot: &SemanticSnapshot) -> bool {
    snapshot.nodes().iter().any(|node| {
        node.role() == SemanticRole::Button
            && names_any(
                &label(node),
                &["send", "post", "reply", "comment", "wyślij"],
            )
    })
}

/// Reads what a step would do from the observed target and its surroundings.
pub(crate) fn classify(
    kind: SemanticActionKind,
    key: Option<SemanticPressKey>,
    node: &SemanticNode,
    snapshot: &SemanticSnapshot,
) -> SiteEffect {
    let text = label(node);
    let commits = commit_words(&text);
    match kind {
        SemanticActionKind::Scroll => SiteEffect::Read,
        SemanticActionKind::Click if super::consent::banner_choice(node, snapshot).is_some() => {
            SiteEffect::Read
        }
        SemanticActionKind::Click => {
            let dismiss = names_any(&text, &DISMISS_WORDS) && commits.is_none();
            if matches!(node.role(), SemanticRole::Tab)
                || node.activation() == Some(SemanticActivation::Disclosure)
                || (node.role() == SemanticRole::Link && commits.is_none())
                || node.activation() == Some(SemanticActivation::Navigation) && commits.is_none()
                || dismiss
            {
                SiteEffect::Read
            } else if node.role() == SemanticRole::Button && pays(node, snapshot) {
                SiteEffect::Commit(Consequence::Purchase)
            } else if node.role() == SemanticRole::Button
                && creates(&text)
                && draft_dialog(node, snapshot).is_some()
            {
                // Creating what the dialog drafted puts it in the workspace.
                SiteEffect::Commit(commits.unwrap_or(Consequence::Save))
            } else if let Some(consequence) = commits.or_else(|| dialog_commits(node, snapshot)) {
                SiteEffect::Commit(consequence)
            } else if node.activation() == Some(SemanticActivation::Submit) {
                if in_search(node, snapshot) || queries(node) {
                    SiteEffect::Read
                } else {
                    SiteEffect::Commit(Consequence::Save)
                }
            } else {
                SiteEffect::Draft
            }
        }
        SemanticActionKind::Fill | SemanticActionKind::Select => {
            if in_search(node, snapshot) {
                SiteEffect::Read
            } else if node.editable_structure().is_some()
                && !ancestors(node, snapshot)
                    .any(|(_, a)| a.landmark_kind() == Some(SemanticLandmarkKind::Form))
                && !composer_has_send(snapshot)
                && !composer(node)
                && draft_dialog(node, snapshot).is_none()
            {
                // A document that saves as it is typed commits on the first key.
                SiteEffect::Commit(Consequence::Edit)
            } else {
                SiteEffect::Draft
            }
        }
        SemanticActionKind::Press => match key {
            Some(SemanticPressKey::Enter) => {
                if in_search(node, snapshot)
                    || queries(node)
                    || matches!(
                        node.role(),
                        SemanticRole::Combobox | SemanticRole::Listbox | SemanticRole::Option
                    )
                {
                    SiteEffect::Read
                } else if composer(node)
                    && (node.role() == SemanticRole::Textbox || node.editable_structure().is_some())
                    && !pays(node, snapshot)
                    && !subtree(snapshot, region_index(node, snapshot))
                        .iter()
                        .any(|part| {
                            part.role() == SemanticRole::Button
                                && matches!(
                                    commit_words(&label(part)),
                                    Some(Consequence::Purchase | Consequence::Destructive)
                                )
                        })
                {
                    // A named message composer commits a communication even
                    // when a compact look omits Send. Unrelated page buttons
                    // must not relabel Enter as saving or purchasing.
                    SiteEffect::Commit(Consequence::Communication)
                } else {
                    // Enter commits what the region's own control would.
                    let region = subtree(snapshot, region_index(node, snapshot));
                    SiteEffect::Commit(
                        region
                            .iter()
                            .filter(|part| part.role() == SemanticRole::Button)
                            .find_map(|part| commit_words(&label(part)))
                            .unwrap_or(if composer_has_send(snapshot) {
                                Consequence::Communication
                            } else {
                                Consequence::Save
                            }),
                    )
                }
            }
            Some(SemanticPressKey::Backspace | SemanticPressKey::Delete) => SiteEffect::Draft,
            Some(SemanticPressKey::Space) => match node.role() {
                SemanticRole::Button | SemanticRole::Link => {
                    classify(SemanticActionKind::Click, None, node, snapshot)
                }
                _ => SiteEffect::Draft,
            },
            _ => SiteEffect::Read,
        },
    }
}

/// A sign-in wall on this page: a visible password field, or a small page
/// asking to sign in with an account field.
pub(crate) fn sign_in_wall(observation: &SemanticObservation) -> bool {
    let Some(snapshot) = observation.frames().first() else {
        return false;
    };
    let nodes = snapshot.nodes();
    let asks = nodes.iter().any(|node| {
        matches!(node.role(), SemanticRole::Heading | SemanticRole::Button)
            && names_any(&label(node), &SIGN_IN_PHRASES)
    });
    // A visible password field is a wall; one without layout facts counts
    // only beside a sign-in heading or button.
    if nodes
        .iter()
        .any(|node| node.role() == SemanticRole::Password && (node.geometry().is_some() || asks))
    {
        return true;
    }
    nodes.len() <= 160
        && asks
        && nodes.iter().any(|node| {
            node.role() == SemanticRole::Textbox
                && (node.sensitivity() != SemanticSensitivity::Public
                    || names_any(&label(node), &["email", "username", "phone", "e-mail"]))
        })
}

/// What the person sees before a held step runs, built only from the page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Preview {
    pub(crate) consequence: Consequence,
    pub(crate) headline: String,
    pub(crate) action: String,
    pub(crate) text: Option<String>,
    pub(crate) facts: Vec<(String, String)>,
}

impl Preview {
    fn digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"ZEPHIUM-SITE-PREVIEW-1\0");
        for part in [&self.headline, &self.action]
            .into_iter()
            .chain(self.text.as_ref())
            .chain(self.facts.iter().flat_map(|(label, value)| [label, value]))
        {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
        hasher.update([self.consequence as u8]);
        hasher.finalize().into()
    }
}

const MAX_FACTS: usize = 12;
const MAX_LINE: usize = 300;
const MAX_TEXT: usize = 4096;

fn clip(text: &str, limit: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn value_text(node: &SemanticNode) -> Option<&str> {
    match node.value() {
        Some(SemanticValueSummary::Text(text))
            if node.sensitivity() == SemanticSensitivity::Public =>
        {
            Some(text.preview().text()).filter(|text| !text.trim().is_empty())
        }
        _ => None,
    }
}

/// A displayed amount such as "$1,240.00", "1 240 zł" or "€89".
fn money(text: &str) -> Option<(f64, String)> {
    const CURRENCY: [&str; 9] = ["$", "€", "£", "zł", "pln", "usd", "eur", "gbp", "¥"];
    let lower = text.to_lowercase();
    if !CURRENCY.iter().any(|mark| lower.contains(mark)) {
        return None;
    }
    let start = text.find(|ch: char| ch.is_ascii_digit())?;
    let digits: String = text[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || matches!(ch, ',' | '.' | ' ' | '\u{a0}'))
        .collect();
    let amount = digits
        .chars()
        .filter(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>()
        .parse::<f64>()
        .ok()?;
    let from = text[..start]
        .rfind(|ch: char| ch.is_whitespace())
        .map_or(0, |at| at + 1);
    let end = start + digits.trim_end().len();
    let after = text[end..]
        .split_whitespace()
        .next()
        .filter(|word| {
            CURRENCY
                .iter()
                .any(|mark| word.to_lowercase().starts_with(mark))
        })
        .map_or(String::new(), |word| format!(" {word}"));
    Some((amount, format!("{}{after}", text[from..end].trim())))
}

fn verb(text: &str, fallback: &str) -> String {
    let name = clip(text, 60);
    if name.is_empty() || name.len() > 40 {
        fallback.to_owned()
    } else {
        name
    }
}

/// Builds the person's card from the observed region around the target.
pub(crate) fn preview(
    consequence: Consequence,
    action: &SemanticPreparedAction,
    node: &SemanticNode,
    snapshot: &SemanticSnapshot,
    site: &str,
) -> Preview {
    let region = subtree(snapshot, region_index(node, snapshot));
    let name = clip(&label(node), 80);
    let heading = region
        .iter()
        .chain(snapshot.nodes())
        .find(|part| part.role() == SemanticRole::Heading && !label(part).trim().is_empty())
        .map(|part| clip(&label(part), 80));
    let headline = match consequence {
        Consequence::Communication => {
            let verb = verb(&name, "Send");
            match &heading {
                Some(place) => {
                    format!("{verb} to {} as you?", place.trim_start_matches("Message "))
                }
                None => format!("{verb} on {site} as you?"),
            }
        }
        Consequence::Purchase => {
            let total = region
                .iter()
                .filter_map(|part| {
                    let text = label(part);
                    money(&text).map(|(amount, shown)| {
                        (names_any(&text, &["total", "razem", "suma"]), amount, shown)
                    })
                })
                .max_by(|a, b| {
                    (a.0, a.1)
                        .partial_cmp(&(b.0, b.1))
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            let verb = verb(&name, "Book");
            match total {
                Some((_, _, shown)) => format!("{verb} for {shown}?"),
                None => format!("{verb} on {site}?"),
            }
        }
        Consequence::Destructive => {
            let verb = verb(&name, "Delete");
            match &heading {
                Some(item) => format!("{verb} {item}?"),
                None => format!("{verb} on {site}?"),
            }
        }
        Consequence::Save | Consequence::Edit => format!("Save changes on {site}?"),
        Consequence::Type => format!("Type on {site}?"),
    };
    let action_line = match action.kind() {
        SemanticActionKind::Fill => format!(
            "type into {}",
            if name.is_empty() { "the page" } else { &name }
        ),
        SemanticActionKind::Press => format!("press Enter in {name}"),
        _ => format!(
            "press {}",
            if name.is_empty() {
                "the control"
            } else {
                &name
            }
        ),
    };
    let text = match consequence {
        Consequence::Edit | Consequence::Type => {
            action.fill_text().map(|text| text.as_str().to_owned())
        }
        Consequence::Communication => (action.kind() == SemanticActionKind::Press)
            .then(|| value_text(node))
            .flatten()
            .or_else(|| {
                region
                    .iter()
                    .filter(|part| {
                        part.role() == SemanticRole::Textbox || part.editable_structure().is_some()
                    })
                    .filter(|part| !in_search(part, snapshot))
                    .find_map(value_text)
            })
            .map(str::to_owned),
        _ => None,
    }
    .map(|text| clip(&text, MAX_TEXT));
    let mut facts = Vec::new();
    for part in region {
        if facts.len() == MAX_FACTS {
            break;
        }
        if part.sensitivity() != SemanticSensitivity::Public || std::ptr::eq(part, node) {
            continue;
        }
        let fact = match part.role() {
            SemanticRole::Textbox | SemanticRole::Combobox | SemanticRole::Spinbutton
                if consequence != Consequence::Communication =>
            {
                part.name()
                    .zip(value_text(part))
                    .map(|(name, value)| (name.as_str().to_owned(), value.to_owned()))
            }
            SemanticRole::Paragraph
            | SemanticRole::Cell
            | SemanticRole::ListItem
            | SemanticRole::Status => label(part)
                .split_once(':')
                .map(|(label, value)| (label.trim().to_owned(), value.trim().to_owned()))
                .filter(|(label, value)| {
                    !label.is_empty() && !value.is_empty() && label.len() <= 60
                }),
            _ => None,
        };
        if let Some((label, value)) = fact {
            facts.push((clip(&label, MAX_LINE), clip(&value, MAX_LINE)));
        }
    }
    Preview {
        consequence,
        headline: clip(&headline, MAX_LINE),
        action: clip(&action_line, MAX_LINE),
        text,
        facts,
    }
}

/// The exact step the person decides on: where it is, what it presses and
/// what it types. Page node keys are left out; the page's structure is not.
pub(crate) fn fingerprint(
    action: &SemanticPreparedAction,
    node: &SemanticNode,
    snapshot: &SemanticSnapshot,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"ZEPHIUM-SITE-STEP-1\0");
    let mut part = |bytes: &[u8]| {
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    };
    part(snapshot.frame().origin().as_url().as_str().as_bytes());
    part(format!("{:?}", action.kind()).as_bytes());
    part(format!("{:?}", action.bound_action().press_key()).as_bytes());
    part(
        action
            .fill_text()
            .map_or("", |text| text.as_str())
            .as_bytes(),
    );
    part(
        format!(
            "{:?}|{:?}|{:?}",
            node.role(),
            node.activation(),
            node.form()
        )
        .as_bytes(),
    );
    part(label(node).as_bytes());
    for (_, ancestor) in ancestors(node, snapshot) {
        part(format!("{:?}|{:?}", ancestor.role(), ancestor.landmark_kind()).as_bytes());
        part(ancestor.name().map_or("", |name| name.as_str()).as_bytes());
    }
    hasher.finalize().into()
}

/// A held step waiting for the person.
#[derive(Clone, Debug)]
pub(crate) struct Pending {
    pub(crate) fingerprint: [u8; 32],
    pub(crate) digest: [u8; 32],
    pub(crate) preview: Preview,
}

/// How an approved step ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Receipt {
    /// It ran and the page showed its effect.
    Committed,
    /// It ran; the page did not show its effect before the page ended.
    Unverified,
    /// It never ran: the page changed first or the step was not taken.
    NotSent,
    Declined,
}

#[derive(Default)]
struct GateState {
    site: String,
    allow_edits: bool,
    /// Typing on this site waits for the person; see `Consequence::Type`.
    hold_typing: bool,
    /// The person allowed typing on this site for the run.
    allow_typing: bool,
    pending: Option<Pending>,
    /// One approved step, consumed by the first matching proposal.
    confirmed: Option<Pending>,
    /// The approved step was handed to the page and awaits verification.
    dispatched: Option<Consequence>,
    receipt: Option<Receipt>,
    declined: Vec<[u8; 32]>,
    /// Status lines seen before the last step, for the commit detector.
    status_before: Vec<String>,
    unconfirmed: Option<String>,
    /// A committing step was held with no one to ask.
    held: bool,
    /// The question put to the person, and whether they answered it.
    ask: Option<(u32, bool)>,
    /// Someone was asked at least once.
    asked: bool,
    /// The entry question waits on the start page's first view.
    entry: EntryCheck,
    /// The person finished a sign-in in a tab while the page waited.
    signed_in_elsewhere: bool,
    /// Rust pressed a cookie banner's refusal on this page.
    consent_pressed: bool,
    /// The task reads a daily app's view as its records.
    view: bool,
    /// The app's views still to open for the read, in order.
    views: Vec<zephium_agentic::AppView>,
    /// The views Rust opened, oldest first.
    opened: Vec<zephium_agentic::AppView>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum EntryCheck {
    #[default]
    Settled,
    /// The first view decides: signed out goes on, anything else asks.
    Pending,
    /// The person is being asked before the model sees the page.
    Asking,
}

/// Shared by a page task and its successors: the held step, the one approved
/// step, declines and what the page did after each step.
#[derive(Default)]
pub(crate) struct SiteGate(Mutex<GateState>);

impl SiteGate {
    pub(crate) fn new(site: String, allow_edits: bool, entry: bool) -> Self {
        Self(Mutex::new(GateState {
            site,
            allow_edits,
            entry: if entry {
                EntryCheck::Pending
            } else {
                EntryCheck::Settled
            },
            ..GateState::default()
        }))
    }
    /// The task only reads what a daily app's views list, going through
    /// the views its goal asks for.
    pub(crate) fn reading_view(self, url: &str, goal: &str) -> Self {
        {
            let mut state = self.state();
            state.view = true;
            state.views = zephium_agentic::ContextNavigationTarget::parse(url)
                .ok()
                .and_then(|url| {
                    url.as_url()
                        .host_str()
                        .and_then(zephium_agentic::DailyApp::of)
                })
                .map(|app| zephium_agentic::app_views(app, goal).to_vec())
                .unwrap_or_default();
        }
        self
    }
    /// Every field typed into on this site waits for the person, until they
    /// allow typing on it for the run.
    pub(crate) fn holding_typing(self, hold: bool) -> Self {
        self.state().hold_typing = hold;
        self
    }
    /// Which views the read went through, for the helper: nothing new in the
    /// first ones, the rows from the last.
    pub(crate) fn view_note(&self) -> Option<String> {
        let state = self.state();
        let (last, before) = state.opened.split_last()?;
        let names = |views: &[zephium_agentic::AppView]| {
            views
                .iter()
                .map(|view| view.name())
                .collect::<Vec<_>>()
                .join(" and ")
        };
        Some(match (before.is_empty(), last.latest()) {
            (true, _) => format!("Read from the {} view.", last.name()),
            (false, true) => format!(
                "Nothing new in {}: these are the latest in {}.",
                names(before),
                last.name()
            ),
            (false, false) => format!(
                "Nothing new in {}; read from {}.",
                names(before),
                last.name()
            ),
        })
    }
    pub(crate) fn site(&self) -> String {
        self.state().site.clone()
    }
    pub(crate) fn signed_in_now(&self) {
        self.state().signed_in_elsewhere = true;
    }
    pub(crate) fn signed_in_elsewhere(&self) -> bool {
        self.state().signed_in_elsewhere
    }
    pub(crate) fn consent_pressing(&self, pressing: bool) {
        self.state().consent_pressed |= pressing;
    }
    pub(crate) fn pressed_consent(&self) -> bool {
        self.state().consent_pressed
    }
    pub(crate) fn entry(&self) -> EntryCheck {
        self.state().entry
    }
    /// The person answered the entry question; the page goes on.
    pub(crate) fn entered(&self) {
        self.state().entry = EntryCheck::Settled;
    }
    fn state(&self) -> std::sync::MutexGuard<'_, GateState> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub(crate) fn pending(&self) -> Option<Pending> {
        self.state().pending.clone()
    }
    pub(crate) fn allow_edits(&self) -> bool {
        self.state().allow_edits
    }
    /// The one extra effect a successor may carry: the approved step's.
    pub(crate) fn approved_class(&self) -> Option<SemanticEffectClass> {
        self.state()
            .confirmed
            .as_ref()
            .map(|pending| pending.preview.consequence.class())
    }
    /// The person approved the held step; `for_run` also allows later edits,
    /// or later typing when typing was what it held.
    pub(crate) fn approve(&self, for_run: bool) -> Option<Preview> {
        let mut state = self.state();
        let pending = state.pending.take()?;
        if for_run && pending.preview.consequence == Consequence::Type {
            state.allow_typing = true;
        } else if for_run {
            state.allow_edits = true;
        }
        let preview = pending.preview.clone();
        state.confirmed = Some(pending);
        state.receipt = None;
        Some(preview)
    }
    pub(crate) fn decline(&self) -> Option<Preview> {
        let mut state = self.state();
        let pending = state.pending.take()?;
        state.declined.push(pending.fingerprint);
        state.receipt = Some(Receipt::Declined);
        Some(pending.preview)
    }
    /// Takes how the last approved step ended, once known.
    pub(crate) fn take_receipt(&self) -> Option<Receipt> {
        self.state().receipt.take()
    }
    /// The page ended: an approved step still waiting ends here too.
    pub(crate) fn finish(&self) -> Option<Receipt> {
        let mut state = self.state();
        if state.dispatched.take().is_some() {
            return Some(Receipt::Unverified);
        }
        if state.confirmed.take().is_some() {
            return Some(Receipt::NotSent);
        }
        state.receipt.take()
    }
    pub(crate) fn unconfirmed(&self) -> Option<String> {
        self.state().unconfirmed.clone()
    }
    pub(crate) fn ask(&self) -> Option<(u32, bool)> {
        self.state().ask
    }
    pub(crate) fn set_ask(&self, ask: Option<(u32, bool)>) {
        let mut state = self.state();
        state.asked |= ask.is_some();
        state.ask = ask;
    }
    /// Held back with the question unanswered, or with no one to ask.
    pub(crate) fn held_back(&self) -> bool {
        let state = self.state();
        state.pending.is_some() || (state.held && !state.asked)
    }
}

fn status_lines(observation: &SemanticObservation) -> Vec<String> {
    observation
        .frames()
        .iter()
        .flat_map(|frame| frame.nodes())
        .filter(|node| matches!(node.role(), SemanticRole::Status | SemanticRole::Heading))
        .map(|node| clip(&label(node), MAX_LINE))
        .filter(|line| !line.is_empty())
        .collect()
}

pub(crate) struct SiteWorkPolicy {
    pub(crate) gate: Arc<SiteGate>,
    /// Someone can be asked; without it a committing step is only held back.
    pub(crate) asks: bool,
}

/// Controls a signed-in page shows for its account.
const ACCOUNT_WORDS: [&str; 10] = [
    "account",
    "profile",
    "avatar",
    "my account",
    "sign out",
    "log out",
    "logout",
    "wyloguj",
    "konto",
    "your profile",
];

/// The start page reads as signed out: a sign-in wall, or a sign-in control
/// in its header or navigation with no account control anywhere.
pub(crate) fn signed_out(observation: &SemanticObservation) -> bool {
    if sign_in_wall(observation) {
        return true;
    }
    let Some(snapshot) = observation.frames().first() else {
        return false;
    };
    let controls = || {
        snapshot
            .nodes()
            .iter()
            .filter(|node| matches!(node.role(), SemanticRole::Link | SemanticRole::Button))
    };
    let offers = controls().any(|node| {
        names_any(&label(node), &SIGN_IN_PHRASES)
            && ancestors(node, snapshot).any(|(_, ancestor)| {
                matches!(
                    ancestor.landmark_kind(),
                    Some(SemanticLandmarkKind::Banner | SemanticLandmarkKind::Navigation)
                )
            })
    });
    offers && !controls().any(|node| names_any(&label(node), &ACCOUNT_WORDS))
}

fn credential(node: &SemanticNode) -> bool {
    node.role() == SemanticRole::Password || node.sensitivity() == SemanticSensitivity::Secret
}

impl AgentWorkLocalActionPolicy for SiteWorkPolicy {
    fn consent_dismissal(&self, observation: &SemanticObservation) -> Option<SemanticReferenceId> {
        super::consent::dismissal(observation)
    }

    fn consent_suspected(&self, observation: &SemanticObservation) -> bool {
        super::consent::suspected(observation)
    }

    fn consent_pressing(&self, pressing: bool) {
        self.gate.consent_pressing(pressing);
    }

    fn whole_first_look(&self) -> bool {
        self.gate.entry() == EntryCheck::Pending
    }

    fn reads_app_view(&self) -> bool {
        self.gate.state().view
    }

    fn app_view(&self, observation: &SemanticObservation) -> Option<SemanticReferenceId> {
        let mut state = self.gate.state();
        while !state.views.is_empty() {
            let view = state.views.remove(0);
            if let Some(control) = zephium_agentic::app_view_control(observation, view) {
                state.opened.push(view);
                return Some(control);
            }
        }
        None
    }

    fn human_wall(&self, observation: &SemanticObservation) -> Option<AgentBrowserHumanReason> {
        let mut state = self.gate.state();
        if state.entry == EntryCheck::Pending {
            state.entry = if signed_out(observation) {
                EntryCheck::Settled
            } else {
                EntryCheck::Asking
            };
        }
        if state.entry == EntryCheck::Asking {
            return Some(AgentBrowserHumanReason::UserDecision);
        }
        if state.unconfirmed.is_some() {
            return Some(AgentBrowserHumanReason::Verification);
        }
        if state.pending.is_some() && self.asks {
            return Some(AgentBrowserHumanReason::UserDecision);
        }
        drop(state);
        sign_in_wall(observation).then_some(AgentBrowserHumanReason::SignIn)
    }

    fn model_action_operations(
        &self,
        node: &SemanticNode,
        _: &SemanticObservation,
    ) -> Result<SemanticOperations, AgentWorkFailure> {
        if node.states().contains(SemanticState::Disabled) || node.geometry().is_none() {
            return Ok(SemanticOperations::NONE);
        }
        let operations = [
            SemanticOperationClass::Click,
            SemanticOperationClass::Fill,
            SemanticOperationClass::Select,
            SemanticOperationClass::Press,
            SemanticOperationClass::Scroll,
        ]
        .into_iter()
        .filter(|operation| node.operations().contains(*operation))
        .filter(|operation| {
            !credential(node)
                || matches!(
                    operation,
                    SemanticOperationClass::Click | SemanticOperationClass::Scroll
                )
        })
        .collect::<Vec<_>>();
        SemanticOperations::try_new(&operations).map_err(|_| AgentWorkFailure::Contract)
    }

    fn assess(
        &self,
        action: &SemanticPreparedAction,
        observation: &SemanticObservation,
    ) -> Result<AgentEffectAssessment, AgentWorkFailure> {
        let snapshot = observation
            .frames()
            .first()
            .ok_or(AgentWorkFailure::Contract)?;
        let node = snapshot
            .nodes()
            .iter()
            .find(|node| node.reference() == action.target_reference())
            .ok_or(AgentWorkFailure::Contract)?;
        if credential(node)
            && matches!(
                action.kind(),
                SemanticActionKind::Fill | SemanticActionKind::Press | SemanticActionKind::Select
            )
        {
            return Err(AgentWorkFailure::ActionDenied);
        }
        let declared = action.effect();
        // A banner choice is a read: refused optional cookies, a notice
        // closed or its settings opened. Accepting is refused while the same
        // banner offers a refusal.
        if action.kind() == SemanticActionKind::Click {
            if let Some(choice) = super::consent::banner_choice(node, snapshot) {
                if choice == super::consent::Choice::Accept
                    && super::consent::offers_refusal(node, snapshot)
                {
                    return Err(AgentWorkFailure::ActionDenied);
                }
                if declared != SemanticEffectClass::Read {
                    return Err(AgentWorkFailure::EffectRequired(SemanticEffectClass::Read));
                }
                return Ok(AgentEffectAssessment::new(
                    action,
                    action.frame().origin().clone(),
                    declared,
                ));
            }
        }
        let accept = || {
            Ok(AgentEffectAssessment::new(
                action,
                action.frame().origin().clone(),
                declared,
            ))
        };
        let observed = classify(
            action.kind(),
            action.bound_action().press_key(),
            node,
            snapshot,
        );
        let held_typing = matches!(
            action.kind(),
            SemanticActionKind::Fill | SemanticActionKind::Select
        ) && {
            let state = self.gate.state();
            state.hold_typing && !state.allow_typing
        };
        // The more consequential side wins; neither lowers the other.
        let consequence = match (observed, Consequence::declared(declared)) {
            (SiteEffect::Commit(consequence), _) => consequence,
            (_, Some(consequence)) => consequence,
            _ if held_typing => Consequence::Type,
            (_, None) if declared == SemanticEffectClass::CapabilityBoundary => {
                return Err(AgentWorkFailure::ActionDenied);
            }
            (_, None) => {
                self.gate.state().status_before = status_lines(observation);
                return accept();
            }
        };
        let mut state = self.gate.state();
        let fingerprint = fingerprint(action, node, snapshot);
        let site = state.site.clone();
        let preview = preview(consequence, action, node, snapshot, &site);
        let digest = preview.digest();
        if let Some(confirmed) = state.confirmed.take() {
            if confirmed.fingerprint == fingerprint && confirmed.digest == digest {
                let fits = if consequence == Consequence::Type {
                    matches!(
                        declared,
                        SemanticEffectClass::Read | SemanticEffectClass::LocalWrite
                    )
                } else {
                    declared == consequence.class()
                };
                if !fits {
                    state.confirmed = Some(confirmed);
                    return Err(AgentWorkFailure::EffectRequired(consequence.class()));
                }
                state.dispatched = Some(consequence);
                state.status_before = status_lines(observation);
                return accept();
            }
            // Edits allowed for the run: the approval covers this edit too.
            if consequence == Consequence::Edit
                && state.allow_edits
                && confirmed.preview.consequence == Consequence::Edit
            {
                if declared != SemanticEffectClass::ExternalWrite {
                    state.confirmed = Some(confirmed);
                    return Err(AgentWorkFailure::EffectRequired(
                        SemanticEffectClass::ExternalWrite,
                    ));
                }
                state.dispatched = Some(consequence);
                state.status_before = status_lines(observation);
                return accept();
            }
            // The page changed since the person decided: that approval is spent.
            state.receipt = Some(Receipt::NotSent);
        }
        if consequence == Consequence::Edit && state.allow_edits {
            if declared != SemanticEffectClass::ExternalWrite {
                return Err(AgentWorkFailure::EffectRequired(
                    SemanticEffectClass::ExternalWrite,
                ));
            }
            state.status_before = status_lines(observation);
            return accept();
        }
        if state.declined.contains(&fingerprint) {
            return Err(AgentWorkFailure::ActionDenied);
        }
        state.held = true;
        if self.asks {
            state.pending = Some(Pending {
                fingerprint,
                digest,
                preview,
            });
        }
        Err(AgentWorkFailure::ActionDenied)
    }

    fn accept_verified_action(
        &mut self,
        _: &SemanticActionBatchResult,
        observation: &SemanticObservation,
    ) -> Result<(), AgentWorkFailure> {
        self.after_step(observation);
        Ok(())
    }
}

impl SiteWorkPolicy {
    /// Records how a verified step ended: the approved one committed, or
    /// the page said a step Rust read as harmless committed something.
    fn after_step(&self, observation: &SemanticObservation) {
        let mut state = self.gate.state();
        if state.dispatched.take().is_some() {
            state.receipt = Some(Receipt::Committed);
            state.status_before = status_lines(observation);
            return;
        }
        // After the fact: a step Rust read as harmless made the page say it
        // committed something.
        let before = std::mem::take(&mut state.status_before);
        if let Some(line) = status_lines(observation)
            .into_iter()
            .find(|line| !before.contains(line) && names_any(line, &DONE_WORDS))
        {
            state.unconfirmed = Some(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn page(nodes: Vec<serde_json::Value>) -> SemanticObservation {
        super::super::tests::reading_observation(json!(nodes), "complete")
    }

    fn effect(observation: &SemanticObservation, key: u64, kind: SemanticActionKind) -> SiteEffect {
        effect_key(observation, key, kind, None)
    }
    fn effect_key(
        observation: &SemanticObservation,
        key: u64,
        kind: SemanticActionKind,
        press: Option<SemanticPressKey>,
    ) -> SiteEffect {
        let snapshot = observation.frames().first().unwrap();
        let node = &snapshot.nodes()[usize::try_from(key - 1).unwrap()];
        classify(kind, press, node, snapshot)
    }

    #[test]
    fn committing_controls_are_held_and_drafting_proceeds() {
        let slack = page(vec![
            json!({"k":1,"r":"document","o":16}),
            json!({"k":2,"p":0,"r":"textbox","n":"Message #design","o":10}),
            json!({"k":3,"p":0,"r":"button","n":"Send now","o":1,"ak":1}),
            json!({"k":4,"p":0,"r":"button","n":"Emoji","o":1,"ak":1}),
            json!({"k":5,"p":0,"r":"link","n":"#general","o":1,"ak":4}),
            json!({"k":6,"p":0,"r":"tab","n":"Threads","o":1}),
        ]);
        assert_eq!(
            effect(&slack, 3, SemanticActionKind::Click),
            SiteEffect::Commit(Consequence::Communication)
        );
        assert_eq!(
            effect(&slack, 4, SemanticActionKind::Click),
            SiteEffect::Draft
        );
        assert_eq!(
            effect(&slack, 5, SemanticActionKind::Click),
            SiteEffect::Read
        );
        assert_eq!(
            effect(&slack, 6, SemanticActionKind::Click),
            SiteEffect::Read
        );
        assert_eq!(
            effect(&slack, 2, SemanticActionKind::Fill),
            SiteEffect::Draft
        );
        assert_eq!(
            effect_key(
                &slack,
                2,
                SemanticActionKind::Press,
                Some(SemanticPressKey::Enter)
            ),
            SiteEffect::Commit(Consequence::Communication)
        );
        let booking = page(vec![
            json!({"k":1,"r":"document","o":16}),
            json!({"k":2,"p":0,"r":"button","n":"Request to book","o":1,"ak":1}),
            json!({"k":3,"p":0,"r":"button","n":"Apply filters","o":1,"ak":1}),
            json!({"k":4,"p":0,"r":"dialog","n":"Delete this page?"}),
            json!({"k":5,"p":3,"r":"paragraph","t":"This cannot be undone."}),
            json!({"k":6,"p":3,"r":"button","n":"Yes","o":1,"ak":1}),
            json!({"k":7,"p":3,"r":"button","n":"Cancel","o":1,"ak":1}),
        ]);
        assert_eq!(
            effect(&booking, 2, SemanticActionKind::Click),
            SiteEffect::Commit(Consequence::Purchase)
        );
        assert_eq!(
            effect(&booking, 3, SemanticActionKind::Click),
            SiteEffect::Draft
        );
        assert_eq!(
            effect(&booking, 6, SemanticActionKind::Click),
            SiteEffect::Commit(Consequence::Destructive)
        );
        assert_eq!(
            effect(&booking, 7, SemanticActionKind::Click),
            SiteEffect::Read
        );
        let search = page(vec![
            json!({"k":1,"r":"document","o":16}),
            json!({"k":2,"p":0,"r":"landmark","lm":"search"}),
            json!({"k":3,"p":1,"r":"searchbox","n":"Where","o":2}),
            json!({"k":4,"p":1,"r":"button","n":"Search","o":1,"ak":2}),
        ]);
        assert_eq!(
            effect(&search, 3, SemanticActionKind::Fill),
            SiteEffect::Read
        );
        assert_eq!(
            effect(&search, 4, SemanticActionKind::Click),
            SiteEffect::Read
        );
        assert_eq!(
            effect_key(
                &search,
                3,
                SemanticActionKind::Press,
                Some(SemanticPressKey::Enter)
            ),
            SiteEffect::Read
        );
    }

    #[test]
    fn rich_editors_draft_until_their_own_control_commits() {
        // Slack's composer with its Send button out of the observation, and
        // Linear's new-issue dialog of rich editors with its Create button.
        let apps = page(vec![
            json!({"k":1,"r":"document","o":16}),
            json!({"k":2,"p":0,"r":"textbox","n":"Message to design","o":11,"fs":1,"es":[1,2,false]}),
            json!({"k":3,"p":0,"r":"button","n":"Create new issue","o":1,"ak":1}),
            json!({"k":4,"p":0,"r":"dialog","n":"New issue"}),
            json!({"k":5,"p":3,"r":"textbox","n":"Issue title","o":11,"fs":1,"es":[1,2,false]}),
            json!({"k":6,"p":3,"r":"textbox","n":"Add description…","o":11,"fs":1,"es":[1,2,false]}),
            json!({"k":7,"p":3,"r":"button","n":"Team: Backlog","o":1,"ak":1}),
            json!({"k":8,"p":3,"r":"button","n":"Create issue","o":1,"ak":1}),
            json!({"k":9,"p":0,"r":"textbox","n":"Block 1","o":11,"fs":1,"es":[1,2,false]}),
        ]);
        assert_eq!(
            effect(&apps, 2, SemanticActionKind::Fill),
            SiteEffect::Draft
        );
        assert_eq!(
            effect_key(
                &apps,
                2,
                SemanticActionKind::Press,
                Some(SemanticPressKey::Enter),
            ),
            SiteEffect::Commit(Consequence::Communication),
            "a named composer still sends when its Send button is outside the look",
        );
        assert_eq!(
            effect(&apps, 3, SemanticActionKind::Click),
            SiteEffect::Draft
        );
        assert_eq!(
            effect(&apps, 5, SemanticActionKind::Fill),
            SiteEffect::Draft
        );
        assert_eq!(
            effect(&apps, 6, SemanticActionKind::Fill),
            SiteEffect::Draft
        );
        assert_eq!(
            effect(&apps, 7, SemanticActionKind::Click),
            SiteEffect::Draft
        );
        assert_eq!(
            effect(&apps, 8, SemanticActionKind::Click),
            SiteEffect::Commit(Consequence::Save)
        );
        // A page of blocks still saves as it is typed.
        assert_eq!(
            effect(&apps, 9, SemanticActionKind::Fill),
            SiteEffect::Commit(Consequence::Edit)
        );
    }

    #[test]
    fn form_facts_let_queries_through_and_hold_posts() {
        let forms = page(vec![
            json!({"k":1,"r":"document","o":16}),
            json!({"k":2,"p":0,"r":"textbox","n":"Find","o":10,"ff":13}),
            json!({"k":3,"p":0,"r":"button","n":"Go","o":1,"ak":2,"ff":13}),
            json!({"k":4,"p":0,"r":"button","n":"Apply","o":1,"ak":2,"ff":5}),
            json!({"k":5,"p":0,"r":"button","n":"Continue","o":1,"ak":2,"ff":6}),
            json!({"k":6,"p":0,"r":"button","n":"Go","o":1,"ak":2,"ff":1}),
        ]);
        assert_eq!(
            effect(&forms, 2, SemanticActionKind::Fill),
            SiteEffect::Read
        );
        assert_eq!(
            effect_key(
                &forms,
                2,
                SemanticActionKind::Press,
                Some(SemanticPressKey::Enter)
            ),
            SiteEffect::Read
        );
        assert_eq!(
            effect(&forms, 3, SemanticActionKind::Click),
            SiteEffect::Read
        );
        assert_eq!(
            effect(&forms, 4, SemanticActionKind::Click),
            SiteEffect::Read
        );
        assert_eq!(
            effect(&forms, 5, SemanticActionKind::Click),
            SiteEffect::Commit(Consequence::Save)
        );
        // A GET to another origin is not the page's own query.
        assert_eq!(
            effect(&forms, 6, SemanticActionKind::Click),
            SiteEffect::Commit(Consequence::Save)
        );
    }

    #[test]
    fn message_names_do_not_lower_purchase_or_destructive_enter_effects() {
        for (control, consequence) in [
            ("Pay", Consequence::Purchase),
            ("Delete", Consequence::Destructive),
        ] {
            let look = page(vec![
                json!({"k":1,"r":"document","o":16}),
                json!({"k":2,"p":0,"r":"textbox","n":"Message","o":10}),
                json!({"k":3,"p":0,"r":"button","n":control,"o":1}),
            ]);
            assert_eq!(
                effect_key(
                    &look,
                    2,
                    SemanticActionKind::Press,
                    Some(SemanticPressKey::Enter)
                ),
                SiteEffect::Commit(consequence),
            );
        }
    }

    fn booking(total: &str) -> SemanticObservation {
        page(vec![
            json!({"k":1,"r":"document","o":16,"fc":true}),
            json!({"k":2,"p":0,"r":"landmark","lm":"form","n":"Reserve","fc":true}),
            json!({"k":3,"p":1,"r":"heading","l":2,"n":"Cabin by the lake","fc":true}),
            json!({"k":4,"p":1,"r":"paragraph","t":"Dates: 3–5 May","fc":true}),
            json!({"k":5,"p":1,"r":"paragraph","t":format!("Total: {total}"),"fc":true}),
            json!({"k":6,"p":1,"r":"button","n":"Request to book","o":9,"ak":2,"fc":true,
                "b":{"x":1,"y":1,"w":100,"h":30}}),
        ])
    }

    fn click(
        observation: &SemanticObservation,
        key: usize,
        effect: SemanticEffectClass,
    ) -> SemanticPreparedAction {
        let snapshot = &observation.frames()[0];
        let proposal = SemanticActionProposal::try_new(
            SemanticActionIntent::Click {
                target: snapshot.nodes()[key - 1].reference(),
            },
            effect,
            SemanticWaitCondition::Immediate,
            SemanticVerification::PageChanged,
            SemanticSettleBudget::try_new(2000).unwrap(),
        )
        .unwrap();
        SemanticActionBatch::bind(
            SemanticActionBatchId::new(1).unwrap(),
            observation,
            &[snapshot.frame().clone()],
            vec![proposal],
        )
        .unwrap()
        .actions()[0]
            .prepare(snapshot)
            .unwrap()
    }

    #[test]
    fn a_compact_slack_composer_enter_holds_its_own_draft_and_requires_exact_approval() {
        let look = |draft: &str| {
            page(vec![
                json!({"k":1,"r":"document","o":16,"fc":true}),
                json!({"k":2,"p":0,"r":"searchbox","n":"Search Slack","o":10,"fc":true,
                "v":{"k":"text","value":"older search"},"b":{"x":1,"y":1,"w":200,"h":30}}),
                json!({"k":3,"p":0,"r":"heading","l":1,"n":"#design","fc":true}),
                // A viewport can omit Send while retaining Slack's rich composer.
                json!({"k":4,"p":0,"r":"textbox","n":"Message to design","o":11,"fs":1,"es":[1,2,false],"fc":true,
                "v":{"k":"text","value":draft},"b":{"x":1,"y":100,"w":400,"h":60}}),
                json!({"k":5,"p":0,"r":"button","n":"Save for later","o":1,"fc":true}),
            ])
        };
        let enter = |observation: &SemanticObservation| {
            let snapshot = &observation.frames()[0];
            let proposal = SemanticActionProposal::try_new(
                SemanticActionIntent::Press {
                    target: snapshot.nodes()[3].reference(),
                    key: SemanticPressKey::Enter,
                },
                SemanticEffectClass::Communication,
                SemanticWaitCondition::Immediate,
                SemanticVerification::PageChanged,
                SemanticSettleBudget::try_new(2000).unwrap(),
            )
            .unwrap();
            SemanticActionBatch::bind(
                SemanticActionBatchId::new(1).unwrap(),
                observation,
                &[snapshot.frame().clone()],
                vec![proposal],
            )
            .unwrap()
            .actions()[0]
                .prepare(snapshot)
                .unwrap()
        };
        let gate = Arc::new(SiteGate::new("slack.com".into(), false, false));
        let policy = SiteWorkPolicy {
            gate: gate.clone(),
            asks: true,
        };
        let original = look("testing agentic browsing on Windows");
        assert!(matches!(
            policy.assess(&enter(&original), &original),
            Err(AgentWorkFailure::ActionDenied)
        ));
        let pending = gate.pending().unwrap().preview;
        assert_eq!(pending.consequence, Consequence::Communication);
        assert_eq!(
            pending.text.as_deref(),
            Some("testing agentic browsing on Windows")
        );
        gate.approve(false).unwrap();
        // A changed draft consumes the old approval without permitting Enter.
        let changed = look("changed draft");
        assert!(matches!(
            policy.assess(&enter(&changed), &changed),
            Err(AgentWorkFailure::ActionDenied)
        ));
        assert_eq!(gate.take_receipt(), Some(Receipt::NotSent));
        assert_eq!(
            gate.pending().unwrap().preview.text.as_deref(),
            Some("changed draft")
        );
        gate.approve(false).unwrap();
        assert!(policy.assess(&enter(&changed), &changed).is_ok());
        assert!(matches!(
            policy.assess(&enter(&changed), &changed),
            Err(AgentWorkFailure::ActionDenied)
        ));
        assert_eq!(gate.finish(), Some(Receipt::Unverified));
    }

    #[test]
    fn a_commit_is_held_with_a_page_preview_and_runs_once_only_while_unchanged() {
        let gate = Arc::new(SiteGate::new("airbnb.com".into(), false, false));
        let policy = SiteWorkPolicy {
            gate: gate.clone(),
            asks: true,
        };
        let page = booking("$1,240.00");
        // Declared as a mere read, the page still shows a purchase.
        let action = click(&page, 6, SemanticEffectClass::Read);
        assert!(matches!(
            policy.assess(&action, &page),
            Err(AgentWorkFailure::ActionDenied)
        ));
        assert_eq!(
            policy.human_wall(&page),
            Some(AgentBrowserHumanReason::UserDecision)
        );
        let held = gate.pending().unwrap().preview;
        assert_eq!(held.consequence, Consequence::Purchase);
        assert_eq!(held.headline, "Request to book for $1,240.00?");
        assert_eq!(held.action, "press Request to book");
        assert!(held
            .facts
            .contains(&("Dates".to_owned(), "3–5 May".to_owned())));
        gate.approve(false).unwrap();
        assert_eq!(gate.approved_class(), Some(SemanticEffectClass::Purchase));
        // The successor must declare the true effect; then it runs once.
        assert!(matches!(
            policy.assess(&action, &page),
            Err(AgentWorkFailure::EffectRequired(
                SemanticEffectClass::Purchase
            ))
        ));
        let purchase = click(&page, 6, SemanticEffectClass::Purchase);
        assert!(policy.assess(&purchase, &page).is_ok());
        assert!(matches!(
            policy.assess(&purchase, &page),
            Err(AgentWorkFailure::ActionDenied)
        ));
        assert_eq!(gate.finish(), Some(Receipt::Unverified));

        // A changed total spends the approval and asks again.
        let gate = Arc::new(SiteGate::new("airbnb.com".into(), false, false));
        let policy = SiteWorkPolicy {
            gate: gate.clone(),
            asks: true,
        };
        let _ = policy.assess(&click(&page, 6, SemanticEffectClass::Purchase), &page);
        gate.approve(false).unwrap();
        let changed = booking("$1,480.00");
        assert!(matches!(
            policy.assess(&click(&changed, 6, SemanticEffectClass::Purchase), &changed),
            Err(AgentWorkFailure::ActionDenied)
        ));
        assert_eq!(gate.take_receipt(), Some(Receipt::NotSent));
        assert_eq!(
            gate.pending().unwrap().preview.headline,
            "Request to book for $1,480.00?"
        );
        // Declined, the same step is refused without asking again.
        gate.decline().unwrap();
        assert!(matches!(
            policy.assess(&click(&changed, 6, SemanticEffectClass::Purchase), &changed),
            Err(AgentWorkFailure::ActionDenied)
        ));
        assert!(gate.pending().is_none());
    }

    #[test]
    fn typing_on_a_site_the_person_did_not_name_waits_until_allowed_for_the_run() {
        let search = page(vec![
            json!({"k":1,"r":"document","o":16,"fc":true}),
            json!({"k":2,"p":0,"r":"landmark","lm":"search","fc":true}),
            json!({"k":3,"p":1,"r":"searchbox","n":"Search","o":2,"fc":true,
                "v":{"k":"text","value":""},"b":{"x":1,"y":1,"w":200,"h":30}}),
        ]);
        let fill = |effect| {
            let snapshot = &search.frames()[0];
            let proposal = SemanticActionProposal::try_new(
                SemanticActionIntent::Fill {
                    target: snapshot.nodes()[2].reference(),
                    value: SemanticActionText::try_new("aisle seat WAW-SFO".into()).unwrap(),
                },
                effect,
                SemanticWaitCondition::Immediate,
                SemanticVerification::TargetValueMatchesInput,
                SemanticSettleBudget::try_new(2000).unwrap(),
            )
            .unwrap();
            SemanticActionBatch::bind(
                SemanticActionBatchId::new(1).unwrap(),
                &search,
                &[snapshot.frame().clone()],
                vec![proposal],
            )
            .unwrap()
            .actions()[0]
                .prepare(snapshot)
                .unwrap()
        };
        // Without the hold a search is a read, as before.
        let open = SiteWorkPolicy {
            gate: Arc::new(SiteGate::new("example.com".into(), false, false)),
            asks: true,
        };
        assert!(open
            .assess(&fill(SemanticEffectClass::Read), &search)
            .is_ok());

        let gate = Arc::new(SiteGate::new("example.com".into(), false, false).holding_typing(true));
        let policy = SiteWorkPolicy {
            gate: gate.clone(),
            asks: true,
        };
        assert!(policy
            .assess(&fill(SemanticEffectClass::Read), &search)
            .is_err());
        let held = gate.pending().unwrap().preview;
        assert_eq!(held.consequence, Consequence::Type);
        assert_eq!(held.headline, "Type on example.com?");
        assert_eq!(held.text.as_deref(), Some("aisle seat WAW-SFO"));
        gate.approve(true).unwrap();
        assert!(!gate.allow_edits());
        // The approved fill runs as declared, and later typing needs no question.
        assert!(policy
            .assess(&fill(SemanticEffectClass::Read), &search)
            .is_ok());
        assert!(policy
            .assess(&fill(SemanticEffectClass::LocalWrite), &search)
            .is_ok());
        assert!(gate.pending().is_none());
    }

    #[test]
    fn an_autosaving_editor_asks_once_then_edits_run_for_the_run() {
        let doc = page(vec![
            json!({"k":1,"r":"document","o":16,"fc":true}),
            json!({"k":2,"p":0,"r":"heading","l":1,"n":"Meeting notes","fc":true}),
            json!({"k":3,"p":0,"r":"textbox","n":"Notes","o":3,"fs":1,"es":[1,1,false],"fc":true,
                "v":{"k":"text","value":""},"b":{"x":1,"y":40,"w":400,"h":300}}),
        ]);
        let fill = |effect| {
            let snapshot = &doc.frames()[0];
            let proposal = SemanticActionProposal::try_new(
                SemanticActionIntent::Fill {
                    target: snapshot.nodes()[2].reference(),
                    value: SemanticActionText::try_new("Agenda: launch".into()).unwrap(),
                },
                effect,
                SemanticWaitCondition::Immediate,
                SemanticVerification::TargetValueMatchesInput,
                SemanticSettleBudget::try_new(2000).unwrap(),
            )
            .unwrap();
            SemanticActionBatch::bind(
                SemanticActionBatchId::new(1).unwrap(),
                &doc,
                &[snapshot.frame().clone()],
                vec![proposal],
            )
            .unwrap()
            .actions()[0]
                .prepare(snapshot)
                .unwrap()
        };
        let gate = Arc::new(SiteGate::new("notion.so".into(), false, false));
        let policy = SiteWorkPolicy {
            gate: gate.clone(),
            asks: true,
        };
        assert!(policy
            .assess(&fill(SemanticEffectClass::LocalWrite), &doc)
            .is_err());
        let held = gate.pending().unwrap().preview;
        assert_eq!(held.consequence, Consequence::Edit);
        assert_eq!(held.text.as_deref(), Some("Agenda: launch"));
        assert_eq!(held.headline, "Save changes on notion.so?");
        gate.approve(true).unwrap();
        assert!(gate.allow_edits());
        assert!(policy
            .assess(&fill(SemanticEffectClass::ExternalWrite), &doc)
            .is_ok());
        // Later edits need no question.
        assert!(policy
            .assess(&fill(SemanticEffectClass::ExternalWrite), &doc)
            .is_ok());
        assert!(gate.pending().is_none());
    }

    #[test]
    fn a_step_read_as_harmless_that_says_it_committed_stops_the_page() {
        let before = page(vec![
            json!({"k":1,"r":"document","o":16,"fc":true}),
            json!({"k":2,"p":0,"r":"button","n":"Done","o":9,"ak":1,"fc":true,
                "b":{"x":1,"y":1,"w":100,"h":30}}),
        ]);
        let after = page(vec![
            json!({"k":1,"r":"document","o":16,"fc":true}),
            json!({"k":3,"p":0,"r":"status","t":"Message sent","fc":true}),
        ]);
        let gate = Arc::new(SiteGate::new("slack.com".into(), false, false));
        let policy = SiteWorkPolicy {
            gate: gate.clone(),
            asks: true,
        };
        assert!(policy
            .assess(&click(&before, 2, SemanticEffectClass::LocalWrite), &before)
            .is_ok());
        policy.after_step(&after);
        assert_eq!(gate.unconfirmed().as_deref(), Some("Message sent"));
        assert_eq!(
            policy.human_wall(&after),
            Some(AgentBrowserHumanReason::Verification)
        );
    }

    #[test]
    fn a_stays_checkout_date_is_no_purchase() {
        let widget = page(vec![
            json!({"k":1,"r":"document","o":16}),
            json!({"k":2,"p":0,"r":"button","n":"Change dates; Check-in: Add date; Checkout: Add date","o":1,"ak":1}),
            json!({"k":3,"p":0,"r":"button","n":"Reserve","o":1,"ak":1}),
        ]);
        assert_eq!(
            effect(&widget, 2, SemanticActionKind::Click),
            SiteEffect::Draft
        );
        assert_eq!(
            effect(&widget, 3, SemanticActionKind::Click),
            SiteEffect::Commit(Consequence::Purchase)
        );
    }

    #[test]
    fn a_cookie_banner_choice_is_a_read_and_never_held() {
        let banner = page(vec![
            json!({"k":1,"r":"document","o":16,"fc":true}),
            json!({"k":2,"p":0,"r":"group","fc":true}),
            json!({"k":3,"p":1,"r":"paragraph","t":"We use cookies to personalise content.","fc":true}),
            json!({"k":4,"p":1,"r":"button","n":"Reject All","o":1,"ak":1,"fc":true,
                "b":{"x":1,"y":1,"w":100,"h":30}}),
            json!({"k":5,"p":1,"r":"button","n":"Accept All","o":1,"ak":1,"fc":true,
                "b":{"x":120,"y":1,"w":100,"h":30}}),
        ]);
        assert_eq!(
            effect(&banner, 4, SemanticActionKind::Click),
            SiteEffect::Read
        );
        let gate = Arc::new(SiteGate::new("lego.com".into(), false, false));
        let policy = SiteWorkPolicy {
            gate: gate.clone(),
            asks: true,
        };
        assert!(policy
            .assess(&click(&banner, 4, SemanticEffectClass::Read), &banner)
            .is_ok());
        // Declared as a message, it is asked for as the read it is.
        assert!(matches!(
            policy.assess(
                &click(&banner, 4, SemanticEffectClass::Communication),
                &banner
            ),
            Err(AgentWorkFailure::EffectRequired(SemanticEffectClass::Read))
        ));
        assert!(matches!(
            policy.assess(&click(&banner, 5, SemanticEffectClass::Read), &banner),
            Err(AgentWorkFailure::ActionDenied)
        ));
        assert!(gate.pending().is_none() && !gate.held_back());
        assert_eq!(
            policy.consent_dismissal(&banner),
            Some(banner.frames()[0].nodes()[3].reference())
        );
    }

    #[test]
    fn a_password_or_a_small_sign_in_form_is_a_wall() {
        assert!(sign_in_wall(&page(vec![
            json!({"k":1,"r":"document","o":16}),
            json!({"k":2,"p":0,"r":"password","n":"Password","q":"secret","o":2,"b":{"x":10,"y":10,"w":200,"h":30}}),
        ])));
        assert!(sign_in_wall(&page(vec![
            json!({"k":1,"r":"document","o":16}),
            json!({"k":2,"p":0,"r":"heading","n":"Sign in to your account","l":1}),
            json!({"k":3,"p":0,"r":"password","n":"Password","q":"secret","o":2}),
        ])));
        assert!(!sign_in_wall(&page(vec![
            json!({"k":1,"r":"document","o":16}),
            json!({"k":2,"p":0,"r":"password","n":"Password","q":"secret","o":2}),
        ])));
        assert!(sign_in_wall(&page(vec![
            json!({"k":1,"r":"document","o":16}),
            json!({"k":2,"p":0,"r":"heading","n":"Sign in to Notion","l":1}),
            json!({"k":3,"p":0,"r":"textbox","n":"Email","q":"sensitive","o":2,"b":{"x":10,"y":50,"w":200,"h":30}}),
        ])));
        assert!(!sign_in_wall(&page(vec![
            json!({"k":1,"r":"document","o":16}),
            json!({"k":2,"p":0,"r":"heading","n":"Inbox","l":1}),
            json!({"k":3,"p":0,"r":"button","n":"Log in to another account","o":1,"ak":1}),
        ])));
    }
}
