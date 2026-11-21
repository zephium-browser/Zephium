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

/// The pages behind this run's current objects: what a reply that names no
/// sources of its own rests on. Empty when they are general knowledge too.
pub(crate) fn run_evidence(objects: &[CanvasObject]) -> Vec<WorkEvidenceLink> {
    let mut links: Vec<WorkEvidenceLink> = Vec::new();
    for link in objects
        .iter()
        .filter(|object| object.current && object.in_this_run)
        .flat_map(|object| &object.artifact.evidence)
    {
        if links.len() == MAX_ARTIFACT_EVIDENCE {
            break;
        }
        if !links.contains(link) {
            links.push(link.clone());
        }
    }
    links
}

/// What a revision that names no sources rests on: a reply without pages
/// takes this run's, a plan whose pages came from its picks none (so its new
/// picks give them), and anything else keeps its own.
pub(crate) fn revised_evidence(
    objects: &[CanvasObject],
    target: &CanvasObject,
) -> Vec<WorkEvidenceLink> {
    let evidence = &target.artifact.evidence;
    match &target.artifact.data {
        WorkArtifactDataV1::Reply { .. } if evidence.is_empty() => run_evidence(objects),
        data @ WorkArtifactDataV1::Plan { .. }
            if !evidence.is_empty() && {
                let picked = pick_evidence(objects, data);
                evidence.iter().all(|link| picked.contains(link))
            } =>
        {
            Vec::new()
        }
        _ => evidence.clone(),
    }
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

/// A canvas draft as the person last left it, labelled for a page task to
/// enter exactly: To, Subject and the body, line breaks kept.
pub(crate) fn draft_words(
    projection: &WorkRuntimeProjection,
    current: WorkExecutionId,
    id: &str,
) -> Result<String, String> {
    let canvas = canvas(projection, current);
    let found = WorkArtifactId::parse(id.trim())
        .and_then(|id| newest(&canvas, id))
        .ok_or("draft is the id of a draft object on this canvas")?;
    let edited = projection
        .executions
        .iter()
        .flat_map(|execution| &execution.user_artifacts)
        .find(|state| state.artifact == found.artifact.id)
        .and_then(|state| state.edited_data.as_ref());
    let WorkArtifactDataV1::Draft {
        destination,
        to,
        subject,
        body,
        ..
    } = edited.unwrap_or(&found.artifact.data)
    else {
        return Err("that object is not a draft".into());
    };
    let mut words = String::new();
    if let Some(to) = to {
        words.push_str(&format!("To: {to}\n"));
    }
    if let Some(subject) = subject {
        words.push_str(&format!("Subject: {subject}\n"));
    }
    let label = if *destination == zephium_core::work::objects::WorkDraftDestinationV1::Email {
        "Body"
    } else {
        "Message"
    };
    words.push_str(&format!("{label}:\n{body}"));
    Ok(words)
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
        strip_native_media(&mut data);
        items_mut(&mut data, |item| {
            for field in ["source", "record_source"] {
                if let Some(index) = item.get(field).and_then(Value::as_u64) {
                    if let Some(key) = keys.get(index as usize) {
                        item.insert(field.into(), Value::String(key.clone()));
                    }
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
/// A reply set the way the canvas shows it, instead of refused for its form:
/// list lines become points, headings and quotes become prose, links keep
/// their words, and text past its limit ends at a sentence, the rest moving
/// to points while they have room.
fn shape_reply(data: &mut Value) {
    use zephium_core::work::objects::limit::REPLY_HEADLINE;
    use zephium_core::work::objects::limit::{REPLY_POINT, REPLY_POINTS, REPLY_TEXT};
    if let Some(Value::String(headline)) = data.get_mut("headline") {
        *headline = words_within(headline, REPLY_HEADLINE);
    }
    let Some(text) = data.get("text").and_then(Value::as_str).map(str::to_owned) else {
        return;
    };
    let mut points: Vec<String> = data
        .get("points")
        .and_then(Value::as_array)
        .map(|points| {
            points
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let mut prose: Vec<String> = Vec::new();
    let mut listed: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            continue;
        }
        let numbered = line
            .split_once(['.', ')'])
            .filter(|(number, rest)| {
                !number.is_empty()
                    && number.len() <= 2
                    && number.bytes().all(|b| b.is_ascii_digit())
                    && rest.starts_with(' ')
            })
            .map(|(_, rest)| rest);
        let item = ["- ", "* ", "+ ", "• "]
            .iter()
            .find_map(|mark| line.strip_prefix(mark))
            .or(numbered);
        if let Some(item) = item {
            listed.push(inline_words(item.trim()));
        } else {
            let line = line.trim_start_matches(['#', '>', ' ']);
            if line.is_empty() {
                if prose.last().is_some_and(|last| !last.is_empty()) {
                    prose.push(String::new());
                }
            } else {
                prose.push(inline_words(line));
            }
        }
    }
    let mut text = prose.join("\n").replace("\n\n\n", "\n\n");
    text = text.trim().to_owned();
    // A reply that was only a list leads with its first item.
    if text.is_empty() && !listed.is_empty() {
        text = listed.remove(0);
    }
    listed.extend(points);
    points = listed;
    if text.chars().count() > REPLY_TEXT {
        let (kept, rest) = sentences_within(&text, REPLY_TEXT);
        text = kept;
        let room = REPLY_POINTS.saturating_sub(points.len());
        let moved: Vec<String> = rest.into_iter().take(room).collect();
        points.splice(0..0, moved);
    }
    let points: Vec<Value> = points
        .into_iter()
        .filter(|point| !point.trim().is_empty())
        .take(REPLY_POINTS)
        .map(|point| Value::String(words_within(&point, REPLY_POINT)))
        .collect();
    data["text"] = Value::String(text);
    if !points.is_empty() || data.get("points").is_some() {
        data["points"] = Value::Array(points);
    }
}

/// Markdown links keep their words and images go; the canvas draws its own.
fn inline_words(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let image = rest[..open].ends_with('!');
        let Some(close) = rest[open..].find("](").map(|i| open + i) else {
            break;
        };
        let Some(end) = rest[close..].find(')').map(|i| close + i) else {
            break;
        };
        out.push_str(&rest[..if image { open - 1 } else { open }]);
        if !image {
            out.push_str(&rest[open + 1..close]);
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out.trim().to_owned()
}

/// The whole sentences that fit in `max` characters, and the sentences after.
fn sentences_within(text: &str, max: usize) -> (String, Vec<String>) {
    let mut sentences = Vec::new();
    let mut start = 0;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (index, (at, c)) in chars.iter().enumerate() {
        let next = chars.get(index + 1).map(|(_, c)| *c);
        if matches!(c, '.' | '!' | '?' | '\n') && next.is_none_or(char::is_whitespace) {
            let end = at + c.len_utf8();
            sentences.push(text[start..end].trim().to_owned());
            start = end;
        }
    }
    if start < text.len() {
        sentences.push(text[start..].trim().to_owned());
    }
    sentences.retain(|s| !s.is_empty());
    let mut kept = String::new();
    let mut taken = 0;
    for sentence in &sentences {
        let joined = if kept.is_empty() {
            sentence.clone()
        } else {
            format!("{kept} {sentence}")
        };
        if joined.chars().count() > max {
            break;
        }
        kept = joined;
        taken += 1;
    }
    if kept.is_empty() {
        return (words_within(text, max), vec![]);
    }
    (kept, sentences.split_off(taken))
}

/// At most `max` characters, cut between words with an ellipsis.
pub(crate) fn words_within(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max.saturating_sub(1)).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{}…", cut.trim_end_matches([',', ';', ':', ' ']))
}

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
        "reply" => shape_reply(data),
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
    if matches!(kind, "picks" | "sheet") && data.get("records").is_some() {
        // Records the lead chose stand on what the read returned; any item
        // values written beside them are dropped, never published.
        let records: Vec<_> = match &data["records"] {
            Value::Array(records) => records
                .iter()
                .filter_map(super::records::Selection::loose)
                .collect(),
            record => super::records::Selection::loose(record)
                .into_iter()
                .collect(),
        };
        if records.is_empty() {
            return Err((
                "records lists r keys from reads, each a key or {source, subtitle, facts, annotation, recommended}".into(),
                ObjectRefusal::Shape,
            ));
        }
        let request = super::records::Presentation {
            title: title.to_owned(),
            layout: if kind == "picks" {
                super::records::Layout::Picks
            } else {
                super::records::Layout::Table
            },
            records,
        };
        return run
            .records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .present(&request)
            .map_err(|error| (error.message, error.reason));
    }
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
    if kind == "picks" {
        tidy_picks(&mut data);
    }
    strip_native_media(&mut data);
    super::schema::strip_schema_words(kind, &mut data);
    let mut left_out: Vec<String> = Vec::new();
    if kind == "list" {
        items_mut(&mut data, |item| {
            if item.get("record_source").is_some_and(|value| {
                value
                    .as_str()
                    .is_none_or(|key| run.source(key.trim()).is_none())
            }) {
                item.remove("record_source");
                left_out.push("items[].record_source (no source of this request)".into());
            }
        });
    }
    // A field the kind does not have is left out on the first try: the
    // object stands without it and the lead learns why from its result.
    for path in super::schema::prune(kind, &mut data) {
        left_out.push(format!("{path} (not a field of {kind})"));
    }
    if let Some(found) = super::schema::mismatch(kind, &data) {
        return Err((format!("{kind}: {}", found.words), ObjectRefusal::Shape));
    }
    normalize(kind, &mut data);
    let mut value_sources = Vec::new();
    if kind == "sheet" {
        let mut uncited = Vec::new();
        value_sources = super::value_sources::ground_sheet(
            &mut data,
            sources,
            &run.source_previews(),
            lenient,
            &mut uncited,
        )
        .map_err(|error| (error, ObjectRefusal::Source))?;
        if !uncited.is_empty() {
            let rows = uncited
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            left_out.push(format!(
                "the citation of rows {rows} (their values are not on the page they cited)"
            ));
        }
    }
    // A retry leaves out a key that is no source of this run rather than
    // refusing the object again: the object then claims less, not more.
    let mut unknown_keys: Vec<String> = Vec::new();
    let mut known = |key: &str| {
        let known = !lenient || run.source(key).is_some();
        if !known && !unknown_keys.iter().any(|k| k == key) {
            unknown_keys.push(key.to_owned());
        }
        known
    };
    let mut keys: Vec<String> = Vec::new();
    for key in sources.iter().chain(&value_sources) {
        let key = key.trim().to_owned();
        if known(&key) && !keys.contains(&key) {
            keys.push(key);
        }
    }
    items_mut(&mut data, |item| {
        if let Some(Value::String(key)) = item.get("source") {
            let key = key.trim().to_owned();
            if !known(&key) {
                item.remove("source");
                return;
            }
            if !keys.contains(&key) {
                keys.push(key.clone());
            }
            let index = keys.iter().position(|k| *k == key).unwrap_or(0);
            item.insert("source".into(), Value::from(index as u64));
        } else if item.get("source").is_some_and(|v| !v.is_number()) {
            item.remove("source");
        }
    });
    // Optional picture provenance cannot discard otherwise valid text.
    items_mut(&mut data, |item| {
        let Some(value) = item.remove("record_source") else {
            return;
        };
        let Some(key) = value
            .as_str()
            .map(str::trim)
            .filter(|key| run.source(key).is_some())
        else {
            left_out.push("items[].record_source (no source of this request)".into());
            return;
        };
        if !keys.iter().any(|candidate| candidate == key) {
            if keys.len() >= MAX_ARTIFACT_EVIDENCE {
                left_out.push("items[].record_source (source limit)".into());
                return;
            }
            keys.push(key.to_owned());
        }
        let index = keys.iter().position(|candidate| candidate == key).unwrap();
        item.insert("record_source".into(), Value::from(index as u64));
    });
    if !unknown_keys.is_empty() {
        left_out.push(format!(
            "the sources {} (not sources of this run)",
            unknown_keys.join(", ")
        ));
    }
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
    let mut clipped = 0;
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
        if clipped < MAX_CLIPS && super::schema::clip_within(&mut data, &fault, words_within) {
            clipped += 1;
            continue;
        }
        // An optional part outside its limits is left out on the first try,
        // and the lead reads why in its result.
        if left_out.len() < MAX_LEFT_OUT && super::schema::drop(&mut data, &fault) {
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
    if evidence.is_empty() {
        evidence = pick_evidence(objects, &data);
    }
    Ok(Proposed {
        title: title.to_owned(),
        data,
        evidence,
        left_out,
    })
}

/// The pages behind the picks a plan's steps point to: what a plan that
/// names no sources of its own rests on.
pub(crate) fn pick_evidence(
    objects: &[CanvasObject],
    data: &WorkArtifactDataV1,
) -> Vec<WorkEvidenceLink> {
    let WorkArtifactDataV1::Plan { steps, .. } = data else {
        return Vec::new();
    };
    let mut links: Vec<WorkEvidenceLink> = Vec::new();
    for link in steps
        .iter()
        .filter_map(|step| step.pick.as_ref())
        .filter_map(|pick| objects.iter().find(|o| o.artifact.id == pick.artifact))
        .flat_map(|object| &object.artifact.evidence)
    {
        if links.len() == MAX_ARTIFACT_EVIDENCE {
            break;
        }
        if !links.contains(link) {
            links.push(link.clone());
        }
    }
    links
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
        "subject" => "summary",
        _ => "object",
    }
}

/// Words that say a value was not found or not checked.
const NOT_FOUND: [&str; 17] = [
    "not observed",
    "not shown",
    "not listed",
    "not stated",
    "not specified",
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
const CLAIMS: [&str; 9] = [
    "complete",
    "finished",
    "verified",
    "checked",
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

/// The value says it was not found rather than giving it.
pub(crate) fn says_not_found(text: &str) -> bool {
    says(text, &NOT_FOUND)
}

/// The text calls a result finished or checked.
pub(crate) fn claims_done(text: &str) -> bool {
    affirms(text, &CLAIMS, true)
}

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
            // A negation anywhere earlier in its clause denies it: "No
            // listings could be verified", "Nothing here is ready yet".
            let clause = text[..at]
                .rsplit(['.', ',', ';', ':', '!', '?', '—', '–'])
                .next()
                .unwrap_or("");
            let clause = clause.rsplit(" but ").next().unwrap_or(clause);
            // "and" or "or" starts another claim unless only modifiers come
            // between it and this one: "No issues found and pricing checked"
            // claims, "could not be checked or independently verified" does
            // not. Walking back, the nearest joint that opens a predicate of
            // its own bounds the claim. A doubtful case is read as a claim:
            // a refusal costs a retry, a missed claim publishes a false one.
            let mut joints: Vec<usize> = clause
                .match_indices(" and ")
                .map(|(at, _)| at + 5)
                .chain(clause.match_indices(" or ").map(|(at, _)| at + 4))
                .collect();
            joints.sort_unstable();
            let clause = joints
                .iter()
                .rev()
                .map(|&start| &clause[start..])
                .find(|claim| !modifiers(claim))
                .unwrap_or(clause);
            let denied = negation
                && clause
                    .split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '’'))
                    .filter(|w| !w.is_empty())
                    .any(|w| NEGATIONS.contains(&w) || matches!(w, "nothing" | "none"));
            whole && !denied
        })
    })
}

