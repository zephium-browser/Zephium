<String>>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, Option<String>>(8)?,
                        r.get::<_, f64>(9)?,
                    ))
                },
            )?;
            let mut rows = Vec::with_capacity(item_count as usize);
            for mapped in mapped {
                let (raw_id, parent, space, section, position, kind, name, url, title, zoom) =
                    mapped?;
                let id = ItemId::parse(&raw_id)
                    .filter(|id| id.to_string() == raw_id)
                    .ok_or_else(|| invalid_data("compatibility item has an invalid id"))?;
                let parent = match parent {
                    Some(raw) => Some(
                        ItemId::parse(&raw)
                            .filter(|id| id.to_string() == raw)
                            .ok_or_else(|| {
                                invalid_data("compatibility item has an invalid parent id")
                            })?,
                    ),
                    None => None,
                };
                let placement = match (space, section.as_str()) {
                    // These files kept Essentials per profile; the first
                    // space holds them until the loader spreads them.
                    (None, "favorites") => Placement::Favorites {
                        space: essentials::home_space(&spaces, profile, None).ok_or_else(|| {
                            invalid_data("compatibility essentials have no space to belong to")
                        })?,
                    },
                    (Some(raw), "pinned" | "today") => {
                        let space = SpaceId::parse(&raw)
                            .filter(|id| id.to_string() == raw)
                            .ok_or_else(|| {
                                invalid_data("compatibility item has an invalid space id")
                            })?;
                        Placement::Space {
                            space,
                            section: if section == "pinned" {
                                SpaceSection::Pinned
                            } else {
                                SpaceSection::Today
                            },
                        }
                    }
                    _ => {
                        return Err(invalid_data("compatibility item has an invalid placement"));
                    }
                };
                let kind = match (kind.as_str(), name, url) {
                    ("folder", Some(name), None) => PersistedKind::Folder { name },
                    ("tab", None, Some(url)) => PersistedKind::Tab {
                        url,
                        title: title.unwrap_or_default(),
                        zoom,
                        last_active_ms: None,
                        custom_icon: None,
                    },
                    _ => {
                        return Err(invalid_data("compatibility item has an invalid kind"));
                    }
                };
                rows.push(Row {
                    id,
                    parent,
                    placement,
                    position,
                    kind,
                });
            }
            if rows.len() != item_count as usize {
                return Err(invalid_data(
                    "compatibility item rows were filtered by persistence limits",
                ));
            }
            rows
        };

        // Rebuild DFS order (parents before children) without recursive calls:
        // pre-v3 files are untrusted and may contain cycles or hostile depth.
        let mut roots: HashMap<Placement, Vec<usize>> = HashMap::new();
        let mut children: HashMap<ItemId, Vec<usize>> = HashMap::new();
        for (index, row) in rows.iter().enumerate() {
            match row.parent {
                Some(parent) => children.entry(parent).or_default().push(index),
                None => roots.entry(row.placement).or_default().push(index),
            }
        }
        for list in roots.values_mut().chain(children.values_mut()) {
            list.sort_by_key(|index| rows[*index].position);
        }
        let placements: Vec<Placement> = spaces
            .iter()
            .flat_map(|space| space_placements(space.id))
            .collect();
        let mut items = Vec::new();
        let mut emitted = HashSet::with_capacity(rows.len());
        'placements: for placement in placements {
            let Some(container_roots) = roots.get(&placement) else {
                continue;
            };
            let mut stack: Vec<(usize, usize)> = container_roots
                .iter()
                .rev()
                .map(|index| (*index, 0))
                .collect();
            while let Some((index, depth)) = stack.pop() {
                if items.len() >= item_budget {
                    break 'placements;
                }
                let row = &rows[index];
                if depth > MAX_ITEM_TREE_DEPTH || !emitted.insert(row.id) {
                    continue;
                }
                items.push(PersistedItem {
                    id: row.id,
                    parent: row.parent,
                    placement: row.placement,
                    kind: row.kind.clone(),
                });
                if depth == MAX_ITEM_TREE_DEPTH
                    || !matches!(&row.kind, PersistedKind::Folder { .. })
                {
                    continue;
                }
                if let Some(descendants) = children.get(&row.id) {
                    stack.extend(descendants.iter().rev().filter_map(|child| {
                        (rows[*child].placement == row.placement).then_some((*child, depth + 1))
                    }));
                }
            }
        }
        if emitted.len() != rows.len() {
            return Err(invalid_data(
                "compatibility item tree is dangling, cyclic, or too deep",
            ));
        }

        if focused {
            let focus = conn
                .query_row(
                    "SELECT active_space IS NULL,
                            CASE WHEN length(CAST(active_space AS BLOB)) <= 26 THEN active_space END,
                            active_item IS NULL,
                            CASE WHEN length(CAST(active_item AS BLOB)) <= 26 THEN active_item END,
                            splits IS NULL,
                            CASE WHEN length(CAST(splits AS BLOB)) <= ?1 THEN splits END
                     FROM focus WHERE id = 1",
                    [MAX_SPLIT_JSON_BYTES as i64],
                    |r| {
                        Ok((
                            (r.get::<_, bool>(0)?, r.get::<_, Option<String>>(1)?),
                            (r.get::<_, bool>(2)?, r.get::<_, Option<String>>(3)?),
                            (r.get::<_, bool>(4)?, r.get::<_, Option<String>>(5)?),
                        ))
                    },
                )
                .optional()?;
            let (space, item, splits) =
                focus.ok_or_else(|| invalid_data("compatibility focus row is missing"))?;
            let space = bounded_optional_text(space, "active space")?;
            let item = bounded_optional_text(item, "active item")?;
            let splits = bounded_optional_text(splits, "split tree")?;
            out.active_space = match space {
                Some(raw) => Some(
                    SpaceId::parse(&raw)
                        .filter(|id| id.to_string() == raw)
                        .ok_or_else(|| {
                            invalid_data("compatibility focus has an invalid active space")
                        })?,
                ),
                None => None,
            };
            out.active_item = match item {
                Some(raw) => Some(
                    ItemId::parse(&raw)
                        .filter(|id| id.to_string() == raw)
                        .ok_or_else(|| {
                            invalid_data("compatibility focus has an invalid active item")
                        })?,
                ),
                None => None,
            };
            out.splits = match splits {
                Some(json) => Some(pane::from_json(&json).ok_or_else(|| {
                    invalid_data("compatibility focus has an invalid split tree")
                })?),
                None => None,
            };
        }

        out.spaces.extend(spaces);
        out.items.extend(items);
        Ok(())
    }

    pub(super) fn purge_legacy_profile_state(&mut self) -> rusqlite::Result<()> {
        if let Some(dir) = &self.dir {
            let degraded = harden_registered_profile_files(dir, &self.registry, true, true)?;
            self.degraded_profiles.extend(degraded);
            self.profiles
                .retain(|profile, _| !self.degraded_profiles.contains(profile));
            return Ok(());
        }
        for conn in self.profiles.values_mut() {
            clear_legacy_session_rows_once(conn)?;
        }
        Ok(())
    }

    pub(super) fn import_legacy(&mut self, path: &Path) -> rusqlite::Result<()> {
        let data = legacy::read(path).ok_or_else(|| {
            rusqlite::Error::InvalidParameterName("invalid legacy browser database".into())
        })?;
        self.meta.execute(
            "INSERT INTO settings(key, value) VALUES (?1, 'started')
             ON CONFLICT(key) DO UPDATE SET value = 'started'",
            [LEGACY_IMPORT_STATE_KEY],
        )?;

        let authoritative = self.meta.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_snapshot WHERE id = 1)",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        if !authoritative {
            self.save(&data.session)?;
        }
        let first = self
            .load()?
            .and_then(|session| session.profiles.first().map(|profile| profile.id))
            .ok_or_else(|| invalid_data("legacy import produced no persistent profile"))?;
        let conn = self.profile_conn(first)?;
        let tx = conn.transaction()?;
        let imported = tx
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                [LEGACY_HISTORY_MARKER],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .as_deref()
            == Some("1");
        if !imported {
            {
                let mut insert = tx.prepare_cached(
                    "INSERT INTO history(url, title, visited_at) VALUES (?1, ?2, ?3)",
                )?;
                for (url, title, at) in &data.visits {
                    insert.execute(params![url, title, at])?;
                }
            }
            tx.execute(
                "DELETE FROM history WHERE id IN (
                     SELECT id FROM history
                     ORDER BY visited_at DESC, id DESC
                     LIMIT -1 OFFSET 50000
                 )",
                [],
            )?;
            tx.execute(
                "INSERT INTO settings(key, value) VALUES (?1, '1')
                 ON CONFLICT(key) DO UPDATE SET value = '1'",
                [LEGACY_HISTORY_MARKER],
            )?;
            enforce_history_budget(&tx)?;
        }
        tx.commit()?;
        self.meta.execute(
            "UPDATE settings SET value = 'complete' WHERE key = ?1",
            [LEGACY_IMPORT_STATE_KEY],
        )?;
        remove_legacy_source(path)
    }
}

