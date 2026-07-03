//! 全局 prompt assembly 持久化（`index.sqlite` 的 prompt assembly 表）。

use std::path::Path;

use runtime_domain::prompt_assembly::{
    PromptSourceKind,
    persistence::{
        PersistedPromptAssemblyEntry, PersistedSkillDiscoverySkillEntry,
        PersistedToolSelectionEntry, PromptAssemblyScope, PromptAssemblyScopeState,
        StoredPromptBody,
    },
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::SessionStoreError;

pub(crate) fn save_global_prompt_assembly_state(
    index_path: &Path,
    state: &PromptAssemblyScopeState,
) -> Result<(), SessionStoreError> {
    if state.scope != PromptAssemblyScope::Global {
        return Err(SessionStoreError::ConfigurationError {
            message: format!(
                "global prompt assembly persistence only accepts global scope, got {}",
                state.scope.as_stored_value()
            ),
        });
    }

    crate::metadata::with_connection(index_path, |conn| {
        let transaction = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_err)?;
        let scope = PromptAssemblyScope::Global.as_stored_value();

        transaction
            .execute(
                "DELETE FROM prompt_assembly_entries WHERE scope = ?1",
                params![scope],
            )
            .map_err(sqlite_err)?;
        transaction
            .execute(
                "DELETE FROM prompt_assembly_extra_prompts WHERE scope = ?1",
                params![scope],
            )
            .map_err(sqlite_err)?;
        transaction
            .execute(
                "DELETE FROM prompt_assembly_core_overrides WHERE scope = ?1",
                params![scope],
            )
            .map_err(sqlite_err)?;
        transaction
            .execute(
                "DELETE FROM prompt_assembly_skill_discovery_overrides WHERE scope = ?1",
                params![scope],
            )
            .map_err(sqlite_err)?;
        transaction
            .execute(
                "DELETE FROM prompt_assembly_skill_discovery_skills WHERE scope = ?1",
                params![scope],
            )
            .map_err(sqlite_err)?;
        transaction
            .execute(
                "DELETE FROM prompt_assembly_tool_guideline_overrides WHERE scope = ?1",
                params![scope],
            )
            .map_err(sqlite_err)?;
        transaction
            .execute(
                "DELETE FROM prompt_assembly_tool_selections WHERE scope = ?1",
                params![scope],
            )
            .map_err(sqlite_err)?;

        for entry in sort_entries(state.entries.clone()) {
            transaction
                .execute(
                    "INSERT INTO prompt_assembly_entries (
                        scope,
                        reference_id,
                        kind,
                        title,
                        enabled,
                        requested_order
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        scope,
                        entry.reference_id,
                        prompt_source_kind_value(entry.kind),
                        entry.title,
                        entry.enabled,
                        entry.requested_order.map(i64::from),
                    ],
                )
                .map_err(sqlite_err)?;
        }

        for prompt in &state.extra_prompts {
            transaction
                .execute(
                    "INSERT INTO prompt_assembly_extra_prompts (
                        scope,
                        reference_id,
                        title,
                        body
                    ) VALUES (?1, ?2, ?3, ?4)",
                    params![scope, prompt.reference_id, prompt.title, prompt.body],
                )
                .map_err(sqlite_err)?;
        }

        if let Some(body) = state.core_system_override.as_deref() {
            transaction
                .execute(
                    "INSERT INTO prompt_assembly_core_overrides (scope, body) VALUES (?1, ?2)",
                    params![scope, body],
                )
                .map_err(sqlite_err)?;
        }

        if let Some(body) = state.skill_discovery_override.as_deref() {
            transaction
                .execute(
                    "INSERT INTO prompt_assembly_skill_discovery_overrides (scope, body) VALUES (?1, ?2)",
                    params![scope, body],
                )
                .map_err(sqlite_err)?;
        }

        for skill in sort_skill_discovery_skills(state.skill_discovery_skills.clone()) {
            transaction
                .execute(
                    "INSERT INTO prompt_assembly_skill_discovery_skills (
                        scope,
                        skill_name,
                        enabled,
                        requested_order
                    ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        scope,
                        skill.skill_name,
                        skill.enabled,
                        skill.requested_order.map(i64::from),
                    ],
                )
                .map_err(sqlite_err)?;
        }

        if let Some(body) = state.tool_guidelines_override.as_deref() {
            transaction
                .execute(
                    "INSERT INTO prompt_assembly_tool_guideline_overrides (scope, body) VALUES (?1, ?2)",
                    params![scope, body],
                )
                .map_err(sqlite_err)?;
        }

        for tool in sort_tool_selections(state.tool_selections.clone()) {
            transaction
                .execute(
                    "INSERT INTO prompt_assembly_tool_selections (
                        scope,
                        tool_name,
                        enabled,
                        requested_order
                    ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        scope,
                        tool.tool_name,
                        tool.enabled,
                        tool.requested_order.map(i64::from),
                    ],
                )
                .map_err(sqlite_err)?;
        }

        transaction.commit().map_err(sqlite_err)?;
        Ok(())
    })
}

