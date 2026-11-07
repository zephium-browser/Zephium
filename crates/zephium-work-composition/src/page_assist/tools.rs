//! The assistant's tools: their fixed definitions and the closed shape of each call.
//! Arguments are the model's words; everything they name is admitted again
//! by the service against what was actually read.
use serde_json::{json, Value};
use zephium_core::work::model::WorkModelTool;
use zephium_page_assist::actions::{self, Step, MAX_STEPS};

use super::port::BringRequest;

/// Most items one list shows.
pub(crate) const MAX_ITEMS: usize = 12;
/// Most tabs one call opens.
pub(crate) const MAX_OPEN: usize = 8;
const MAX_TITLE_CHARS: usize = 140;
const MAX_META: usize = 4;
const MAX_META_CHARS: usize = 40;
const MAX_CSS_BYTES: usize = 24 * 1024;
const MAX_SUMMARY_CHARS: usize = 160;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ItemCall {
    pub(crate) reference: Option<String>,
    pub(crate) title: String,
    pub(crate) meta: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Call {
    ListTabs,
    ReadPage {
        tab: String,
    },
    ShowItems {
        items: Vec<ItemCall>,
    },
    OpenTabs {
        targets: Vec<String>,
    },
    GoTo {
        target: String,
        new_tab: bool,
    },
    Act {
        steps: Vec<Step>,
    },
    InspectStyle {
        page: String,
    },
    Restyle {
        page: String,
        summary: String,
        css: String,
    },
    HandOff {
        title: String,
        summary: String,
        bring: BringRequest,
    },
}

fn tool(name: &str, description: &str, schema: Value) -> WorkModelTool {
    WorkModelTool {
        name: name.to_owned(),
        description: description.to_owned(),
        schema,
        strict: false,
    }
}

/// The tools, in a fixed order so their definitions stay cached.
pub(crate) fn definitions() -> Vec<WorkModelTool> {
    vec![
        tool(
            "list_tabs",
            "Lists the open tabs of this Space with their handles, titles and hosts.",
            json!({"type": "object", "properties": {}, "additionalProperties": false}),
        ),
        tool(
            "read_page",
            "Reads one open tab by its handle from list_tabs and returns it as a lettered page.",
            json!({
                "type": "object",
                "properties": {"tab": {"type": "string", "description": "Handle such as t2."}},
                "required": ["tab"],
                "additionalProperties": false
            }),
        ),
        tool(
            "show_items",
            "Shows items from a page as numbered cards in the reply and marks each on the page with the same number. Use for any answer that is a set of things on a page.",
            json!({
                "type": "object",
                "properties": {
                    "items": {
                        "type": "array",
                        "maxItems": MAX_ITEMS,
                        "items": {
                            "type": "object",
                            "properties": {
                                "ref": {"type": "string", "description": "The item's main passage, such as a12: its title link when it has one."},
                                "title": {"type": "string", "description": "The item's name as the page gives it."},
                                "meta": {"type": "array", "maxItems": MAX_META, "items": {"type": "string"}, "description": "Short facts from the page, such as \"#2\", \"749 points\", \"541 comments\"."}
                            },
                            "required": ["ref", "title"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["items"],
                "additionalProperties": false
            }),
        ),
        tool(
            "open_tabs",
            "Opens links in new background tabs. Each target is a link passage reference such as a12, or an address you showed or the person wrote.",
            json!({
                "type": "object",
                "properties": {"targets": {"type": "array", "maxItems": MAX_OPEN, "items": {"type": "string"}}},
                "required": ["targets"],
                "additionalProperties": false
            }),
        ),
        tool(
            "go_to",
            "Moves the tab you are working in to one page, as following a link does, and reads it so you can keep working there. The target is a link passage reference such as a12, an address the person wrote or a tab listed by list_tabs, or `search: words` to propose a search on the person's own search engine. Every search needs the person's approval of its exact query and destination; a refusal must not be worked around. new_tab only when the person asks for a new tab or the current page must stay.",
            json!({
                "type": "object",
                "properties": {"target": {"type": "string"}, "new_tab": {"type": "boolean"}},
                "required": ["target"],
                "additionalProperties": false
            }),
        ),
        tool(
            "act",
            "Runs small steps on one page, in order. Verbs: click, fill, select, check, uncheck, scroll, expand, open, submit (press Return in a field, as a search box needs after fill). fill and select take a value. scroll takes a passage, or up or down.",
            json!({
                "type": "object",
                "properties": {
                    "steps": {
                        "type": "array",
                        "maxItems": MAX_STEPS,
                        "items": {
                            "type": "object",
                            "properties": {
                                "verb": {"type": "string", "enum": ["click", "fill", "select", "check", "uncheck", "scroll", "expand", "open", "submit"]},
                                "ref": {"type": "string", "description": "The control's passage, such as a7; up or down for a scroll."},
                                "value": {"type": "string"}
                            },
                            "required": ["verb"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["steps"],
                "additionalProperties": false
            }),
        ),
        tool(
            "inspect_style",
            "Returns a page's outline for restyling: its scheme, colours, fonts and regions with their selectors.",
            json!({
                "type": "object",
                "properties": {"page": {"type": "string", "description": "The page's letter, such as a."}},
                "required": ["page"],
                "additionalProperties": false
            }),
        ),
        tool(
            "restyle",
            "Proposes a stylesheet for the page's whole site. The person applies it with one click; nothing changes before that.",
            json!({
                "type": "object",
                "properties": {
                    "page": {"type": "string", "description": "The page's letter, such as a."},
                    "summary": {"type": "string", "description": "What changes, under 60 characters, such as \"Stories as cards\"."},
                    "css": {"type": "string"}
                },
                "required": ["page", "summary", "css"],
                "additionalProperties": false
            }),
        ),
        tool(
            "hand_off",
            "Proposes a job for Work, Zephium's agent workspace, with what it should bring along. Nothing runs until the person continues.",
            json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Short, such as \"Zephium status page\"."},
                    "summary": {"type": "string", "description": "One line on what Work will do."},
                    "tabs": {"type": "array", "items": {"type": "string"}, "description": "Tab handles to bring, from list_tabs or the attached page (t1)."},
                    "task": {"type": "string", "description": "Words naming a task of the person's to bring."},
                    "note": {"type": "string", "description": "Words naming a note of the person's to bring."},
                    "repository": {"type": "string", "description": "A GitHub repository as owner/name."},
                    "folder": {"type": "string", "description": "A local folder, such as ~/Dev/project."}
                },
                "required": ["title", "summary"],
                "additionalProperties": false
            }),
        ),
    ]
}

fn string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(zephium_page_assist::text::clean)
        .filter(|text| !text.is_empty())
}

fn bounded(value: &Value, key: &str, limit: usize) -> Option<String> {
    string(value, key).map(|text| zephium_page_assist::text::bounded(&text, limit))
}

fn strings(value: &Value, key: &str, max: usize, limit: usize) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(zephium_page_assist::text::clean)
                .filter(|text| !text.is_empty())
                .map(|text| zephium_page_assist::text::bounded(&text, limit))
                .take(max)
                .collect()
        })
        .unwrap_or_default()
}

/// The tool's own name for the log; anything else is "unknown".
pub(crate) fn known(name: &str) -> &'static str {
    [
        "list_tabs",
        "read_page",
        "show_items",
        "open_tabs",
        "go_to",
        "act",
        "inspect_style",
        "restyle",
        "hand_off",
    ]
    .into_iter()
    .find(|known| *known == name)
    .unwrap_or("unknown")
}