fn bounded_optional_text(
    field: (bool, Option<String>),
    label: &str,
) -> rusqlite::Result<Option<String>> {
    match field {
        (true, None) => Ok(None),
        (false, Some(value)) => Ok(Some(value)),
        _ => Err(rusqlite::Error::InvalidParameterName(format!(
            "compatibility {label} exceeds persistence limit"
        ))),
    }
}

pub(super) fn remove_legacy_source(path: &Path) -> rusqlite::Result<()> {
    if !regular_file_exists(path)? {
        return Ok(());
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(rusqlite::Error::ToSqlConversionFailure(Box::new(error))),
    }
}

pub(super) fn clear_legacy_session_rows_once(conn: &mut Connection) -> rusqlite::Result<()> {
    let complete = conn
        .query_row(
            "SELECT value = 'complete' FROM settings WHERE key = ?1",
            [LEGACY_SESSION_PURGE_MARKER],
            |row| row.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false);
    if complete {
        return Ok(());
    }
    let tx = conn.transaction()?;
    tx.execute_batch("DELETE FROM items; DELETE FROM spaces; DELETE FROM focus;")?;
    tx.commit()?;
    // This is security cleanup, not optional compaction. secure_delete
    // overwrites deleted cells and the one-time checkpoint removes their WAL
    // copies. Full VACUUM is intentionally not on the startup path.
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, 'complete')
         ON CONFLICT(key) DO UPDATE SET value = 'complete'",
        [LEGACY_SESSION_PURGE_MARKER],
    )?;
    Ok(())
}

