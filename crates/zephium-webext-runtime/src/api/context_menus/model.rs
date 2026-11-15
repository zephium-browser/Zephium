use super::Request;
use crate::ApiError;
use serde_json::{json, Value};
use zephium_core::injection::MatchPattern;

const MAX_ITEMS: usize = 1024;
const MAX_ITEM_BYTES: usize = 16 * 1024;

/// Native hit-test facts; page code cannot supply this record through an API call.
#[derive(Clone, Debug, Default)]
pub struct MenuContext {
    /// Browser-owned document identity captured when the native menu opens.
    pub document_id: Option<String>,
    /// Menu registry revision captured by the native presentation owner.
    pub menu_generation: u64,
    /// Committed document address.
    pub page_url: String,
    /// Native link target.
    pub link_url: Option<String>,
    /// Native image or media target.
    pub src_url: Option<String>,
    /// image, video or audio, when the native hit test establishes it.
    pub media_type: Option<String>,
    /// Editable hit-test target.
    pub editable: bool,
    /// The native hit test establishes a selection (text may be unavailable).
    pub selection: bool,
}

/// Validated menu entry retained independently of a background execution.
#[derive(Clone, Debug)]
pub struct MenuItem {
    /// Extension-local identifier.
    pub id: String,
    /// Original normalized properties.
    pub properties: Value,
}

/// One install's bounded menu model. Background suspension does not clear it.
#[derive(Clone, Debug, Default)]
pub struct MenuStore {
    items: Vec<MenuItem>,
}

fn missing(id: &str) -> ApiError {
    ApiError::other(format!("Cannot find menu item with id {id}"))
}

fn matches(patterns: Option<&Value>, target: Option<&str>) -> bool {
    patterns.is_none_or(|patterns| {
        let Some(target) = target.and_then(|target| url::Url::parse(target).ok()) else {
            return false;
        };
        patterns.as_array().is_some_and(|patterns| {
            patterns.iter().any(|pattern| {
                pattern
                    .as_str()
                    .and_then(|pattern| MatchPattern::parse(pattern).ok())
                    .is_some_and(|pattern| pattern.matches_url(&target))
            })
        })
    })
}

impl MenuStore {
    /// Applies an already schema-validated operation atomically.
    pub fn apply(&mut self, request: Request) -> Result<Value, ApiError> {
        let preferred = match &request {
            Request::Create(properties) if properties["checked"] == true => {
                properties["id"].as_str().map(str::to_owned)
            }
            Request::Update(id, properties) if properties["checked"] == true => {
                id.as_str().map(str::to_owned)
            }
            _ => None,
        };
        let result = match request {
            Request::RemoveAll => {
                self.items.clear();
                Ok(Value::Null)
            }
            Request::Remove(id) => {
                let id = id.as_str().ok_or_else(crate::browser::unsupported)?;
                if !self.items.iter().any(|item| item.id == id) {
                    return Err(missing(id));
                }
                let mut removed = vec![id.to_owned()];
                loop {
                    let previous = removed.len();
                    for item in &self.items {
                        if item.properties["parentId"]
                            .as_str()
                            .is_some_and(|parent| removed.iter().any(|id| id == parent))
                            && !removed.contains(&item.id)
                        {
                            removed.push(item.id.clone());
                        }
                    }
                    if previous == removed.len() {
                        break;
                    }
                }
                self.items.retain(|item| !removed.contains(&item.id));
                Ok(Value::Null)
            }
            Request::Create(properties) => {
                let id = properties["id"]
                    .as_str()
                    .ok_or_else(crate::browser::unsupported)?
                    .to_owned();
                if self.items.iter().any(|item| item.id == id) {
                    return Err(ApiError::other(format!(
                        "Cannot create item with duplicate id {id}"
                    )));
                }
                if self.items.len() >= MAX_ITEMS {
                    return Err(ApiError::too_many_pending());
                }
                self.validate(&id, &properties)?;
                self.items.push(MenuItem {
                    id: id.clone(),
                    properties,
                });
                Ok(Value::String(id))
            }
            Request::Update(id, update) => {
                let id = id.as_str().ok_or_else(crate::browser::unsupported)?;
                let index = self
                    .items
                    .iter()
                    .position(|item| item.id == id)
                    .ok_or_else(|| missing(id))?;
                let mut properties = self.items[index].properties.clone();
                let object = properties
                    .as_object_mut()
                    .ok_or_else(ApiError::malformed_arguments)?;
                object.extend(
                    update
                        .as_object()
                        .ok_or_else(ApiError::malformed_arguments)?
                        .clone(),
                );
                self.validate(id, &properties)?;
                self.items[index].properties = properties;
                Ok(Value::Null)
            }
        };
        if result.is_ok() {
            self.normalize_radios(preferred.as_deref());
        }
        result
    }

