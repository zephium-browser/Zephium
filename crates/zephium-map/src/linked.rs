use crate::doc::{Head, Marks, Node};
use serde_json::Value;
use std::collections::BTreeMap;

pub(crate) fn objects(head: &Head, mut visit: impl FnMut(&Value)) {
    for raw in &head.linked_data {
        if let Ok(root) = serde_json::from_str::<Value>(raw) {
            let mut stack = vec![&root];
            while let Some(value) = stack.pop() {
                match value {
                    Value::Object(obj) => {
                        visit(value);
                        stack.extend(obj.values().rev());
                    }
                    Value::Array(items) => stack.extend(items.iter().rev()),
                    _ => {}
                }
            }
        }
    }
}

pub(crate) fn typed(object: &Value, name: &str) -> bool {
    let is = |v: &Value| {
        v.as_str()
            .is_some_and(|s| s.rsplit('/').next() == Some(name))
    };
    let t = &object["@type"];
    is(t) || t.as_array().is_some_and(|a| a.iter().any(is))
}

pub(crate) fn mark_answers(nodes: &mut [Node], head: &Head) {
    let mut ids = BTreeMap::new();
    let mut texts = BTreeMap::new();
    objects(head, |obj| {
        if !typed(obj, "QAPage") {
            return;
        }
        let question = &obj["mainEntity"];
        for (field, mark) in [
            ("suggestedAnswer", Marks::SUGGESTED),
            ("acceptedAnswer", Marks::ACCEPTED),
        ] {
            let v = &question[field];
            for answer in v
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(std::slice::from_ref(v))
            {
                if let Some((_, id)) = answer
                    .get("@id")
                    .or_else(|| answer.get("url"))
                    .and_then(Value::as_str)
                    .and_then(|s| s.rsplit_once('#'))
                {
                    ids.insert(id.to_owned(), mark);
                }
                if let Some(text) = answer["text"].as_str() {
                    let key = crate::clean_text(&crate::flatten(text));
                    if !key.is_empty() {
                        texts.insert(key, mark);
                    }
                }
            }
        }
    });
    if ids.is_empty() && texts.is_empty() {
        return;
    }
    for n in nodes {
        if let Some(&mark) = n
            .selector_id()
            .and_then(|id| ids.get(id))
            .or_else(|| texts.get(&n.text))
        {
            n.marks.insert(mark);
        }
    }
}