pub(crate) fn load_global_prompt_assembly_state(
    index_path: &Path,
) -> Result<PromptAssemblyScopeState, SessionStoreError> {
    crate::metadata::with_connection(index_path, |conn| {
        let scope = PromptAssemblyScope::Global.as_stored_value();
        let mut entries_statement = conn
            .prepare(
                "SELECT reference_id, kind, title, enabled, requested_order
                 FROM prompt_assembly_entries
                 WHERE scope = ?1
                 ORDER BY
                    CASE WHEN requested_order IS NULL THEN 1 ELSE 0 END,
                    requested_order ASC,
                    reference_id ASC",
            )
            .map_err(sqlite_err)?;
        let entries = entries_statement
            .query_map(params![scope], |row| {
                let kind = parse_prompt_source_kind(&row.get::<_, String>(1)?)?;
                let requested_order = row
                    .get::<_, Option<i64>>(4)?
                    .map(|value| {
                        u16::try_from(value).map_err(|_| {
                            rusqlite::Error::FromSqlConversionFailure(
                                4,
                                rusqlite::types::Type::Integer,
                                Box::new(std::io::Error::other(
                                    "requested_order exceeds u16 range",
                                )),
                            )
                        })
                    })
                    .transpose()?;
                Ok(PersistedPromptAssemblyEntry {
                    reference_id: row.get(0)?,
                    kind,
                    title: row.get(2)?,
                    enabled: row.get(3)?,
                    requested_order,
                })
            })
            .map_err(sqlite_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err)?;

        let mut prompts_statement = conn
            .prepare(
                "SELECT reference_id, title, body
                 FROM prompt_assembly_extra_prompts
                 WHERE scope = ?1
                 ORDER BY reference_id ASC",
            )
            .map_err(sqlite_err)?;
        let extra_prompts = prompts_statement
            .query_map(params![scope], |row| {
                Ok(StoredPromptBody {
                    reference_id: row.get(0)?,
                    title: row.get(1)?,
                    body: row.get(2)?,
                })
            })
            .map_err(sqlite_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err)?;

        let core_system_override = conn
            .query_row(
                "SELECT body FROM prompt_assembly_core_overrides WHERE scope = ?1",
                params![scope],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_err)?;

        let skill_discovery_override = conn
            .query_row(
                "SELECT body FROM prompt_assembly_skill_discovery_overrides WHERE scope = ?1",
                params![scope],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_err)?;

        let mut discovery_skills_statement = conn
            .prepare(
                "SELECT skill_name, enabled, requested_order
                 FROM prompt_assembly_skill_discovery_skills
                 WHERE scope = ?1
                 ORDER BY
                    CASE WHEN requested_order IS NULL THEN 1 ELSE 0 END,
                    requested_order ASC,
                    skill_name ASC",
            )
            .map_err(sqlite_err)?;
        let skill_discovery_skills = discovery_skills_statement
            .query_map(params![scope], |row| {
                let requested_order = row
                    .get::<_, Option<i64>>(2)?
                    .map(|value| {
                        u16::try_from(value).map_err(|_| {
                            rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Integer,
                                Box::new(std::io::Error::other(
                                    "requested_order exceeds u16 range",
                                )),
                            )
                        })
                    })
                    .transpose()?;
                Ok(PersistedSkillDiscoverySkillEntry {
                    skill_name: row.get(0)?,
                    enabled: row.get(1)?,
                    requested_order,
                })
            })
            .map_err(sqlite_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err)?;

        let tool_guidelines_override = conn
            .query_row(
                "SELECT body FROM prompt_assembly_tool_guideline_overrides WHERE scope = ?1",
                params![scope],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_err)?;

        let mut tool_selections_statement = conn
            .prepare(
                "SELECT tool_name, enabled, requested_order
                 FROM prompt_assembly_tool_selections
                 WHERE scope = ?1
                 ORDER BY
                    CASE WHEN requested_order IS NULL THEN 1 ELSE 0 END,
                    requested_order ASC,
                    tool_name ASC",
            )
            .map_err(sqlite_err)?;
        let tool_selections = tool_selections_statement
            .query_map(params![scope], |row| {
                let requested_order = row
                    .get::<_, Option<i64>>(2)?
                    .map(|value| {
                        u16::try_from(value).map_err(|_| {
                            rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Integer,
                                Box::new(std::io::Error::other(
                                    "requested_order exceeds u16 range",
                                )),
                            )
                        })
                    })
                    .transpose()?;
                Ok(PersistedToolSelectionEntry {
                    tool_name: row.get(0)?,
                    enabled: row.get(1)?,
                    requested_order,
                })
            })
            .map_err(sqlite_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err)?;

        Ok(PromptAssemblyScopeState {
            scope: PromptAssemblyScope::Global,
            core_system_override,
            skill_discovery_override,
            tool_guidelines_override,
            entries,
            skill_discovery_skills,
            tool_selections,
            extra_prompts,
        })
    })
}