/// The call `name` with `arguments`; none when it is not one of the tools
/// or its arguments do not fit.
pub(crate) fn parse(name: &str, arguments: &Value) -> Option<Call> {
    Some(match name {
        "list_tabs" => Call::ListTabs,
        "read_page" => Call::ReadPage {
            tab: string(arguments, "tab")?,
        },
        "show_items" => {
            let items: Vec<ItemCall> = arguments
                .get("items")?
                .as_array()?
                .iter()
                .filter_map(|item| {
                    Some(ItemCall {
                        reference: string(item, "ref"),
                        title: bounded(item, "title", MAX_TITLE_CHARS)?,
                        meta: strings(item, "meta", MAX_META, MAX_META_CHARS),
                    })
                })
                .take(MAX_ITEMS)
                .collect();
            if items.is_empty() {
                return None;
            }
            Call::ShowItems { items }
        }
        "go_to" => Call::GoTo {
            target: {
                let target = arguments.get("target")?.as_str()?.trim();
                if target.is_empty()
                    || target.chars().count() > 2048
                    || target != zephium_page_assist::text::clean(target)
                {
                    return None;
                }
                target.to_owned()
            },
            new_tab: arguments
                .get("new_tab")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        "open_tabs" => {
            let targets = strings(arguments, "targets", MAX_OPEN, 2048);
            if targets.is_empty() {
                return None;
            }
            Call::OpenTabs { targets }
        }
        "act" => {
            let list = arguments.get("steps")?.as_array()?;
            if list.is_empty() || list.len() > MAX_STEPS {
                return None;
            }
            let steps: Option<Vec<Step>> = list
                .iter()
                .map(|step| {
                    actions::step(
                        step.get("verb")?.as_str()?,
                        step.get("ref").and_then(Value::as_str),
                        step.get("value").and_then(Value::as_str),
                    )
                })
                .collect();
            Call::Act { steps: steps? }
        }
        "inspect_style" => Call::InspectStyle {
            page: string(arguments, "page")?,
        },
        "restyle" => {
            let css = arguments.get("css")?.as_str()?;
            if css.len() > MAX_CSS_BYTES {
                return None;
            }
            Call::Restyle {
                page: string(arguments, "page")?,
                summary: bounded(arguments, "summary", MAX_SUMMARY_CHARS)?,
                css: css.to_owned(),
            }
        }
        "hand_off" => Call::HandOff {
            title: bounded(arguments, "title", 80)?,
            summary: bounded(arguments, "summary", MAX_SUMMARY_CHARS)?,
            bring: BringRequest {
                tabs: strings(arguments, "tabs", 8, 16),
                task: bounded(arguments, "task", 120),
                note: bounded(arguments, "note", 120),
                repository: bounded(arguments, "repository", 140),
                folder: bounded(arguments, "folder", 260),
            },
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_definition_parses_its_own_minimal_call() {
        let calls = [
            ("list_tabs", json!({})),
            ("read_page", json!({"tab": "t2"})),
            (
                "show_items",
                json!({"items": [{"ref": "a3", "title": "Story"}]}),
            ),
            ("open_tabs", json!({"targets": ["a3"]})),
            ("go_to", json!({"target": "search: rust language"})),
            ("act", json!({"steps": [{"verb": "click", "ref": "a3"}]})),
            ("inspect_style", json!({"page": "a"})),
            (
                "restyle",
                json!({"page": "a", "summary": "Cards", "css": "p{color:#111}"}),
            ),
            (
                "hand_off",
                json!({"title": "Status page", "summary": "Compare three monitors."}),
            ),
        ];
        let names: Vec<String> = definitions().into_iter().map(|tool| tool.name).collect();
        assert_eq!(names.len(), calls.len());
        for (name, arguments) in calls {
            assert!(names.iter().any(|n| n == name));
            assert!(parse(name, &arguments).is_some(), "{name}");
        }
    }

    #[test]
    fn calls_outside_their_shape_are_refused() {
        assert_eq!(parse("eval", &json!({})), None);
        assert_eq!(parse("show_items", &json!({"items": []})), None);
        assert_eq!(
            parse("show_items", &json!({"items": [{"ref": "a1"}]})),
            None
        );
        assert_eq!(
            parse(
                "act",
                &json!({"steps": [{"verb": "navigate", "ref": "a1"}]})
            ),
            None
        );
        let nine: Vec<Value> = (0..9)
            .map(|_| json!({"verb": "click", "ref": "a1"}))
            .collect();
        assert_eq!(parse("act", &json!({"steps": nine})), None);
        let css = "p{}".repeat(MAX_CSS_BYTES);
        assert_eq!(
            parse("restyle", &json!({"page": "a", "summary": "x", "css": css})),
            None
        );
    }

    #[test]
    fn model_text_is_cleaned_and_bounded() {
        let Some(Call::ShowItems { items }) = parse(
            "show_items",
            &json!({"items": [{"ref": "a1", "title": "  A\u{202E}\n story ", "meta": ["1", "2", "3", "4", "5"]}]}),
        ) else {
            panic!("items")
        };
        assert_eq!(items[0].title, "A story");
        assert_eq!(items[0].meta.len(), MAX_META);
    }
}