/// Words that only modify a predicate they continue: "fully", "yet",
/// "been".
fn modifiers(text: &str) -> bool {
    text.split_whitespace().all(|word| {
        (word.len() > 4 && word.ends_with("ly"))
            || matches!(
                word,
                "be" | "been"
                    | "being"
                    | "is"
                    | "was"
                    | "were"
                    | "are"
                    | "yet"
                    | "all"
                    | "also"
                    | "even"
                    | "still"
            )
    })
}

/// What the run knows when an object is proposed.
pub(crate) struct Honesty {
    /// The object rests on sources.
    pub sourced: bool,
    /// Names of this request's parts that could not do their job, and
    /// their services, in lower case: "slack", "flights".
    pub failed: Vec<String>,
    /// The request tried to read pages and could read none of them.
    pub unread: bool,
}

/// Refuses an object that stands for a failure or claims more than the run
/// found: a figure or pick that says it was not found, a to-do to retry the
/// agent's own work, a recommended pick without sources, a headline that
/// calls the result complete while a part failed, or a table or picks
/// when no page the request tried to read could be read.
pub(crate) fn honest(data: &WorkArtifactDataV1, run: &Honesty) -> Result<(), String> {
    if run.unread
        && matches!(
            data,
            WorkArtifactDataV1::Sheet { .. } | WorkArtifactDataV1::Picks { .. }
        )
    {
        return Err("no page you tried to read could be read, so a table or picks would rest on search snippets alone. Publish none: reply with what could not be read, and offer to try again".into());
    }
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
            if (!run.failed.is_empty() || run.unread) && claims_done(headline) {
                return Err("reply: the headline calls the result finished or checked while a part could not do its job or no page could be read; say what was found".into());
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
    let held = |object: &CanvasObject| -> usize {
        let WorkArtifactDataV1::Picks { items, .. } = &object.artifact.data else {
            return 0;
        };
        if !(object.current && object.in_this_run && object.artifact.part.is_some()) {
            return 0;
        }
        names
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
    };
    // The same thing found by parts on several sites is the offers side by
    // side: the lead's one picks of them is the result, not a copy.
    let parts: std::collections::BTreeSet<WorkPartId> = objects
        .iter()
        .filter(|object| held(object) > 0)
        .filter_map(|object| object.artifact.part)
        .collect();
    if parts.len() >= 2 {
        return None;
    }
    // Picks that add offers to a part's own are the comparison, not a copy.
    let count = |object: &CanvasObject| match &object.artifact.data {
        WorkArtifactDataV1::Picks { items, .. } => items.len(),
        _ => 0,
    };
    objects.iter().find(|object| {
        held(object) > 0 && held(object) * 2 >= names.len() && names.len() <= count(object)
    })
}

/// The lead's own picks of this run that already hold most of `names`: a
/// second picks of the same things is an update of the first.
pub(crate) fn lead_holds<'a>(
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
        let held = names
            .iter()
            .filter(|name| {
                items
                    .iter()
                    .any(|item| item.name.trim().to_lowercase() == **name)
            })
            .count();
        object.current
            && object.in_this_run
            && object.artifact.part.is_none()
            && held * 2 >= names.len()
    })
}

