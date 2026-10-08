//! `create`, `revise` and `read_canvas`: model JSON in, validated objects on
//! the canvas out. Items name their sources by key; Rust maps keys to the
//! artifact's evidence and every limit comes back as a fault to correct.
use serde_json::{Map, Value};
use zephium_core::work::{artifact::*, runtime::*, *};

use super::call::clip;
use super::run::LeadRun;

pub const MAX_TITLE_CHARS: usize = 60;
const MAX_CANVAS_LINES: usize = 60;
const MAX_READ_BYTES: usize = 24 * 1024;

/// One object on this work's canvas.
#[derive(Clone)]
pub(crate) struct CanvasObject {
    pub artifact: WorkArtifactV1,
    pub current: bool,
    pub in_this_run: bool,
    pub part_title: Option<String>,
}

/// Objects the runs of this work placed, oldest first; records a page gave
/// a part are sources, not objects.
pub(crate) fn canvas(
    projection: &WorkRuntimeProjection,
    current: WorkExecutionId,
) -> Vec<CanvasObject> {
    let mut objects = Vec::new();
    for execution in &projection.executions {
        for step in &execution.steps {
            if !matches!(step.kind, WorkStepKindV1::Publish) {
                continue;
            }
            for id in &step.artifacts {
                if let Some(artifact) = execution.artifacts.iter().find(|a| a.id == *id) {
                    let part_title = artifact.part.and_then(|part| {
                        execution
                            .parts
                            .iter()
                            .find(|p| p.id == part)
                            .map(|p| p.title.clone())
                    });
                    objects.push(CanvasObject {
                        artifact: artifact.clone(),
                        current: true,
                        in_this_run: execution.id == current,
                        part_title,
                    });
                }
            }
        }
    }
    let revised: Vec<WorkArtifactId> = objects.iter().filter_map(|o| o.artifact.revises).collect();
    for object in &mut objects {
        object.current = !revised.contains(&object.artifact.id);
    }
    objects
}

/// The newest version of the object `id` names.
pub(crate) fn newest(objects: &[CanvasObject], id: WorkArtifactId) -> Option<&CanvasObject> {
    let mut at = objects.iter().find(|o| o.artifact.id == id)?;
    for _ in 0..objects.len() {
        match objects
            .iter()
            .find(|o| o.artifact.revises == Some(at.artifact.id))
        {
            Some(newer) => at = newer,
            None => break,
        }
    }
    Some(at)
}

fn summary(artifact: &WorkArtifactV1) -> String {
    let text = artifact.data.plain_text();
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && *line != artifact.title)
        .unwrap_or("");
    clip(line, 110)
}

fn count(data: &WorkArtifactDataV1) -> Option<String> {
    let (n, noun) = match data {
        WorkArtifactDataV1::Picks { items, .. } => (items.len(), "items"),
        WorkArtifactDataV1::Plan { steps, .. } => (steps.len(), "steps"),
        WorkArtifactDataV1::List { items, .. } => (items.len(), "items"),
        WorkArtifactDataV1::Sheet { rows, .. } => (rows.len(), "rows"),
        WorkArtifactDataV1::Diagram { nodes, .. } => (nodes.len(), "nodes"),
        _ => return None,
    };
    Some(format!("{n} {noun}"))
}

/// The compact view a turn carries: current objects only, newest last.
pub(crate) fn view(objects: &[CanvasObject]) -> String {
    let current: Vec<&CanvasObject> = objects.iter().filter(|o| o.current).collect();
    let skip = current.len().saturating_sub(MAX_CANVAS_LINES);
    let mut out = String::new();
    for object in &current[skip..] {
        let artifact = &object.artifact;
        out.push_str(&format!(
            "- {} {} \"{}\"",
            artifact.id,
            artifact.data.kind_name(),
            artifact.title
        ));
        if let Some(part) = &object.part_title {
            out.push_str(&format!(" · part {part}"));
        }
        if let Some(count) = count(&artifact.data) {
            out.push_str(&format!(" · {count}"));
        }
        if artifact.revises.is_some() {
            out.push_str(" · updated");
        }
        if object.in_this_run {
            out.push_str(" · this request");
        }
        let line = summary(artifact);
        if !line.is_empty() {
            out.push_str(&format!(" — {line}"));
        }
        out.push('\n');
    }
    if skip > 0 {
        out.insert_str(0, &format!("({skip} older objects not listed)\n"));
    }
    out
}

