use std::{collections::BTreeMap, time::Duration};

use zephium_map_driver::{
    wire::{Change, Entry, Failure, Outcome, Place, Row, StartRefusal, Tier},
    Counts,
};

const COLUMNS: usize = 100;
const PLACE_INTERVAL: Duration = Duration::from_millis(500);
const TABLE_DIVIDER: &str = " | ";
const TABLE_CELL: usize = (COLUMNS - TABLE_DIVIDER.len()) / 2;
const ELLIPSIS: char = '…';

#[derive(Default)]
struct Slot {
    printed: Option<Duration>,
    labels: BTreeMap<String, String>,
    sections: Vec<(String, String)>,
}

#[derive(Default)]
pub(super) struct Printer {
    slots: [Slot; super::MAX_SITES],
}

impl Printer {
    pub(super) fn section(&self, label: &str) -> Result<(u8, &str), &'static str> {
        let mut matches = self.slots.iter().enumerate().flat_map(|(index, slot)| {
            slot.sections.iter().filter_map(move |(id, name)| {
                name.eq_ignore_ascii_case(label.trim())
                    .then_some((index as u8, id.as_str()))
            })
        });
        let section = matches.next().ok_or("unknown label")?;
        if matches.next().is_some() {
            return Err("ambiguous label");
        }
        Ok(section)
    }

    pub(super) fn lines(
        &mut self,
        change: &Change,
        elapsed: Duration,
        counts: Counts,
    ) -> Vec<String> {
        match change {
            Change::FirstLayer {
                official,
                social,
                organic,
                also,
                ..
            } => {
                let mut line = format!("First layer  {organic} organic");
                if let Some(official) = official {
                    line.push_str(&format!(
                        "  official {} ({})",
                        official.title, official.site
                    ));
                }
                if !social.is_empty() {
                    let networks = social
                        .iter()
                        .map(|network| format!("{network:?}"))
                        .collect::<Vec<_>>();
                    line.push_str(&format!("  social {}", networks.join(", ")));
                }
                let mut lines = vec![clip(&line, COLUMNS)];
                lines.extend(also.iter().map(|result| {
                    clip(
                        &format!("  {:?}  {} ({})", result.kind, result.title, result.host),
                        COLUMNS,
                    )
                }));
                lines
            }
            Change::Place {
                slot,
                place,
                complete,
                ..
            } => {
                let Some(state) = self.slots.get_mut(usize::from(*slot)) else {
                    return Vec::new();
                };
                state.labels = entries(place)
                    .map(|entry| (entry.id.clone(), entry.label.clone()))
                    .collect();
                state.sections = place
                    .sections
                    .iter()
                    .filter(|e| !e.menu_only)
                    .map(|entry| (entry.id.clone(), entry.label.clone()))
                    .collect();
                if *complete
                    || state
                        .printed
                        .is_none_or(|last| elapsed.saturating_sub(last) > PLACE_INTERVAL)
                {
                    state.printed = Some(elapsed);
                    place_lines(*slot, place)
                } else {
                    Vec::new()
                }
            }
            Change::PlaceFailed { slot, failure, .. } => {
                if let Some(state) = self.slots.get_mut(usize::from(*slot)) {
                    state.labels.clear();
                    state.sections.clear();
                    state.printed = Some(elapsed);
                }
                vec![format!(
                    "Place {} failed  {}",
                    u16::from(*slot) + 1,
                    failure_text(*failure)
                )]
            }
            Change::Compare { rows, .. } => {
                let mut lines = vec![table_line("Left", "Right")];
                lines.extend(rows.iter().map(|row| self.row(row)));
                lines
            }
            Change::End {
                outcome,
                elapsed_ms,
                ..
            } => vec![end_line(*outcome, *elapsed_ms, counts)],
            Change::Opened { .. } => Vec::new(),
        }
    }

    fn row(&self, row: &Row) -> String {
        let label = |slot: &Slot, id: &Option<String>| {
            id.as_ref()
                .and_then(|id| slot.labels.get(id))
                .map_or_else(|| "-".into(), |label| clip(label, TABLE_CELL))
        };
        table_line(
            &label(&self.slots[0], &row.left),
            &label(&self.slots[1], &row.right),
        )
    }
}

pub(super) fn intro(query: &str) -> String {
    clip(&format!("Stand-in results page for {query}"), COLUMNS)
}

pub(super) fn refusal(reason: StartRefusal) -> &'static str {
    match reason {
        StartRefusal::NotAMap => {
            "Map refused  This query does not describe a place or a comparison."
        }
        StartRefusal::UnsupportedEngine => "Map refused  The search engine is unsupported.",
        StartRefusal::Invalid => "Map refused  The query is invalid or exceeds a limit.",
    }
}