    fn normalize_radios(&mut self, preferred: Option<&str>) {
        let mut parents: Vec<Value> = self
            .items
            .iter()
            .map(|item| item.properties["parentId"].clone())
            .collect();
        parents.sort_by_key(Value::to_string);
        parents.dedup();
        for parent in parents {
            let siblings: Vec<_> = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.properties["parentId"] == parent)
                .map(|(index, _)| index)
                .collect();
            for run in siblings
                .split(|index| self.items[*index].properties["type"] != "radio")
                .filter(|run| !run.is_empty())
                .map(<[usize]>::to_vec)
                .collect::<Vec<_>>()
            {
                let chosen = run
                    .iter()
                    .copied()
                    .find(|index| Some(self.items[*index].id.as_str()) == preferred)
                    .or_else(|| {
                        run.iter()
                            .copied()
                            .rev()
                            .find(|index| self.items[*index].properties["checked"] == true)
                    })
                    .unwrap_or(run[0]);
                for index in run {
                    self.items[index].properties["checked"] = (index == chosen).into();
                }
            }
        }
    }

    fn validate(&self, id: &str, properties: &Value) -> Result<(), ApiError> {
        if properties.to_string().len() > MAX_ITEM_BYTES {
            return Err(ApiError::too_many_pending());
        }
        let kind = properties["type"].as_str().unwrap_or("normal");
        if kind != "separator" && properties["title"].as_str().is_none_or(str::is_empty) {
            return Err(ApiError::other(
                "All menu items except for separators must have a title",
            ));
        }
        if properties.get("checked").is_some() && !matches!(kind, "checkbox" | "radio") {
            return Err(ApiError::other(
                "Only items with type \"radio\" or \"checkbox\" can be checked",
            ));
        }
        let mut parent = properties["parentId"].as_str();
        let mut depth = 0;
        while let Some(next) = parent {
            depth += 1;
            if depth >= 32 {
                return Err(ApiError::too_many_pending());
            }
            if next == id {
                return Err(ApiError::other("An item cannot be an ancestor of itself"));
            }
            let item = self
                .items
                .iter()
                .find(|item| item.id == next)
                .ok_or_else(|| missing(next))?;
            if !matches!(item.properties["type"].as_str(), None | Some("normal")) {
                return Err(ApiError::other("Parent items must have type \"normal\""));
            }
            parent = item.properties["parentId"].as_str();
        }
        for key in ["documentUrlPatterns", "targetUrlPatterns"] {
            if let Some(patterns) = properties[key].as_array() {
                for pattern in patterns {
                    MatchPattern::parse(
                        pattern.as_str().ok_or_else(ApiError::malformed_arguments)?,
                    )
                    .map_err(|_| ApiError::other("Invalid URL pattern"))?;
                }
            }
        }
        Ok(())
    }

    /// Visible items in registration order, filtered against native hit-test facts.
    pub fn visible(&self, context: &MenuContext) -> Vec<MenuItem> {
        self.items
            .iter()
            .filter(|item| {
                let mut current = Some(*item);
                while let Some(item) = current {
                    let p = &item.properties;
                    if p["visible"] == false
                        || !matches(p.get("documentUrlPatterns"), Some(&context.page_url))
                    {
                        return false;
                    }
                    let contexts = p["contexts"]
                        .as_array()
                        .cloned()
                        .unwrap_or_else(|| vec![json!("page")]);
                    if !contexts.iter().any(|kind| match kind.as_str() {
                        Some("all") => true,
                        Some("page") => {
                            !context.editable
                                && !context.selection
                                && context.link_url.is_none()
                                && context.media_type.is_none()
                        }
                        Some("link") => {
                            context.link_url.is_some()
                                && matches(p.get("targetUrlPatterns"), context.link_url.as_deref())
                        }
                        Some("editable") => context.editable,
                        Some("selection") => context.selection,
                        Some("image" | "audio" | "video") => {
                            kind.as_str() == context.media_type.as_deref()
                                && matches(p.get("targetUrlPatterns"), context.src_url.as_deref())
                        }
                        _ => false,
                    }) {
                        return false;
                    }
                    current = p["parentId"]
                        .as_str()
                        .and_then(|id| self.items.iter().find(|item| item.id == id));
                }
                true
            })
            .cloned()
            .collect()
    }

    /// Applies checkbox/radio state and builds onClicked data after native activation.
    pub fn click(&mut self, id: &str, context: &MenuContext) -> Result<Value, ApiError> {
        if !self
            .visible(context)
            .iter()
            .any(|item| item.id == id && item.properties["enabled"] != false)
        {
            return Err(missing(id));
        }
        let index = self
            .items
            .iter()
            .position(|item| item.id == id)
            .ok_or_else(|| missing(id))?;
        let previous = self.items[index].properties["checked"] == true;
        let kind = self.items[index].properties["type"]
            .as_str()
            .unwrap_or("normal")
            .to_owned();
        let parent = self.items[index].properties["parentId"].clone();
        if kind == "radio" {
            let siblings: Vec<_> = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.properties["parentId"] == parent)
                .map(|(index, _)| index)
                .collect();
            let position = siblings
                .iter()
                .position(|sibling| *sibling == index)
                .ok_or_else(|| missing(id))?;
            let mut group = vec![index];
            for &sibling in siblings[..position].iter().rev() {
                if self.items[sibling].properties["type"] != "radio" {
                    break;
                }
                group.push(sibling);
            }
            for &sibling in &siblings[position + 1..] {
                if self.items[sibling].properties["type"] != "radio" {
                    break;
                }
                group.push(sibling);
            }
            for sibling in group {
                self.items[sibling].properties["checked"] = false.into();
            }
        }
        let mut data =
            json!({"menuItemId":id,"pageUrl":context.page_url,"editable":context.editable});
        if !parent.is_null() {
            data["parentMenuItemId"] = parent;
        }
        if matches!(kind.as_str(), "checkbox" | "radio") {
            let checked = kind == "radio" || !previous;
            self.items[index].properties["checked"] = checked.into();
            data["wasChecked"] = previous.into();
            data["checked"] = checked.into();
        }
        for (key, value) in [
            ("linkUrl", &context.link_url),
            ("srcUrl", &context.src_url),
            ("mediaType", &context.media_type),
        ] {
            if let Some(value) = value {
                data[key] = value.clone().into();
            }
        }
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn radio_runs_and_parent_contexts_follow_the_retained_tree() {
        let mut store = MenuStore::default();
        for properties in [
            json!({"id":"one","title":"One","type":"radio"}),
            json!({"id":"two","title":"Two","type":"radio"}),
            json!({"id":"separator","type":"separator"}),
            json!({"id":"three","title":"Three","type":"radio"}),
        ] {
            store.apply(Request::Create(properties)).unwrap();
        }
        let context = MenuContext {
            page_url: "https://example.com/".into(),
            ..Default::default()
        };
        assert_eq!(
            store
                .visible(&context)
                .iter()
                .filter(|item| item.properties["checked"] == true)
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "three"]
        );
        store.click("two", &context).unwrap();
        store.apply(Request::Remove(json!("separator"))).unwrap();
        assert_eq!(
            store
                .visible(&context)
                .iter()
                .filter(|item| item.properties["checked"] == true)
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["three"]
        );
        store
            .apply(Request::Create(
                json!({"id":"parent","title":"Parent","contexts":["page"]}),
            ))
            .unwrap();
        store
            .apply(Request::Create(
                json!({"id":"child","title":"Child","parentId":"parent","contexts":["editable"]}),
            ))
            .unwrap();
        let editable = MenuContext {
            editable: true,
            ..context
        };
        assert!(!store
            .visible(&editable)
            .iter()
            .any(|item| item.id == "child"));
        assert!(store.click("child", &editable).is_err());
    }

    #[test]
    fn menu_mutations_filter_and_preserve_state_until_removed() {
        let mut store = MenuStore::default();
        store
            .apply(Request::Create(
                json!({"id":"parent","title":"Tools","contexts":["all"]}),
            ))
            .unwrap();
        store.apply(Request::Create(json!({"id":"child","parentId":"parent","title":"Fill","type":"checkbox","contexts":["editable"],"documentUrlPatterns":["https://example.com/*"]}))).unwrap();
        assert!(store
            .apply(Request::Update(
                json!("parent"),
                json!({"parentId":"child"})
            ))
            .is_err());
        assert!(store
            .apply(Request::Create(json!({"id":"parent","title":"Duplicate"})))
            .is_err());
        let context = MenuContext {
            page_url: "https://example.com/".into(),
            editable: true,
            ..Default::default()
        };
        assert_eq!(store.visible(&context).len(), 2);
        assert_eq!(store.click("child", &context).unwrap()["checked"], true);
        assert_eq!(
            store.clone().click("child", &context).unwrap()["wasChecked"],
            true
        );
        store
            .apply(Request::Update(json!("child"), json!({"enabled":false})))
            .unwrap();
        assert!(store.click("child", &context).is_err());
        store.apply(Request::Remove(json!("parent"))).unwrap();
        assert!(store.visible(&context).is_empty());
        assert!(store.apply(Request::Remove(json!("child"))).is_err());
    }
}