/// Full data of the named objects, their items' sources given as keys.
pub(crate) fn read(run: &LeadRun, objects: &[CanvasObject], ids: &[String]) -> String {
    let mut out = Vec::new();
    let mut bytes = 0;
    for id in ids.iter().take(8) {
        let Some(found) = WorkArtifactId::parse(id.trim())
            .and_then(|id| objects.iter().find(|o| o.artifact.id == id))
        else {
            out.push(serde_json::json!({"id": id, "error": "no such object on this canvas"}));
            continue;
        };
        let artifact = &found.artifact;
        let keys: Vec<String> = artifact
            .evidence
            .iter()
            .map(|link| run.cite(link.clone(), "", None))
            .collect();
        let mut data = serde_json::to_value(&artifact.data).unwrap_or(Value::Null);
        items_mut(&mut data, |item| {
            if let Some(index) = item.get("source").and_then(Value::as_u64) {
                if let Some(key) = keys.get(index as usize) {
                    item.insert("source".into(), Value::String(key.clone()));
                }
            }
        });
        let mut record = serde_json::json!({
            "id": artifact.id.to_string(),
            "title": artifact.title,
            "kind": artifact.data.kind_name(),
            "data": data,
            "sources": keys,
        });
        if !found.current {
            if let Some(newer) = newest(objects, artifact.id) {
                record["newer_version"] = Value::String(newer.artifact.id.to_string());
            }
        }
        let size = record.to_string().len();
        if bytes + size > MAX_READ_BYTES {
            out.push(serde_json::json!({"id": id, "error": "too large to return with the others; read it alone"}));
            continue;
        }
        bytes += size;
        out.push(record);
    }
    Value::Array(out).to_string()
}

/// Calls `f` on every item object of a kind's item arrays.
fn items_mut(data: &mut Value, mut f: impl FnMut(&mut Map<String, Value>)) {
    for field in ["items", "steps", "rows"] {
        if let Some(Value::Array(items)) = data.get_mut(field) {
            for item in items.iter_mut() {
                if let Value::Object(item) = item {
                    f(item);
                }
            }
        }
    }
}

