//! A shared native preferences object suspends only when every owner agrees.
use std::collections::HashMap;

pub(crate) fn suspension_groups(
    views: impl IntoIterator<Item = (usize, bool)>,
) -> HashMap<usize, bool> {
    let mut groups = HashMap::new();
    for (preferences, suspend) in views {
        let agreed = groups.entry(preferences).or_insert(true);
        *agreed &= suspend;
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_awake_sharer_prevents_suspension_independent_of_iteration_order() {
        for views in [
            vec![(1, false), (1, true), (2, true)],
            vec![(2, true), (1, true), (1, false)],
        ] {
            let groups = suspension_groups(views);
            assert_eq!(groups.get(&1), Some(&false));
            assert_eq!(groups.get(&2), Some(&true));
        }
        assert_eq!(
            suspension_groups([(1, true), (1, true)]).get(&1),
            Some(&true)
        );
    }
}