/// Overlong texts one object may have clipped before it is refused.
const MAX_CLIPS: usize = 24;

/// Hand-written picks in the shapes models send: a `title` for the name, a
/// fact without its kind (plain text), a fact as "Label: value".
fn tidy_picks(data: &mut Value) {
    // One pick is the recommendation: the first the lead marked.
    let mut recommended = false;
    items_mut(data, |item| {
        if item.get("recommended").and_then(Value::as_bool) == Some(true) {
            if recommended {
                item.remove("recommended");
            }
            recommended = true;
        }
        if item.get("name").is_none() {
            if let Some(title) = item.remove("title") {
                item.insert("name".into(), title);
            }
        }
        if let Some(Value::Array(facts)) = item.get_mut("facts") {
            for fact in facts.iter_mut() {
                if let Value::String(line) = fact {
                    if let Some((label, value)) = line.split_once(':') {
                        *fact = serde_json::json!({"label": label.trim(), "value": value.trim()});
                    }
                }
                if let Value::Object(fact) = fact {
                    fact.entry("kind")
                        .or_insert_with(|| Value::String("text".into()));
                }
            }
            facts.retain(Value::is_object);
        }
    });
}

/// Native references never enter or survive the model JSON vocabulary.
pub(crate) fn strip_native_media(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("record_media");
            object.remove("media_attempted");
            for value in object.values_mut() {
                strip_native_media(value);
            }
        }
        Value::Array(items) => {
            for item in items {
                strip_native_media(item);
            }
        }
        _ => {}
    }
}
/// Hydrates only exact archived evidence identities. Labels and URLs cannot
/// borrow another record's pixels. Ambiguous source membership yields no image.
pub(crate) fn hydrate_record_media(
    proposed: &mut Proposed,
    executions: &[WorkExecutionFact],
    profile: zephium_core::ids::ProfileId,
    work: WorkId,
) {
    let lookup = |wanted: &WorkEvidenceLink| {
        let mut matched = false;
        let mut image = None;
        let mut ambiguous = false;
        for artifact in executions.iter().flat_map(|execution| &execution.artifacts) {
            if let WorkArtifactDataV1::ComparisonMatrix {
                subjects, cells, ..
            } = &artifact.data
            {
                for (index, subject) in subjects.iter().enumerate() {
                    let matches = cells.get(index).is_some_and(|cells| {
                        cells
                            .iter()
                            .flat_map(|cell| &cell.evidence)
                            .any(|index| artifact.evidence.get(usize::from(*index)) == Some(wanted))
                    });
                    if matches {
                        if matched
                            && image
                                != Some((subject.record_media.clone(), subject.media_attempted))
                        {
                            ambiguous = true;
                        }
                        matched = true;
                        image = Some((subject.record_media.clone(), subject.media_attempted));
                    }
                }
            }
        }
        if ambiguous {
            Some((None, true))
        } else {
            image
        }
    };
    if let WorkArtifactDataV1::List { items, .. } = &mut proposed.data {
        for item in items {
            item.record_media = None;
            item.media_attempted = false;
            let index = item.record_source;
            let source = index.and_then(|index| proposed.evidence.get(usize::from(index)));
            if let Some((image, withheld)) =
                source.and_then(|source| list_record_media(source, executions, profile, work))
            {
                item.media_attempted = withheld;
                item.record_source = index;
                item.record_media = image;
            }
        }
    }
    if let WorkArtifactDataV1::Picks { items, .. } = &mut proposed.data {
        for item in items {
            item.record_media = None;
            item.media_attempted = false;
            let source = item
                .source
                .and_then(|index| proposed.evidence.get(usize::from(index)));
            if let Some((image, withheld)) = source.and_then(lookup) {
                item.media_attempted = withheld;
                item.record_media = image;
                if withheld {
                    item.image_candidates.clear();
                }
            }
        }
    }
}