pub(super) fn failure_text(failure: Failure) -> &'static str {
    match failure {
        Failure::Cancelled => "cancelled",
        Failure::Blocked => "access blocked",
        Failure::Challenge => "site requires a challenge",
        Failure::Consent => "site requires consent",
        Failure::NoResults => "no usable results",
        Failure::Unreadable => "page unreadable",
        Failure::Unavailable => "page unavailable",
        Failure::Timeout => "read timed out",
        Failure::Limit => "read limit reached",
        Failure::Missing => "page or result missing",
    }
}

fn end_line(outcome: Outcome, elapsed_ms: u32, counts: Counts) -> String {
    let outcome = match outcome {
        Outcome::Done => "Done".to_owned(),
        Outcome::Cancelled => "Cancelled".to_owned(),
        Outcome::Failed { failure } => format!("Failed ({})", failure_text(failure)),
    };
    let suffix = format!(
        "  {elapsed_ms}ms  reads {}  data {:.1} MB  renders wanted {}",
        counts.fetches,
        counts.bytes as f64 / 1_000_000.0,
        counts.render_unavailable
    );
    format!("{}{suffix}", clip(&outcome, COLUMNS - suffix.len()))
}

fn table_line(left: &str, right: &str) -> String {
    let pad = " ".repeat(TABLE_CELL.saturating_sub(columns(left)));
    format!("{left}{pad}{TABLE_DIVIDER}{right}")
}

fn entries(place: &Place) -> impl Iterator<Item = &Entry> {
    place
        .sections
        .iter()
        .flat_map(|section| std::iter::once(section).chain(&section.children))
}

fn place_lines(slot: u8, place: &Place) -> Vec<String> {
    let heading = format!("Place {}  ", u16::from(slot) + 1);
    let (title, host) = pair(
        &place.title,
        &place.host,
        COLUMNS - heading.len() - " ()".len(),
    );
    let mut lines = vec![format!("{heading}{title} ({host})")];
    for tier in [Tier::Primary, Tier::Secondary, Tier::Action] {
        let mut sections = place
            .sections
            .iter()
            .filter(|entry| entry.tier == tier)
            .peekable();
        if tier != Tier::Primary && sections.peek().is_some() {
            lines.push(format!("  --- {tier:?} ---"));
        }
        for section in sections {
            lines.push(entry_line(section, place.focus.as_deref(), "  "));
            for child in &section.children {
                lines.push(entry_line(child, place.focus.as_deref(), "    "));
            }
        }
    }
    lines
}

fn entry_line(entry: &Entry, focus: Option<&str>, indent: &str) -> String {
    let mark = if Some(entry.id.as_str()) == focus {
        "* "
    } else {
        ""
    };
    if entry.menu_only {
        return format!(
            "{indent}{mark}{} ▸",
            clip(&entry.label, COLUMNS - indent.len() - mark.len() - 2)
        );
    }
    let off_site = if entry.off_site { " [off-site]" } else { "" };
    let more = if entry.more { " [more]" } else { "" };
    let available = COLUMNS - indent.len() - mark.len() - off_site.len() - more.len() - "  ".len();
    let display = entry
        .host
        .as_ref()
        .map(|host| format!("{host}{}", entry.path));
    let (label, path) = pair(
        &entry.label,
        display.as_deref().unwrap_or(&entry.path),
        available,
    );
    format!("{indent}{mark}{label}  {path}{off_site}{more}")
}

fn pair(left: &str, right: &str, available: usize) -> (String, String) {
    let right_min = columns(right).min(available / 2);
    let left = clip(left, available - right_min);
    let right = clip(right, available - columns(&left));
    (left, right)
}

fn char_columns(ch: char) -> usize {
    if ch.is_ascii() {
        1
    } else {
        2
    }
}

fn columns(text: &str) -> usize {
    text.chars().map(char_columns).sum()
}

fn clip(text: &str, limit: usize) -> String {
    let text: String = zephium_map::clean_text(text)
        .chars()
        .map(|ch| {
            if ch.is_control() || ch.is_whitespace() {
                ' '
            } else {
                ch
            }
        })
        .collect();
    if columns(&text) <= limit {
        return text;
    }
    let Some(available) = limit.checked_sub(char_columns(ELLIPSIS)) else {
        return String::new();
    };
    let mut used = 0;
    let mut out: String = text
        .chars()
        .take_while(|ch| {
            used += char_columns(*ch);
            used <= available
        })
        .collect();
    out.push(ELLIPSIS);
    out
}

#[cfg(test)]
mod tests;
