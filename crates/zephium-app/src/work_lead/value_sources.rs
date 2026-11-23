//! Fact tables cite observed values, not a model-selected heading from the page.
use serde_json::Value;
use zephium_core::work::artifact::WorkEvidenceSourceV1;

use super::run::LeadSource;

/// A row whose values are not on the page it cites is refused; on the
/// lead's retry of the same object it stands without that citation instead,
/// its words its own, and is named in `uncited` (1-based rows).
pub(super) fn ground_sheet(
    data: &mut Value,
    declared: &[String],
    sources: &[LeadSource],
    lenient: bool,
    uncited: &mut Vec<usize>,
) -> Result<Vec<String>, String> {
    let mut used = Vec::new();
    let Some(rows) = data.get_mut("rows").and_then(Value::as_array_mut) else {
        return Ok(used);
    };
    for (index, row) in rows.iter_mut().enumerate() {
        let key = row.get("source").and_then(|value| {
            value.as_str().or_else(|| {
                value
                    .as_u64()
                    .and_then(|i| declared.get(i as usize))
                    .map(String::as_str)
            })
        });
        let Some(anchor) = key.and_then(|key| sources.iter().find(|s| s.key == key.trim())) else {
            continue;
        };
        if !anchor
            .preview
            .as_ref()
            .is_some_and(|p| matches!(p.source, WorkEvidenceSourceV1::NativeExtraction))
        {
            continue;
        }
        let Some(cells) = row.get("cells").and_then(Value::as_array) else {
            continue;
        };
        // The first column identifies the fact or record; the remaining cells
        // contain its values. A one-column table contains the value itself.
        let mut primary = None;
        let mut supported = Vec::new();
        let mut unsupported = false;
        for (column, value) in cells.iter().enumerate().skip(usize::from(cells.len() > 1)) {
            let Some(value) = value.as_str().filter(|s| !s.is_empty()) else {
                continue;
            };
            let supports = |source: &&LeadSource| {
                source.link.extraction_id == anchor.link.extraction_id
                    && source.preview.as_ref().is_some_and(|p| {
                        !p.truncated
                            && matches!(p.source, WorkEvidenceSourceV1::NativeExtraction)
                            && contains_value(&p.text, value)
                    })
            };
            let chosen = if supports(&anchor) {
                Some(anchor)
            } else {
                let mut matching = sources.iter().filter(supports);
                let first = matching.next();
                if matching.next().is_some() {
                    None
                } else {
                    first
                }
            };
            let Some(chosen) = chosen else {
                if lenient {
                    unsupported = true;
                    break;
                }
                return Err(format!("sheet: rows[{index}].cells[{column}] (row {}, cell {}) has no unambiguous observed value behind its citation; copy the exact page value with its source, use native record keys, or explain the uncertainty in the reply", index + 1, column + 1));
            };
            primary.get_or_insert_with(|| chosen.key.clone());
            if !supported.contains(&chosen.key) {
                supported.push(chosen.key.clone());
            }
        }
        if unsupported {
            if let Value::Object(row) = row {
                row.remove("source");
            }
            uncited.push(index + 1);
            continue;
        }
        for key in supported {
            if !used.contains(&key) {
                used.push(key);
            }
        }
        if let Some(primary) = primary {
            row["source"] = Value::String(primary);
        }
    }
    Ok(used)
}