/// Only an authenticated immutable collection row may supply a List picture.
/// Every matching row participates, including failed enrichment, so a shared
/// source cannot silently borrow the one row that happened to have an image.
fn list_record_media(
    wanted: &WorkEvidenceLink,
    executions: &[WorkExecutionFact],
    profile: zephium_core::ids::ProfileId,
    work: WorkId,
) -> Option<(
    Option<zephium_core::work::media::WorkRecordMediaRefV1>,
    bool,
)> {
    let mut matched = false;
    let mut refused = false;
    let mut withheld = false;
    let mut selected = None;
    for execution in executions {
        for artifact in &execution.artifacts {
            let WorkArtifactDataV1::ComparisonMatrix {
                subjects, cells, ..
            } = &artifact.data
            else {
                continue;
            };
            for (index, subject) in subjects.iter().enumerate() {
                if !cells.get(index).is_some_and(|row| {
                    row.iter()
                        .flat_map(|cell| &cell.evidence)
                        .any(|index| artifact.evidence.get(usize::from(*index)) == Some(wanted))
                }) {
                    continue;
                }
                matched = true;
                withheld |= subject.media_attempted;
                let Some(media) = subject.record_media.as_ref().filter(|media| {
                    media.validate().is_ok()
                        && media.profile == profile
                        && media.work == work
                        && media.execution == execution.id
                        && media.execution == artifact.execution
                        && media.attempt == artifact.attempt
                        && media.extraction == wanted.extraction_id
                }) else {
                    refused = true;
                    withheld |= subject.record_media.is_some();
                    continue;
                };
                if selected.as_ref().is_some_and(|old| old != media) {
                    refused = true;
                    withheld = true;
                }
                selected = Some(media.clone());
            }
        }
    }
    matched.then_some((if refused { None } else { selected }, withheld))
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
    use serde_json::json;

    #[test]
    fn a_reply_written_as_markdown_is_set_not_refused() {
        let mut data = json!({
            "headline": "Svelte 5.40 adds attachments, async components and a new way to read state across files",
            "text": "## What changed\nSvelte 5.40 is out. See [the release notes](https://svelte.dev/blog).\n\n- Attachments replace actions\n- Async components\n1. Faster hydration",
        });
        normalize("reply", &mut data);
        let headline = data["headline"].as_str().unwrap();
        assert!(headline.chars().count() <= 80 && headline.ends_with('…'));
        assert_eq!(
            data["text"],
            "What changed\nSvelte 5.40 is out. See the release notes."
        );
        assert_eq!(
            data["points"],
            json!([
                "Attachments replace actions",
                "Async components",
                "Faster hydration"
            ])
        );
        let reply: WorkArtifactDataV1 = serde_json::from_value(
            json!({"kind":"reply","headline":headline,"text":data["text"],"points":data["points"]}),
        )
        .unwrap();
        assert!(reply.lead_fault(0).is_none());
    }

    #[test]
    fn a_long_reply_ends_at_a_sentence_and_the_rest_become_points() {
        let sentence =
            "The updater checks a signed manifest on every launch and installs in the background.";
        let text = [sentence; 8].join(" ");
        let mut data = json!({"headline":"Tauri updater","text":text});
        normalize("reply", &mut data);
        let text = data["text"].as_str().unwrap();
        assert!(text.chars().count() <= 480 && text.ends_with('.'));
        let points = data["points"].as_array().unwrap();
        assert!(!points.is_empty() && points.len() <= 5);
        assert!(points
            .iter()
            .all(|p| p.as_str().unwrap().chars().count() <= 140));
    }

    #[test]
    fn native_media_never_enters_model_input_or_survives_model_object_fields() {
        let mut value = serde_json::json!({
            "kind":"picks", "items":[{"name":"One", "source":0,
                "record_media":{"origin":"native_crop","digest":"private-reference"},
                "media_attempted":true,
                "nested":[{"record_media":{"origin":"native_fetch"},"keep":"source text"}]
            }]
        });
        strip_native_media(&mut value);
        assert_eq!(
            value,
            serde_json::json!({
                "kind":"picks", "items":[{"name":"One","source":0,"nested":[{"keep":"source text"}]}]
            })
        );
        assert!(!value.to_string().contains("private-reference"));
    }

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
    fn a_reply_without_its_own_sources_rests_on_this_runs_sourced_objects() {
        let object = |id: &str, evidence: serde_json::Value, in_this_run: bool| CanvasObject {
            artifact: serde_json::from_value(serde_json::json!({
                "version": 1, "id": id, "execution": "01M3RV9DBTWYS7770BES8YYKT1",
                "node": "01M3RV9DBSG3ZRK3M2H2P2HZ0R", "attempt": "01M3RV9DBTWYS7770BES8YYKT2",
                "output": "result", "title": "Sets", "evidence": evidence,
                "review": "mechanical", "presentation": "automatic",
                "data": {"kind": "picks", "facet": "product", "items": [{"name": "Paris"}]}
            }))
            .unwrap(),
            current: true,
            in_this_run,
            part_title: None,
        };
        let link = |source: u16| serde_json::json!({"extraction_id": "01M3RV9DBTWYS7770BES8YYKT9", "source_id": source});
        let general = object("01M3RV9DBTWYS7770BES8YYKT3", serde_json::json!([]), true);
        assert!(run_evidence(std::slice::from_ref(&general)).is_empty());
        let earlier = object(
            "01M3RV9DBTWYS7770BES8YYKT4",
            serde_json::json!([link(7)]),
            false,
        );
        let picks = object(
            "01M3RV9DBTWYS7770BES8YYKT5",
            serde_json::json!([link(1), link(2)]),
            true,
        );
        let sheet = object(
            "01M3RV9DBTWYS7770BES8YYKT6",
            serde_json::json!([link(2), link(3)]),
            true,
        );
        let links = run_evidence(&[earlier, general, picks, sheet]);
        assert_eq!(
            links.iter().map(|l| l.source_id).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        let many: Vec<_> = (0..MAX_ARTIFACT_EVIDENCE as u16 + 8)
            .map(|i| {
                object(
                    "01M3RV9DBTWYS7770BES8YYKT7",
                    serde_json::json!([link(i)]),
                    true,
                )
            })
            .collect();
        assert_eq!(run_evidence(&many).len(), MAX_ARTIFACT_EVIDENCE);
    }

    #[test]
    fn a_plan_without_its_own_sources_rests_on_the_picks_it_points_to() {
        let link = |source: u16| serde_json::json!({"extraction_id": "01M3RV9DBTWYS7770BES8YYKT9", "source_id": source});
        let picks = |id: &str, evidence: serde_json::Value| CanvasObject {
            artifact: serde_json::from_value(serde_json::json!({
                "version": 1, "id": id, "execution": "01M3RV9DBTWYS7770BES8YYKT1",
                "node": "01M3RV9DBSG3ZRK3M2H2P2HZ0R", "attempt": "01M3RV9DBTWYS7770BES8YYKT2",
                "output": "result", "title": "Flights", "evidence": evidence,
                "review": "mechanical", "presentation": "automatic",
                "data": {"kind": "picks", "facet": "flight", "items": [{"name": "LOT"}]}
            }))
            .unwrap(),
            current: true,
            in_this_run: true,
            part_title: None,
        };
        let flights = picks(
            "01M3RV9DBTWYS7770BES8YYKT3",
            serde_json::json!([link(1), link(2)]),
        );
        let other = picks("01M3RV9DBTWYS7770BES8YYKT4", serde_json::json!([link(5)]));
        let plan = |pick: Option<&str>| -> WorkArtifactDataV1 {
            serde_json::from_value(serde_json::json!({"kind": "plan", "steps": [
                {"kind": "travel", "title": "Fly to San Francisco", "pick": pick.map(|id| serde_json::json!({"artifact": id, "index": 0}))},
                {"kind": "task", "title": "Apply for ESTA"}
            ]}))
            .unwrap()
        };
        let objects = [flights, other];
        let links = pick_evidence(&objects, &plan(Some("01M3RV9DBTWYS7770BES8YYKT3")));
        assert_eq!(
            links.iter().map(|l| l.source_id).collect::<Vec<_>>(),
            [1, 2]
        );
        // A plan that points to no pick stays general knowledge.
        assert!(pick_evidence(&objects, &plan(None)).is_empty());
    }

    #[test]
    fn a_revision_without_sources_rests_on_what_its_object_rests_on_now() {
        let link = |source: u16| serde_json::json!({"extraction_id": "01M3RV9DBTWYS7770BES8YYKT9", "source_id": source});
        let object =
            |id: &str, evidence: serde_json::Value, data: serde_json::Value| CanvasObject {
                artifact: serde_json::from_value(serde_json::json!({
                    "version": 1, "id": id, "execution": "01M3RV9DBTWYS7770BES8YYKT1",
                    "node": "01M3RV9DBSG3ZRK3M2H2P2HZ0R", "attempt": "01M3RV9DBTWYS7770BES8YYKT2",
                    "output": "result", "title": "Object", "evidence": evidence,
                    "review": "mechanical", "presentation": "automatic", "data": data
                }))
                .unwrap(),
                current: true,
                in_this_run: true,
                part_title: None,
            };
        let picks =
            serde_json::json!({"kind": "picks", "facet": "flight", "items": [{"name": "LOT"}]});
        let plan = serde_json::json!({"kind": "plan", "steps": [
            {"kind": "travel", "title": "Fly", "pick": {"artifact": "01M3RV9DBTWYS7770BES8YYKT3", "index": 0}}
        ]});
        let flights = object(
            "01M3RV9DBTWYS7770BES8YYKT3",
            serde_json::json!([link(1)]),
            picks,
        );
        let reply = object(
            "01M3RV9DBTWYS7770BES8YYKT5",
            serde_json::json!([]),
            serde_json::json!({"kind": "reply", "headline": "Flights", "text": "Two flights fit."}),
        );
        let derived = object(
            "01M3RV9DBTWYS7770BES8YYKT6",
            serde_json::json!([link(1)]),
            plan.clone(),
        );
        let own = object(
            "01M3RV9DBTWYS7770BES8YYKT7",
            serde_json::json!([link(1), link(4)]),
            plan,
        );
        let objects = [flights, reply.clone(), derived.clone(), own.clone()];
        let ids =
            |links: Vec<WorkEvidenceLink>| links.iter().map(|l| l.source_id).collect::<Vec<_>>();

        // A reply first placed without pages takes the ones placed since.
        assert_eq!(ids(revised_evidence(&objects, &reply)), [1, 4]);
        // A plan's pages from its picks come again from the picks it names now.
        assert!(revised_evidence(&objects, &derived).is_empty());
        // Pages a plan named itself stay.
        assert_eq!(ids(revised_evidence(&objects, &own)), [1, 4]);
    }

    #[test]
    fn the_leads_offers_side_by_side_are_no_copy_of_a_parts_picks() {
        let picks = |id: &str, part: &str, names: &[&str]| {
            CanvasObject {
            artifact: serde_json::from_value(serde_json::json!({
                "version": 1, "id": id, "execution": "01M3RV9DBTWYS7770BES8YYKT1",
                "node": "01M3RV9DBSG3ZRK3M2H2P2HZ0R", "attempt": "01M3RV9DBTWYS7770BES8YYKT2",
                "output": "result", "title": "Offers on Amazon.de", "evidence": [],
                "review": "mechanical", "presentation": "automatic", "part": part,
                "data": {"kind": "picks", "facet": "product",
                    "items": names.iter().map(|n| serde_json::json!({"name": n})).collect::<Vec<_>>()}
            }))
            .unwrap(),
            current: true,
            in_this_run: true,
            part_title: None,
        }
        };
        let amazon = [picks(
            "01M3RV9DBTWYS7770BES8YYKT3",
            "01M3RV9DBTWYS7770BES8YYKT4",
            &["LEGO Technic 42115", "LEGO Technic 42115 green"],
        )];
        let names = |list: &[&str]| list.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
        assert!(part_holds(&amazon, &names(&["LEGO Technic 42115"])).is_some());
        assert!(part_holds(
            &amazon,
            &names(&[
                "LEGO Technic 42115",
                "LEGO Technic 42115",
                "LEGO Technic 42115"
            ])
        )
        .is_none());
        // The lead's own picks of the same stays, picked again, are updated.
        let mine = [{
            let mut object = picks(
                "01M3RV9DBTWYS7770BES8YYKT5",
                "01M3RV9DBTWYS7770BES8YYKT4",
                &["Room in Dogpatch", "Room in Potrero Hill", "Guesthouse"],
            );
            object.artifact.part = None;
            object
        }];
        let again = names(&["Room in Dogpatch", "Room in Potrero Hill", "Condo in SoMa"]);
        assert!(lead_holds(&mine, &again).is_some());
        assert!(lead_holds(&mine, &names(&["Condo in SoMa", "Loft"])).is_none());
        assert!(lead_holds(&amazon, &names(&["LEGO Technic 42115"])).is_none());
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
            unread: false,
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
                failed: vec![],
                unread: false,
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
            "No Best Buy listings could be verified",
            "Nothing on Allegro is confirmed yet",
        ] {
            let plain: WorkArtifactDataV1 = serde_json::from_value(serde_json::json!({
                "kind": "reply", "headline": headline,
                "text": "I couldn't read Slack: its page kept changing under me. Try again or use the Slack connection."
            }))
            .unwrap();
            assert!(honest(&plain, &honest_run).is_ok(), "{headline}");
        }
        for headline in [
            "Pricing could not be checked",
            "Pricing could not be checked or verified",
            "Pricing could not be checked or independently verified",
            "Nothing was read, so pricing is not yet fully confirmed",
        ] {
            assert!(!claims_done(headline), "{headline}");
        }
        for headline in [
            "Slack was not read, but the plan is complete",
            "No issues found and pricing checked",
            "No issues found and pricing fully checked",
            "No issues found and pricing reviewed and independently verified",
        ] {
            assert!(claims_done(headline), "{headline}");
            let claimed: WorkArtifactDataV1 = serde_json::from_value(serde_json::json!({
                "kind": "reply", "headline": headline, "text": "The plan stands."
            }))
            .unwrap();
            assert!(honest(&claimed, &honest_run).is_err(), "{headline}");
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
                failed: vec![],
                unread: false,
            }
        )
        .is_err());
        let tagged: WorkArtifactDataV1 = serde_json::from_value(serde_json::json!({
            "kind": "picks", "facet": "service", "items": [{"name": "Hetzner", "tags": ["Top pick"]}]
        }))
        .unwrap();
        assert!(honest(&tagged, &honest_run).is_err());
    }

    #[test]
    fn no_table_when_no_page_could_be_read() {
        let unread = Honesty {
            sourced: true,
            failed: vec![],
            unread: true,
        };
        let sheet: WorkArtifactDataV1 = serde_json::from_value(serde_json::json!({
            "kind": "sheet", "columns": [{"label": "Provider", "kind": "text"}, {"label": "Price", "kind": "text"}],
            "rows": [{"cells": ["Hetzner", "CX23 from $6.49/mo"]}, {"cells": ["Vercel", ""]}]
        }))
        .unwrap();
        assert!(honest(&sheet, &unread).unwrap_err().contains("no page"));
        assert!(mend(&sheet, &unread).is_none());
        let picks: WorkArtifactDataV1 = serde_json::from_value(serde_json::json!({
            "kind": "picks", "facet": "service", "items": [{"name": "Hetzner"}]
        }))
        .unwrap();
        assert!(honest(&picks, &unread).is_err());
        let claimed: WorkArtifactDataV1 = serde_json::from_value(serde_json::json!({
            "kind": "reply", "headline": "Comparison complete", "text": "The table is on the canvas."
        }))
        .unwrap();
        assert!(honest(&claimed, &unread).is_err());
        for headline in ["Pricing checked", "Pricing finished"] {
            let claimed: WorkArtifactDataV1 = serde_json::from_value(serde_json::json!({
                "kind": "reply", "headline": headline, "text": "The table is on the canvas."
            }))
            .unwrap();
            assert!(honest(&claimed, &unread).is_err(), "{headline}");
        }
        let negated: WorkArtifactDataV1 = serde_json::from_value(serde_json::json!({
            "kind": "reply", "headline": "Pricing could not be checked", "text": "The pages didn't open."
        }))
        .unwrap();
        assert!(honest(&negated, &unread).is_ok());
        let plain: WorkArtifactDataV1 = serde_json::from_value(serde_json::json!({
            "kind": "reply", "headline": "I couldn't read the pricing pages",
            "text": "Vercel, Cloudflare and Hetzner's pages didn't open. Try again."
        }))
        .unwrap();
        assert!(honest(&plain, &unread).is_ok());
        let read = Honesty {
            unread: false,
            ..unread
        };
        assert!(honest(&sheet, &read).is_ok());
    }
}