fn prompt_source_kind_value(kind: PromptSourceKind) -> &'static str {
    match kind {
        PromptSourceKind::CoreSystemPrompt => "core_system_prompt",
        PromptSourceKind::InstructionsFile => "instructions_file",
        PromptSourceKind::ExtraPrompt => "extra_prompt",
        PromptSourceKind::SkillDiscovery => "skill_discovery",
        PromptSourceKind::LongLivedSkill => "long_lived_skill",
        PromptSourceKind::ToolGuidelines => "tool_guidelines",
    }
}

fn parse_prompt_source_kind(value: &str) -> Result<PromptSourceKind, rusqlite::Error> {
    match value {
        "core_system_prompt" => Ok(PromptSourceKind::CoreSystemPrompt),
        "instructions_file" => Ok(PromptSourceKind::InstructionsFile),
        "extra_prompt" => Ok(PromptSourceKind::ExtraPrompt),
        "skill_discovery" => Ok(PromptSourceKind::SkillDiscovery),
        "long_lived_skill" => Ok(PromptSourceKind::LongLivedSkill),
        "tool_guidelines" => Ok(PromptSourceKind::ToolGuidelines),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::other(format!(
                "unknown prompt source kind `{value}`"
            ))),
        )),
    }
}

fn sqlite_err(source: rusqlite::Error) -> SessionStoreError {
    SessionStoreError::SqliteError { source }
}

fn sort_entries(
    mut entries: Vec<PersistedPromptAssemblyEntry>,
) -> Vec<PersistedPromptAssemblyEntry> {
    entries.sort_by(|left, right| {
        left.requested_order
            .unwrap_or(u16::MAX)
            .cmp(&right.requested_order.unwrap_or(u16::MAX))
            .then_with(|| left.reference_id.cmp(&right.reference_id))
    });
    entries
}

fn sort_skill_discovery_skills(
    mut entries: Vec<PersistedSkillDiscoverySkillEntry>,
) -> Vec<PersistedSkillDiscoverySkillEntry> {
    entries.sort_by(|left, right| {
        left.requested_order
            .unwrap_or(u16::MAX)
            .cmp(&right.requested_order.unwrap_or(u16::MAX))
            .then_with(|| left.skill_name.cmp(&right.skill_name))
    });
    entries
}

fn sort_tool_selections(
    mut entries: Vec<PersistedToolSelectionEntry>,
) -> Vec<PersistedToolSelectionEntry> {
    entries.sort_by(|left, right| {
        left.requested_order
            .unwrap_or(u16::MAX)
            .cmp(&right.requested_order.unwrap_or(u16::MAX))
            .then_with(|| left.tool_name.cmp(&right.tool_name))
    });
    entries
}

#[cfg(test)]
mod tests {
    use runtime_domain::prompt_assembly::{
        PromptAssemblyInput, PromptSourceInactiveReason, PromptSourceOrigin, PromptSourceStatus,
        resolve_prompt_assembly,
    };
    use tempfile::tempdir;

    use super::*;

    fn sample_global_state() -> PromptAssemblyScopeState {
        PromptAssemblyScopeState {
            scope: PromptAssemblyScope::Global,
            core_system_override: Some("global core override".to_string()),
            skill_discovery_override: None,
            entries: vec![
                PersistedPromptAssemblyEntry {
                    reference_id: "skill-discovery".to_string(),
                    kind: PromptSourceKind::SkillDiscovery,
                    title: "Skill discovery source".to_string(),
                    enabled: true,
                    requested_order: Some(5),
                },
                PersistedPromptAssemblyEntry {
                    reference_id: "shared-rules".to_string(),
                    kind: PromptSourceKind::ExtraPrompt,
                    title: "shared-rules".to_string(),
                    enabled: true,
                    requested_order: Some(10),
                },
            ],
            skill_discovery_skills: Vec::new(),
            extra_prompts: vec![StoredPromptBody {
                reference_id: "shared-rules".to_string(),
                title: "shared-rules".to_string(),
                body: "global rules".to_string(),
            }],
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
        }
    }