fn owns_placement(profile: ProfileId, s: &SessionState, placement: Placement) -> bool {
    match placement {
        Placement::Favorites { space } | Placement::Space { space, .. } => s
            .spaces
            .iter()
            .any(|x| x.id == space && x.profile == profile),
        // Never saved: a mini window's page belongs to no session.
        Placement::Mini { .. } => false,
    }
}

fn section_to_str(section: SpaceSection) -> &'static str {
    match section {
        SpaceSection::Pinned => "pinned",
        SpaceSection::Today => "today",
    }
}

#[cfg(test)]
mod tests {
    use super::super::filesystem::configure;
    use super::*;
    use crate::migrations;

    #[test]
    fn legacy_session_purge_is_durable_and_exactly_once() {
        let mut connection = Connection::open_in_memory().unwrap();
        configure(&connection).unwrap();
        migrations::apply(&mut connection, migrations::PROFILE).unwrap();
        connection
            .execute(
                "INSERT INTO spaces(id, name, position) VALUES ('space', 'Legacy', 0)",
                [],
            )
            .unwrap();

        clear_legacy_session_rows_once(&mut connection).unwrap();
        let changes_after_first = connection.total_changes();
        clear_legacy_session_rows_once(&mut connection).unwrap();

        assert_eq!(connection.total_changes(), changes_after_first);
        let marker: String = connection
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                [LEGACY_SESSION_PURGE_MARKER],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marker, "complete");
    }
}