#[cfg(test)]
mod list_record_media_tests {
    use super::*;
    use serde_json::json;
    use zephium_core::{ids::ProfileId, work::media::*};

    fn fixture() -> (
        ProfileId,
        WorkId,
        WorkExecutionFact,
        WorkEvidenceLink,
        WorkRecordMediaRefV1,
    ) {
        let profile = ProfileId::generate();
        let work = WorkId::generate();
        let execution = WorkExecutionId::generate();
        let attempt = WorkAttemptId::generate();
        let source = WorkEvidenceLink {
            extraction_id: WorkArtifactId::generate(),
            source_id: 1,
        };
        let media = WorkRecordMediaRefV1 {
            profile,
            work,
            execution,
            attempt,
            step: WorkStepId::generate(),
            extraction: source.extraction_id,
            record: 0,
            source: "a".repeat(64),
            digest: "b".repeat(64),
            width: 128,
            height: 96,
            origin: WorkRecordMediaOriginV1::NativeCrop,
        };
        let limits = json!({"model_tokens":1000,"cost_micro_usd":1000,"operations":8,"timeout_seconds":60,"max_workers":1});
        let mut fact: WorkExecutionFact = serde_json::from_value(json!({
            "authorization":"user_directed_agent","id":execution,"approved_revision":"3",
            "spec":{"plan_revision":"2","limits":limits,"nodes":[]},
            "status":"needs_review","attempts":[],"artifacts":[]
        }))
        .unwrap();
        fact.artifacts.push(serde_json::from_value(json!({
            "version":1,"id":WorkArtifactId::generate(),"execution":execution,"node":WorkPlanNodeId::generate(),"attempt":attempt,
            "output":"result","title":"Native rows","review":"source_mapped_needs_review","presentation":"automatic","general_knowledge":false,
            "evidence":[source],"data":{"kind":"comparison_matrix","subjects":[{"name":"Native row","media_attempted":true,"record_media":media}],"criteria":[{"name":"Title","kind":{"kind":"text"}}],"cells":[[{"value":{"kind":"text","text":"Native row"},"evidence":[0]}]]}
        })).unwrap());
        fact.artifacts[0]
            .validate()
            .expect("valid synthetic source artifact");
        (profile, work, fact, source, media)
    }
    fn proposed(source: WorkEvidenceLink, record: Option<u16>) -> Proposed {
        Proposed{title:"Reading".into(),data:serde_json::from_value(json!({"kind":"list","style":"reading","items":[{"title":"Kept source-backed title","detail":"Kept detail","source":1,"record_source":record}]})).unwrap(),evidence:vec![source,WorkEvidenceLink{extraction_id:WorkArtifactId::generate(),source_id:0}],left_out:vec![]}
    }
    fn item(proposed: &Proposed) -> &zephium_core::work::objects::WorkListItemV1 {
        let WorkArtifactDataV1::List { items, .. } = &proposed.data else {
            panic!("list")
        };
        &items[0]
    }
    #[test]
    fn list_exact_record_source_is_independent_of_prose_and_keeps_crop_local() {
        let (profile, work, fact, source, media) = fixture();
        let mut value = proposed(source, Some(0));
        hydrate_record_media(&mut value, &[fact], profile, work);
        assert_eq!(item(&value).record_media.as_ref(), Some(&media));
        assert_eq!(item(&value).source, Some(1));
        assert!(item(&value).media_attempted);
        assert!(value.data.lead_fault(2).is_none());
        let mut model = serde_json::to_value(&value.data).unwrap();
        strip_native_media(&mut model);
        assert!(!model.to_string().contains("native_crop"));
        assert!(!model.to_string().contains("record_media"));
        assert_eq!(model["items"][0]["record_source"], 0);
        let mut context = value.data.clone();
        context.clear_record_media();
        assert!(!serde_json::to_string(&context)
            .unwrap()
            .contains("record_media"));
    }
    #[test]
    fn public_native_picture_does_not_need_a_withholding_flag() {
        let (profile, work, mut fact, source, media) = fixture();
        let WorkArtifactDataV1::ComparisonMatrix { subjects, .. } = &mut fact.artifacts[0].data
        else {
            panic!()
        };
        subjects[0].media_attempted = false;
        let mut value = proposed(source, Some(0));
        hydrate_record_media(&mut value, &[fact], profile, work);
        assert_eq!(item(&value).record_media.as_ref(), Some(&media));
        assert!(!item(&value).media_attempted);
        assert!(value.data.lead_fault(2).is_none());
    }