    #[tokio::test]
    async fn global_prompt_assembly_roundtrip_persists_entries_bodies_and_core_override() {
        let root = tempdir().expect("tempdir should exist");
        let store = crate::LocalSessionStore::open_in(root.path().to_path_buf())
            .await
            .expect("local session store should open");
        let state = sample_global_state();

        crate::store::SessionStore::save_global_prompt_assembly_state(&store, &state)
            .await
            .expect("global prompt assembly should save");
        let loaded = crate::store::SessionStore::load_global_prompt_assembly_state(&store)
            .await
            .expect("global prompt assembly should load");

        assert_eq!(loaded, state);
    }

    #[tokio::test]
    async fn loaded_project_scope_can_override_loaded_global_scope_at_resolution_time() {
        let root = tempdir().expect("tempdir should exist");
        let work_dir = root.path().join("repo");
        std::fs::create_dir_all(&work_dir).expect("work dir should exist");
        let store = crate::LocalSessionStore::open_in(root.path().join("hunea"))
            .await
            .expect("local session store should open");

        let global_state = sample_global_state();
        crate::store::SessionStore::save_global_prompt_assembly_state(&store, &global_state)
            .await
            .expect("global prompt assembly should save");
        let loaded_global = crate::store::SessionStore::load_global_prompt_assembly_state(&store)
            .await
            .expect("global prompt assembly should load");

        let project_state =
            runtime_domain::prompt_assembly::persistence::PromptAssemblyScopeState {
                scope: PromptAssemblyScope::Project,
                core_system_override: Some("project core override".to_string()),
                skill_discovery_override: None,
                entries: vec![PersistedPromptAssemblyEntry {
                    reference_id: "shared-rules".to_string(),
                    kind: PromptSourceKind::ExtraPrompt,
                    title: "shared-rules".to_string(),
                    enabled: true,
                    requested_order: Some(10),
                }],
                skill_discovery_skills: Vec::new(),
                extra_prompts: vec![StoredPromptBody {
                    reference_id: "shared-rules".to_string(),
                    title: "shared-rules".to_string(),
                    body: "project rules".to_string(),
                }],
                tool_guidelines_override: None,
                tool_selections: Vec::new(),
            };
        runtime_domain::prompt_assembly::persistence::save_project_prompt_assembly_state(
            &work_dir,
            &project_state,
        )
        .expect("project prompt assembly should save");
        let loaded_project =
            runtime_domain::prompt_assembly::persistence::load_project_prompt_assembly_state(
                &work_dir,
            )
            .expect("project prompt assembly should load");

        let input = PromptAssemblyInput {
            core_system: runtime_domain::prompt_assembly::CoreSystemPromptInput {
                global_override_present: loaded_global.core_system_override.is_some(),
                project_override_present: loaded_project.core_system_override.is_some(),
            },
            candidates: vec![
                runtime_domain::prompt_assembly::PromptSourceCandidate {
                    reference_id: "shared-rules".to_string(),
                    kind: PromptSourceKind::ExtraPrompt,
                    title: "shared-rules".to_string(),
                    origin: Some(PromptSourceOrigin::Global),
                    collision_key: Some("shared-rules".to_string()),
                    enabled: true,
                    resolvable: true,
                    requested_order: Some(10),
                },
                runtime_domain::prompt_assembly::PromptSourceCandidate {
                    reference_id: "shared-rules".to_string(),
                    kind: PromptSourceKind::ExtraPrompt,
                    title: "shared-rules".to_string(),
                    origin: Some(PromptSourceOrigin::Project),
                    collision_key: Some("shared-rules".to_string()),
                    enabled: true,
                    resolvable: true,
                    requested_order: Some(10),
                },
            ],
        };

        let snapshot = resolve_prompt_assembly(&input);

        assert_eq!(
            snapshot.active_sources[0].origin,
            Some(PromptSourceOrigin::Project)
        );
        assert!(
            snapshot.inactive_sources.iter().any(|source| {
                source.reference_id == "shared-rules"
                    && matches!(
                        source.status,
                        PromptSourceStatus::Inactive {
                            reason: PromptSourceInactiveReason::Shadowed,
                        }
                    )
                    && source.origin == Some(PromptSourceOrigin::Global)
            }),
            "loaded global state should stay present but be shadowed by project state"
        );
    }
}
