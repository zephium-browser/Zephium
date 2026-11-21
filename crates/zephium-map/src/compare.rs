use crate::{site::EntryId, Place, SectionRole};

pub struct Row {
    pub role: Option<SectionRole>,
    pub label: String,
    pub left: Option<EntryId>,
    pub right: Option<EntryId>,
}
fn normalized(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
pub fn compare(left: &Place, right: &Place, shared: Option<SectionRole>) -> Vec<Row> {
    let mut taken = vec![false; right.sections.len()];
    let mut pairs = vec![None; left.sections.len()];
    for by_role in [true, false] {
        for (i, l) in left.sections.iter().enumerate() {
            if pairs[i].is_some() {
                continue;
            }
            if let Some((j, _)) = right.sections.iter().enumerate().find(|(j, r)| {
                !taken[*j]
                    && if by_role {
                        l.role.is_some() && l.role == r.role
                    } else {
                        normalized(&l.label) == normalized(&r.label)
                    }
            }) {
                pairs[i] = Some(j);
                taken[j] = true;
            }
        }
    }
    let mut rows = Vec::new();
    for (l, j) in left.sections.iter().zip(pairs) {
        rows.push(Row {
            role: l.role.or_else(|| j.and_then(|i| right.sections[i].role)),
            label: l.label.clone(),
            left: Some(l.id.clone()),
            right: j.map(|i| right.sections[i].id.clone()),
        });
    }
    for (r, used) in right.sections.iter().zip(taken) {
        if !used {
            rows.push(Row {
                role: r.role,
                label: r.label.clone(),
                left: None,
                right: Some(r.id.clone()),
            });
        }
    }
    rows.sort_by_key(|r| {
        (
            shared.is_some() && shared != r.role,
            r.left.is_none() || r.right.is_none(),
        )
    });
    rows
}