    #[test]
    fn list_missing_or_stale_record_source_never_falls_back_to_text_source() {
        let (profile, work, fact, source, _) = fixture();
        for index in [None, Some(1), Some(99)] {
            let mut value = proposed(source.clone(), index);
            let WorkArtifactDataV1::List { items, .. } = &mut value.data else {
                panic!()
            };
            items[0].source = Some(0);
            hydrate_record_media(&mut value, std::slice::from_ref(&fact), profile, work);
            assert!(item(&value).record_media.is_none());
            assert_eq!(item(&value).title, "Kept source-backed title");
            assert_eq!(item(&value).detail.as_deref(), Some("Kept detail"));
        }
    }
    #[test]
    fn list_foreign_custody_and_cross_extraction_refuse_picture_only() {
        let (profile, work, fact, source, _) = fixture();
        for kind in 0..5 {
            let mut bad = fact.clone();
            let WorkArtifactDataV1::ComparisonMatrix { subjects, .. } = &mut bad.artifacts[0].data
            else {
                panic!()
            };
            let media = subjects[0].record_media.as_mut().unwrap();
            match kind {
                0 => media.profile = ProfileId::generate(),
                1 => media.work = WorkId::generate(),
                2 => media.execution = WorkExecutionId::generate(),
                3 => media.attempt = WorkAttemptId::generate(),
                _ => media.extraction = WorkArtifactId::generate(),
            }
            let mut value = proposed(source.clone(), Some(0));
            hydrate_record_media(&mut value, &[bad], profile, work);
            assert!(item(&value).record_media.is_none());
            assert!(item(&value).media_attempted);
            assert_eq!(item(&value).title, "Kept source-backed title");
        }
    }
    #[test]
    fn list_ambiguous_and_failed_native_rows_cannot_borrow_successful_pixels() {
        let (profile, work, fact, source, _) = fixture();
        for missing in [false, true] {
            let mut duplicate = fact.artifacts[0].clone();
            duplicate.id = WorkArtifactId::generate();
            let WorkArtifactDataV1::ComparisonMatrix { subjects, .. } = &mut duplicate.data else {
                panic!()
            };
            if missing {
                subjects[0].record_media = None;
                subjects[0].media_attempted = false;
            } else {
                subjects[0].record_media.as_mut().unwrap().record = 1;
            }
            let mut rows = fact.clone();
            rows.artifacts.push(duplicate);
            let mut value = proposed(source.clone(), Some(0));
            hydrate_record_media(&mut value, &[rows], profile, work);
            assert!(item(&value).record_media.is_none());
            assert_eq!(item(&value).title, "Kept source-backed title");
        }
    }
    #[test]
    fn list_identical_immutable_row_copies_remain_one_identity() {
        let (profile, work, mut fact, source, media) = fixture();
        let mut duplicate = fact.artifacts[0].clone();
        duplicate.id = WorkArtifactId::generate();
        fact.artifacts.push(duplicate);
        let mut value = proposed(source, Some(0));
        hydrate_record_media(&mut value, &[fact], profile, work);
        assert_eq!(item(&value).record_media.as_ref(), Some(&media));
    }
    #[test]
    fn list_additive_defaults_and_record_custody_validation_remain_closed() {
        let mut plain: WorkArtifactDataV1 = serde_json::from_value(
            json!({"kind":"list","style":"reading","items":[{"title":"Old object"}]}),
        )
        .unwrap();
        assert!(!serde_json::to_string(&plain)
            .unwrap()
            .contains("record_source"));
        let (_, _, _, _, media) = fixture();
        let WorkArtifactDataV1::List { items, .. } = &mut plain else {
            panic!()
        };
        items[0].record_media = Some(media);
        items[0].media_attempted = true;
        assert_eq!(
            plain.lead_fault(1).unwrap().field,
            WorkArtifactField::ListItemMedia
        );
        let WorkArtifactDataV1::List { items, .. } = &mut plain else {
            panic!()
        };
        items[0].record_source = Some(1);
        assert_eq!(
            plain.lead_fault(1).unwrap().field,
            WorkArtifactField::ItemSource
        );
    }
}