fn contains_value(text: &str, value: &str) -> bool {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.is_empty() {
        return false;
    }
    let word = |c: char| c.is_alphanumeric() || matches!(c, '_' | '.' | ',');
    text.match_indices(&value).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + value.len()..].chars().next();
        !(value.chars().next().is_some_and(word) && before.is_some_and(word)
            || value.chars().next_back().is_some_and(word) && after.is_some_and(word))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use zephium_core::work::{artifact::*, WorkArtifactId};

    fn sources() -> Vec<LeadSource> {
        let extraction = WorkArtifactId::generate();
        [
            "Launch date",
            "25 December 2021 , 12:20 UTC",
            "Ariane 5 ECA+",
            "6.5 m (21 ft)",
        ]
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let link = WorkEvidenceLink {
                extraction_id: extraction,
                source_id: (i + 1) as u16,
            };
            LeadSource {
                key: format!("s{}", i + 1),
                link: link.clone(),
                url: None,
                preview: Some(WorkEvidencePreviewV1 {
                    version: 1,
                    link,
                    origin: "https://example.test/".into(),
                    role: if i == 0 { "cell_header" } else { "cell" }.into(),
                    text: text.to_string(),
                    truncated: false,
                    source_bytes: text.len().to_string(),
                    link_destination: None,
                    source: WorkEvidenceSourceV1::NativeExtraction,
                }),
            }
        })
        .collect()
    }

    #[test]
    fn repeated_header_citations_resolve_to_each_exact_value() {
        let mut table = json!({"rows":[
            {"cells":["Launch date","25 December 2021"],"source":"s1"},
            {"cells":["Launch vehicle","Ariane 5 ECA+"],"source":0},
            {"cells":["Diameter","6.5 m"],"source":"s1"}]});
        let mut uncited = Vec::new();
        let used =
            ground_sheet(&mut table, &["s1".into()], &sources(), false, &mut uncited).unwrap();
        assert!(uncited.is_empty());
        assert_eq!(used, ["s2", "s3", "s4"]);
        for (i, key) in used.iter().enumerate() {
            assert_eq!(table["rows"][i]["source"], *key);
        }
        assert_eq!(table["rows"][0]["cells"][1], "25 December 2021");
    }

    #[test]
    fn transformed_ambiguous_truncated_and_other_page_values_do_not_borrow_citations() {
        for case in 0..4 {
            let mut sources = sources();
            let value = if case == 0 {
                "2021-12-25"
            } else {
                "25 December 2021"
            };
            match case {
                1 => {
                    let mut duplicate = sources[1].clone();
                    duplicate.key = "s5".into();
                    duplicate.link.source_id = 5;
                    sources.push(duplicate);
                }
                2 => sources[1].preview.as_mut().unwrap().truncated = true,
                3 => sources[1].link.extraction_id = WorkArtifactId::generate(),
                _ => {}
            }
            let mut table = json!({"rows":[{"cells":["Date",value],"source":"s1"}]});
            assert!(
                ground_sheet(&mut table, &[], &sources, false, &mut Vec::new()).is_err(),
                "case {case}"
            );
        }
    }

    #[test]
    fn a_retried_sheet_keeps_its_own_words_without_the_pages_citation() {
        let mut table = json!({"rows":[
            {"cells":["Launch date","25 December 2021"],"source":"s1"},
            {"cells":["Launch vehicle","Heavy lift rocket"],"source":"s1"},
            {"cells":["Diameter","6.5 m"],"source":"s1"}]});
        let refused =
            ground_sheet(&mut table.clone(), &[], &sources(), false, &mut Vec::new()).unwrap_err();
        assert!(
            refused.starts_with("sheet: rows[1].cells[1] (row 2, cell 2) "),
            "{refused}"
        );
        let mut uncited = Vec::new();
        let used = ground_sheet(&mut table, &[], &sources(), true, &mut uncited).unwrap();
        assert_eq!(uncited, [2]);
        assert_eq!(used, ["s2", "s4"]);
        assert_eq!(table["rows"][0]["source"], "s2");
        assert!(table["rows"][1].get("source").is_none());
        assert_eq!(table["rows"][1]["cells"][1], "Heavy lift rocket");
        assert_eq!(table["rows"][2]["source"], "s4");
    }

    #[test]
    fn exact_boundaries_preserve_case_numbers_and_unicode() {
        assert!(contains_value("Price: 129,99 zł / sztuka", "129,99 zł"));
        assert!(contains_value("Żółć 東京", "Żółć 東京"));
        for (text, value) in [
            ("118", "18"),
            ("6.5 m", "5 m"),
            ("January", "Jan"),
            ("RFC Title", "RFC title"),
        ] {
            assert!(!contains_value(text, value));
        }
    }
}