/// Plain values where the wire wants decimal strings; models send numbers.
fn normalize(kind: &str, data: &mut Value) {
    fn stringify(value: &mut Value) {
        match value {
            Value::Number(number) => *value = Value::String(number.to_string()),
            Value::Bool(flag) => *value = Value::String(if *flag { "yes" } else { "no" }.into()),
            Value::Null => *value = Value::String(String::new()),
            _ => {}
        }
    }
    fn trim(value: &mut Value) {
        match value {
            Value::String(text) => {
                let trimmed = text.trim();
                if trimmed.len() != text.len() {
                    *text = trimmed.to_owned();
                }
            }
            Value::Array(items) => items.iter_mut().for_each(trim),
            Value::Object(map) => map.values_mut().for_each(trim),
            _ => {}
        }
    }
    // Code and diff lines keep their indentation.
    if !matches!(kind, "code" | "diff") {
        trim(data);
    }
    // Headlines, names and labels are set as type: no Markdown marks.
    for field in ["headline"] {
        if let Some(Value::String(text)) = data.get_mut(field) {
            *text = super::call::plain(text);
        }
    }
    items_mut(data, |item| {
        for field in ["name", "title"] {
            if let Some(Value::String(text)) = item.get_mut(field) {
                *text = super::call::plain(text);
            }
        }
    });
    match kind {
        "picks" => items_mut(data, |item| {
            if let Some(price) = item.get_mut("price").and_then(Value::as_object_mut) {
                if let Some(amount) = price.get_mut("amount") {
                    stringify(amount);
                }
            }
            if let Some(rating) = item.get_mut("rating").and_then(Value::as_object_mut) {
                if let Some(value) = rating.get_mut("value") {
                    stringify(value);
                }
            }
        }),
        "sheet" => items_mut(data, |row| {
            if let Some(Value::Array(cells)) = row.get_mut("cells") {
                cells.iter_mut().for_each(stringify);
            }
        }),
        "plot" => {
            if let Some(Value::Array(series)) = data.get_mut("series") {
                for series in series {
                    if let Some(Value::Array(points)) = series.get_mut("points") {
                        for point in points.iter_mut().filter_map(Value::as_object_mut) {
                            if let Some(x) = point.get_mut("x") {
                                stringify(x);
                            }
                            for key in ["y", "y2"] {
                                match point.get(key) {
                                    Some(Value::Null) => {
                                        point.remove(key);
                                    }
                                    Some(_) => stringify(point.get_mut(key).expect("present")),
                                    None => {}
                                }
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// What a create or revise asks for, parsed and validated.
pub struct Proposed {
    pub title: String,
    pub data: WorkArtifactDataV1,
    pub evidence: Vec<WorkEvidenceLink>,
    /// Optional parts a second try placed the object without, in words.
    pub left_out: Vec<String>,
}

/// Why an object was refused, as a closed fact for development logs.
#[derive(Clone, Copy, Debug)]
pub enum ObjectRefusal {
    Title,
    Shape,
    Source,
    Links,
    Pick,
    Field(WorkArtifactField),
    /// It stood for a failure or claimed more than the run found.
    Honesty,
    /// Another object already covers its subject.
    Duplicate,
    /// A second reply, or a part's second object.
    Second,
}

/// Optional parts a second try may leave out before it refuses.
const MAX_LEFT_OUT: usize = 24;

/// Turns a model's object into validated data, or a fault naming the exact
/// place. `lenient` is a second try of the same object: fields the kind does
/// not have and optional parts outside their limits are left out, and the
/// object stands without them. Required text is never cut.
#[allow(clippy::too_many_arguments)]
pub(crate) fn propose(
    run: &LeadRun,
    objects: &[CanvasObject],
    kind: &str,
    title: &str,
    data: Value,
    sources: &[String],
    inherited: &[WorkEvidenceLink],
    lenient: bool,
) -> Result<Proposed, (String, ObjectRefusal)> {
    let title = super::call::plain(title);
    let title = title.trim();
    if title.is_empty() || title.chars().count() > MAX_TITLE_CHARS || title.contains('\n') {
        return Err((
            format!(
                "title is {} characters; it is one line of 1 to {MAX_TITLE_CHARS}",
                title.chars().count()
            ),
            ObjectRefusal::Title,
        ));
    }
    let Value::Object(mut map) = data else {
        return Err((
            "data must be an object with the kind's fields".into(),
            ObjectRefusal::Shape,
        ));
    };
    map.remove("kind");
    let mut data = Value::Object(map);
    super::schema::strip_schema_words(kind, &mut data);
    let mut left_out: Vec<String> = Vec::new();
    if lenient {
        for path in super::schema::prune(kind, &mut data) {
            left_out.push(format!("{path} (not a field of {kind})"));
        }
    }
    if let Some(found) = super::schema::mismatch(kind, &data) {
        return Err((format!("{kind}: {}", found.words), ObjectRefusal::Shape));
    }
    normalize(kind, &mut data);
    let mut keys: Vec<String> = Vec::new();
    for key in sources {
        let key = key.trim().to_owned();
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    items_mut(&mut data, |item| {
        if let Some(Value::String(key)) = item.get("source") {
            let key = key.trim().to_owned();
            if !keys.contains(&key) {
                keys.push(key.clone());
            }
            let index = keys.iter().position(|k| *k == key).unwrap_or(0);
            item.insert("source".into(), Value::from(index as u64));
        } else if item.get("source").is_some_and(|v| !v.is_number()) {
            item.remove("source");
        }
    });
    let mut evidence = Vec::new();
    let mut unknown = None;
    for key in &keys {
        match run.source(key) {
            Some(source) => evidence.push(source.link),
            None => {
                unknown.get_or_insert_with(|| key.clone());
            }
        }
    }
    if let Some(key) = unknown {
        return Err((
            format!("sources: {key} is not a source of this run; use the keys search, reads and parts returned"),
            ObjectRefusal::Source,
        ));
    }
    if evidence.is_empty() {
        evidence = inherited.to_vec();
    }
    if evidence.len() > MAX_ARTIFACT_EVIDENCE {
        return Err((
            format!(
                "sources has {} keys; the limit is {MAX_ARTIFACT_EVIDENCE}",
                evidence.len()
            ),
            ObjectRefusal::Source,
        ));
    }
    if let Value::Object(map) = &mut data {
        map.insert("kind".into(), Value::String(kind.to_owned()));
    }
    let data = loop {
        let parsed: WorkArtifactDataV1 = serde_json::from_value(data.clone()).map_err(|error| {
            (
                format!("data does not match the {kind} shape: {error}"),
                ObjectRefusal::Shape,
            )
        })?;
        let Some(fault) = parsed.lead_fault(evidence.len()) else {
            break parsed;
        };
        if lenient && left_out.len() < MAX_LEFT_OUT && super::schema::drop(&mut data, &fault) {
            left_out.push(left_words(&fault));
            continue;
        }
        return Err((
            format!("{kind}: {}", fault.describe()),
            ObjectRefusal::Field(fault.field),
        ));
    };
    if evidence.is_empty() && data.claims_observed_links() {
        return Err((
            format!("{kind}: pictures and links must come from sources; list the keys they came from in sources"),
            ObjectRefusal::Links,
        ));
    }
    let mut data = data;
    // The canvas fetches pictures by themselves, so once the run holds the
    // person's data a picture address must be one a page showed.
    if run.is_private() {
        let dropped = keep_shown_pictures(&mut data, |url| run.known_url(url));
        if dropped > 0 {
            left_out.push(format!(
                "{dropped} picture address(es) no page showed; use the photo addresses reads returned"
            ));
        }
    }
    if let WorkArtifactDataV1::Plan { steps, .. } = &mut data {
        for (index, step) in steps.iter_mut().enumerate() {
            let Some(pick) = &step.pick else { continue };
            let ok = objects.iter().any(|o| {
                o.artifact.id == pick.artifact
                    && matches!(&o.artifact.data, WorkArtifactDataV1::Picks { items, .. } if usize::from(pick.index) < items.len())
            });
            if ok {
                continue;
            }
            if lenient {
                step.pick = None;
                left_out.push(format!("steps[{index}].pick (no such pick on the canvas)"));
                continue;
            }
            return Err((
                format!(
                    "plan: steps[{index}].pick names a pick that is not on the canvas; use a picks object id and an item index from it"
                ),
                ObjectRefusal::Pick,
            ));
        }
    }
    Ok(Proposed {
        title: title.to_owned(),
        data,
        evidence,
        left_out,
    })
}

/// What a second try left out, for the model: the place and why.
fn left_words(fault: &zephium_core::work::objects::WorkObjectFault) -> String {
    use zephium_core::work::objects::{WorkFaultDrop, WorkTextFound};
    let place = fault
        .drop
        .map(|drop| fault.fill(drop.path()))
        .unwrap_or_default();
    let why = match (fault.found, fault.limit) {
        (Some(WorkTextFound::Characters(n)), Some(limit)) => {
            format!("{n} characters; the limit is {limit}")
        }
        (Some(WorkTextFound::Items(n)), Some(limit)) => {
            format!("{n} items; the first {limit} stay")
        }
        (Some(WorkTextFound::LineBreak), _) => "it had a line break".into(),
        _ => fault.field.phrase().to_owned(),
    };
    match fault.drop {
        Some(WorkFaultDrop::Empty(_)) => format!("{place} emptied ({why})"),
        _ => format!("{place} ({why})"),
    }
}

/// Places a new object or a revision, durably, as one Publish step.
pub(crate) async fn publish(
    run: &LeadRun,
    proposed: Proposed,
    part: Option<WorkPartId>,
    revises: Option<WorkArtifactId>,
) -> Result<WorkArtifactId, WorkError> {
    let kind = proposed.data.kind_name();
    let artifact = WorkArtifactV1 {
        version: 1,
        id: WorkArtifactId::generate(),
        execution: run.probe.execution(),
        node: run.probe.node(),
        attempt: run.probe.attempt(),
        output: run.output.name.clone(),
        title: proposed.title,
        general_knowledge: proposed.evidence.is_empty(),
        data: proposed.data,
        evidence: proposed.evidence,
        review: run.output.review,
        presentation: WorkArtifactPresentationV1::Automatic,
        revises,
        part,
    };
    artifact.validate()?;
    let id = artifact.id;
    let mut step = run.step(WorkStepKindV1::Publish, WorkStepStatus::Succeeded, part);
    step.artifacts = vec![id];
    step.note = Some(if revises.is_some() {
        format!("Updated the {}", noun(kind))
    } else {
        format!("Placed the {}", noun(kind))
    });
    run.begin(step, vec![artifact]).await?;
    Ok(id)
}

fn noun(kind: &str) -> &'static str {
    match kind {
        "reply" => "answer",
        "picks" => "picks",
        "plan" => "plan",
        "list" => "list",
        "sheet" => "table",
        "plot" => "chart",
        "diagram" => "diagram",
        "code" => "code",
        "diff" => "change",
        "document" => "document",
        "draft" => "draft",
        "media" => "media",
        "project" => "project",
        _ => "object",
    }
}

/// Words that say a value was not found or not checked.
const NOT_FOUND: [&str; 12] = [
    "not verified",
    "unverified",
    "could not",
    "couldn't",
    "can't access",
    "cannot access",
    "not retrieved",
    "not captured",
    "not obtained",
    "not found",
    "lookup unavailable",
    "no exact",
];
/// What a figure says when it has no value.
const NO_VALUE: [&str; 5] = [
    "unknown",
    "unavailable",
    "not available",
    "n/a",
    "none found",
];
/// The agent's own unfinished work, as a to-do.
const REDO: [&str; 9] = [
    "retry",
    "try again",
    "recheck",
    "check again",
    "rerun",
    "sign in",
    "grant",
    "allow access",
    "give access",
];
/// Words that claim a finished or checked result.
const CLAIMS: [&str; 7] = [
    "complete",
    "verified",
    "confirmed",
    "all set",
    "ready",
    "success",
    "done",
];
/// Tags that stand in for the one recommended mark.
const PICK_TAGS: [&str; 5] = [
    "top pick",
    "best pick",
    "our pick",
    "recommended",
    "best choice",
];

fn says(text: &str, words: &[&str]) -> bool {
    affirms(text, words, false)
}

/// Words that turn a claim after them into its opposite.
const NEGATIONS: [&str; 12] = [
    "not",
    "no",
    "never",
    "couldn't",
    "couldn’t",
    "can't",
    "can’t",
    "cannot",
    "isn't",
    "isn’t",
    "wasn't",
    "wasn’t",
];

/// A claim the text makes: one of `words` as a whole word, and with
/// `negation`, not denied by one of the three words before it ("could not
/// complete", "isn't ready").
fn affirms(text: &str, words: &[&str], negation: bool) -> bool {
    let text = text.to_lowercase();
    words.iter().any(|word| {
        text.match_indices(word).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let after = text[at + word.len()..].chars().next();
            let whole = !before.is_some_and(char::is_alphanumeric)
                && !after.is_some_and(char::is_alphanumeric);
            let denied = negation
                && text[..at]
                    .split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '’'))
                    .filter(|w| !w.is_empty())
                    .rev()
                    .take(3)
                    .any(|w| NEGATIONS.contains(&w));
            whole && !denied
        })
    })
}

/// What the run knows when an object is proposed.
pub(crate) struct Honesty {
    /// The object rests on sources.
    pub sourced: bool,
    /// Names of this request's parts that could not do their job, and
    /// their services, in lower case: "slack", "flights".
    pub failed: Vec<String>,
}

/// Refuses an object that stands for a failure or claims more than the run
/// found: a figure or pick that says it was not found, a to-do to retry the
/// agent's own work, a recommended pick without sources, or a headline that
/// calls the result complete while a part failed.
pub(crate) fn honest(data: &WorkArtifactDataV1, run: &Honesty) -> Result<(), String> {
    match data {
        WorkArtifactDataV1::Reply {
            headline, figures, ..
        } => {
            if let Some(at) = figures.iter().position(|f| {
                says(&f.value, &NOT_FOUND)
                    || says(&f.value, &NO_VALUE)
                    || says(&f.label, &NOT_FOUND)
            }) {
                return Err(format!("reply: figure {} stands for something that was not found; a figure shows a value that was found. Leave it out and say what is missing in the text", at + 1));
            }
            if !run.failed.is_empty() && affirms(headline, &CLAIMS, true) {
                return Err("reply: the headline calls the result finished or checked while a part could not do its job; say what was found".into());
            }
        }
        WorkArtifactDataV1::Picks { items, .. } => {
            for (at, item) in items.iter().enumerate() {
                let mut text = vec![item.name.as_str()];
                text.extend(item.subtitle.as_deref());
                text.extend(item.price.as_ref().map(|p| p.display.as_str()));
                text.extend(item.facts.iter().map(|f| f.value.as_str()));
                if text.iter().any(|t| says(t, &NOT_FOUND)) {
                    return Err(format!("picks: item {} says it could not be found or checked; picks hold only things that were found. Leave it out", at + 1));
                }
                let tagged = item.tags.iter().any(|tag| says(tag, &PICK_TAGS));
                if (item.recommended || tagged) && !run.sourced {
                    return Err(format!("picks: item {} is marked recommended without sources; recommend only what your sources show, and never with a tag", at + 1));
                }
                if tagged {
                    return Err(format!("picks: item {} carries a recommendation as a tag; set recommended on it instead", at + 1));
                }
            }
        }
        WorkArtifactDataV1::List { items, .. } => {
            let about_failure = |title: &str| {
                let lower = title.to_lowercase();
                (says(title, &REDO) || says(title, &NOT_FOUND))
                    && run.failed.iter().any(|name| lower.contains(name.as_str()))
            };
            if let Some(at) = items.iter().position(|item| about_failure(&item.title)) {
                return Err(format!("list: item {} is about work that could not be done; a list holds what was found, and a part's failure shows on its own row with its fix", at + 1));
            }
        }
        WorkArtifactDataV1::Sheet { columns, rows, .. } => {
            for (at, row) in rows.iter().enumerate() {
                let failed = row.cells.iter().zip(columns).any(|(cell, column)| {
                    column.kind == zephium_core::work::objects::WorkSheetColumnKindV1::Text
                        && says(cell, &NOT_FOUND)
                });
                if failed {
                    return Err(format!("sheet: row {} has a cell that says a value was not found; an unknown cell stays empty", at + 1));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// The object without the parts `honest` refuses: cells that say a value
/// was not found stand empty, such figures and facts are left out. None when
/// what is left would still be refused or nothing changed.
pub(crate) fn mend(
    data: &WorkArtifactDataV1,
    run: &Honesty,
) -> Option<(WorkArtifactDataV1, String)> {
    let mut data = data.clone();
    let mut mended = 0usize;
    match &mut data {
        WorkArtifactDataV1::Reply { figures, .. } => {
            let before = figures.len();
            figures.retain(|f| {
                !(says(&f.value, &NOT_FOUND)
                    || says(&f.value, &NO_VALUE)
                    || says(&f.label, &NOT_FOUND))
            });
            mended = before - figures.len();
        }
        WorkArtifactDataV1::Picks { items, .. } => {
            for item in items.iter_mut() {
                let before = item.facts.len();
                item.facts.retain(|f| !says(&f.value, &NOT_FOUND));
                mended += before - item.facts.len();
            }
        }
        WorkArtifactDataV1::Sheet { columns, rows, .. } => {
            for row in rows.iter_mut() {
                for (cell, column) in row.cells.iter_mut().zip(columns.iter()) {
                    if column.kind == zephium_core::work::objects::WorkSheetColumnKindV1::Text
                        && says(cell, &NOT_FOUND)
                    {
                        cell.clear();
                        mended += 1;
                    }
                }
            }
        }
        _ => {}
    }
    (mended > 0 && honest(&data, run).is_ok()).then(|| {
        (
            data,
            format!("{mended} values that said they were not found (left empty)"),
        )
    })
}

/// Significant words of a title, for telling whether two objects are about
/// the same subject.
fn subject(title: &str) -> Vec<String> {
    const SMALL: [&str; 16] = [
        "a", "an", "the", "of", "for", "and", "to", "in", "on", "with", "your", "my", "by", "vs",
        "new", "updated",
    ];
    let mut words: Vec<String> = title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let w = w.to_lowercase();
            match w.strip_suffix('s') {
                Some(stem) if stem.len() > 2 => stem.to_owned(),
                _ => w,
            }
        })
        .filter(|w| !SMALL.contains(&w.as_str()))
        .collect();
    words.sort();
    words.dedup();
    words
}

/// Two titles name the same subject when most words of the shorter one are
/// in the longer one.
pub(crate) fn same_subject(a: &str, b: &str) -> bool {
    let (a, b) = (subject(a), subject(b));
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if short.is_empty() {
        return false;
    }
    let shared = short.iter().filter(|w| long.contains(w)).count();
    shared * 3 >= short.len() * 2
}

/// A current object of the same kind about the same subject: a follow-up
/// revises it instead of placing a copy. Replies belong to their request,
/// and the exact records a part's tools made stay as they are.
pub(crate) fn duplicate<'a>(
    objects: &'a [CanvasObject],
    kind: &str,
    title: &str,
    part: Option<WorkPartId>,
) -> Option<&'a CanvasObject> {
    if matches!(kind, "reply" | "diff" | "project" | "media") {
        return None;
    }
    objects.iter().find(|object| {
        object.current
            && object.artifact.data.kind_name() == kind
            && (object.artifact.part.is_none() || object.artifact.part != part)
            && same_subject(&object.artifact.title, title)
    })
}

/// A part's picks of this run that already hold most of these items: the
/// lead's picks of the same things would stand twice on the canvas.
pub(crate) fn part_holds<'a>(
    objects: &'a [CanvasObject],
    names: &[String],
) -> Option<&'a CanvasObject> {
    let names: Vec<String> = names.iter().map(|n| n.trim().to_lowercase()).collect();
    if names.is_empty() {
        return None;
    }
    objects.iter().find(|object| {
        let WorkArtifactDataV1::Picks { items, .. } = &object.artifact.data else {
            return false;
        };
        object.current
            && object.in_this_run
            && object.artifact.part.is_some()
            && names
                .iter()
                .filter(|name| {
                    items.iter().any(|item| {
                        let held = item.name.trim().to_lowercase();
                        held == **name
                            || (name.len() >= 4 && held.contains(name.as_str()))
                            || (held.len() >= 4 && name.contains(held.as_str()))
                    })
                })
                .count()
                * 2
                >= names.len()
    })
}

/// Removes picture addresses `shown` does not know from the pictures the
/// canvas fetches on its own, and says how many went.
fn keep_shown_pictures(data: &mut WorkArtifactDataV1, shown: impl Fn(&str) -> bool) -> usize {
    let mut dropped = 0;
    let mut keep = |candidates: &mut Vec<String>| {
        let before = candidates.len();
        candidates.retain(|url| shown(url));
        dropped += before - candidates.len();
    };
    match data {
        WorkArtifactDataV1::Picks { items, .. } => items
            .iter_mut()
            .for_each(|item| keep(&mut item.image_candidates)),
        WorkArtifactDataV1::ComparisonMatrix { subjects, .. }
        | WorkArtifactDataV1::Findings { subjects, .. }
        | WorkArtifactDataV1::EvidenceCollection { subjects, .. } => subjects
            .iter_mut()
            .for_each(|subject| keep(&mut subject.image_candidates)),
        _ => {}
    }
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_private_run_keeps_only_the_pictures_pages_showed() {
        let mut data: WorkArtifactDataV1 = serde_json::from_value(serde_json::json!({
            "kind": "picks",
            "facet": "stay",
            "items": [{"name": "Loft", "image_candidates": [
                "https://img.example.com/loft.jpg",
                "https://secret-words.collector.example/p.png"
            ]}]
        }))
        .unwrap();
        let dropped =
            keep_shown_pictures(&mut data, |url| url == "https://img.example.com/loft.jpg");
        assert_eq!(dropped, 1);
        let WorkArtifactDataV1::Picks { items, .. } = data else {
            panic!()
        };
        assert_eq!(
            items[0].image_candidates,
            ["https://img.example.com/loft.jpg"]
        );
    }

    #[test]
    fn a_follow_up_about_the_same_subject_is_the_same_object() {
        assert!(same_subject(
            "Modern AI SaaS architecture",
            "Modern AI SaaS architecture — provider recommendation"
        ));
        assert!(same_subject(
            "Warsaw–San Francisco flights",
            "Flights Warsaw to San Francisco"
        ));
        assert!(!same_subject(
            "Modern browser architecture",
            "AI coding agent architecture"
        ));
        assert!(!same_subject("Homes near YC", "Flights to SFO"));
    }

    #[test]
    fn failures_never_stand_as_results() {
        let honest_run = Honesty {
            sourced: true,
            failed: vec!["slack".into(), "flights".into()],
        };
        let reply: WorkArtifactDataV1 = serde_json::from_value(serde_json::json!({
            "kind": "reply", "headline": "Airfare recheck complete", "text": "The lookup failed.",
            "figures": [{"label": "Exact-date fare", "value": "Not verified"}]
        }))
        .unwrap();
        assert!(honest(&reply, &honest_run)
            .unwrap_err()
            .contains("figure 1"));
        let headline: WorkArtifactDataV1 = serde_json::from_value(serde_json::json!({
            "kind": "reply", "headline": "Airfare recheck complete", "text": "No fare was found."
        }))
        .unwrap();
        assert!(honest(&headline, &honest_run).is_err());
        assert!(honest(
            &headline,
            &Honesty {
                sourced: true,
                failed: vec![]
            }
        )
        .is_ok());
        let said: WorkArtifactDataV1 = serde_json::from_value(serde_json::json!({
            "kind": "reply", "headline": "I can't access Slack yet", "text": "Sign in to Slack and ask again."
        }))
        .unwrap();
        assert!(honest(&said, &honest_run).is_ok());
        for headline in [
            "I couldn't read Slack",
            "The Slack check could not complete",
            "Your day plan isn't ready",
        ] {
            let plain: WorkArtifactDataV1 = serde_json::from_value(serde_json::json!({
                "kind": "reply", "headline": headline,
                "text": "I couldn't read Slack: its page kept changing under me. Try again or use the Slack connection."
            }))
            .unwrap();
            assert!(honest(&plain, &honest_run).is_ok(), "{headline}");
        }
        let todo: WorkArtifactDataV1 = serde_json::from_value(serde_json::json!({
            "kind": "list", "style": "todo", "items": [{"title": "Retry the Slack check"}]
        }))
        .unwrap();
        assert!(honest(&todo, &honest_run).is_err());
        let work: WorkArtifactDataV1 = serde_json::from_value(serde_json::json!({
            "kind": "list", "style": "todo", "items": [{"title": "Retry the failed payment for invoice 12"},
            {"title": "Unblock Ana on the deck"}]
        }))
        .unwrap();
        assert!(honest(&work, &honest_run).is_ok());
        let pick: WorkArtifactDataV1 = serde_json::from_value(serde_json::json!({
            "kind": "picks", "facet": "flight", "items": [{"name": "WAW → SFO", "recommended": true,
            "price": {"display": "$869 route-wide signal"},
            "facts": [{"label": "Fare", "value": "Exact-date fare not verified", "kind": "partial"}]}]
        }))
        .unwrap();
        assert!(honest(&pick, &honest_run).is_err());
        let unsourced: WorkArtifactDataV1 = serde_json::from_value(serde_json::json!({
            "kind": "picks", "facet": "service", "items": [{"name": "Hetzner", "recommended": true}]
        }))
        .unwrap();
        assert!(honest(
            &unsourced,
            &Honesty {
                sourced: false,
                failed: vec![]
            }
        )
        .is_err());
        let tagged: WorkArtifactDataV1 = serde_json::from_value(serde_json::json!({
            "kind": "picks", "facet": "service", "items": [{"name": "Hetzner", "tags": ["Top pick"]}]
        }))
        .unwrap();
        assert!(honest(&tagged, &honest_run).is_err());
    }
}
