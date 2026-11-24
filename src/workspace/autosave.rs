use super::*;
use anyhow::{Context as _, Result};

fn save_drafts(
    storage: &Storage,
    collections: &[Collection],
    globals: &[Variable],
    environment: &[Variable],
    entries: &mut [Entry],
    sidebar: SidebarState<usize>,
) -> Result<()> {
    storage.save(
        collections,
        globals,
        environment,
        entries.iter().map(|entry| StoredRequest {
            collection_id: entry.collection_id,
            request: &entry.draft,
        }),
        sidebar,
    )?;
    // Only mark drafts as saved after the atomic file replacement succeeded.
    for entry in entries {
        entry.saved = Some(entry.draft.clone());
    }
    Ok(())
}

impl Workspace {
    pub(super) fn save_before_close(&mut self, cx: &mut Context<Self>) -> bool {
        match self.save_on_exit(cx) {
            Ok(()) => true,
            Err(error) => {
                self.save_error = Some(format!("Could not autosave before closing. {error:#}"));
                cx.notify();
                false
            }
        }
    }

    pub(super) fn save_on_exit(&mut self, cx: &App) -> Result<()> {
        if self.exit_saved {
            return Ok(());
        }
        if self.in_team() {
            self.save_team_cache(false, None)?;
            self.exit_saved = true;
            return Ok(());
        }
        let storage = self
            .storage
            .as_ref()
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        let mut collections = self.collections.clone();
        let mut globals = self.global_variables.clone();
        let mut environment = self.environment_variables.clone();
        let mut variable_error = None;
        if let Some(editor) = &self.variable_editor {
            match editor.variables(cx) {
                Ok(variables) => match editor.scope {
                    VariableScope::Global => {
                        globals =
                            crate::variables::merge_edits(&editor.original, &variables, &globals)
                    }
                    VariableScope::Environment => {
                        environment = crate::variables::merge_edits(
                            &editor.original,
                            &variables,
                            &environment,
                        )
                    }
                    VariableScope::Collection(id) => {
                        let collection = collections
                            .iter_mut()
                            .find(|collection| collection.id == id)
                            .context("The edited collection no longer exists")?;
                        collection.variables = crate::variables::merge_edits(
                            &editor.original,
                            &variables,
                            &collection.variables,
                        );
                    }
                },
                Err(error) => variable_error = Some(error),
            }
        }
        let name = self.collection_name.read(cx).value().trim().to_owned();
        let mut next_collection_id = self.next_collection_id;
        if let Some(edit) = self.collection_edit.filter(|_| !name.is_empty()) {
            match edit {
                CollectionEdit::New | CollectionEdit::NewFolder(_) => {
                    let parent_id = match edit {
                        CollectionEdit::NewFolder(id) => Some(id),
                        _ => None,
                    };
                    anyhow::ensure!(
                        parent_id.is_none_or(|id| collections.iter().any(|group| group.id == id)),
                        "The parent folder no longer exists"
                    );
                    let id = next_collection_id;
                    next_collection_id = id.checked_add(1).context("Too many collections")?;
                    collections.push(Collection {
                        parent_id,
                        id,
                        name,
                        variables: Vec::new(),
                    });
                }
                CollectionEdit::Rename(id) => {
                    if let Some(collection) = collections
                        .iter_mut()
                        .find(|collection| collection.id == id)
                    {
                        collection.name = name;
                    }
                }
            }
        }
        let sidebar = self.local_sidebar(true);
        save_drafts(
            storage,
            &collections,
            &globals,
            &environment,
            &mut self.requests,
            sidebar,
        )?;
        self.collections = collections;
        self.global_variables = globals;
        self.environment_variables = environment;
        self.next_collection_id = next_collection_id;
        self.collection_edit = None;
        if let Some(error) = variable_error {
            if let Some(editor) = &mut self.variable_editor {
                editor.error = Some(error.to_string());
            }
            // Request drafts are already safe, while invalid variable edits remain open for correction.
            return Err(error.context("Correct the variable editor before closing"));
        }
        self.exit_saved = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Collection, Entry, Request, SidebarState, Storage, Variable, save_drafts};

    #[test]
    fn exit_save_keeps_all_drafts_scripts_disabled_rows_and_environment_values()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("workspace");
        let (storage, _) = Storage::open_in(path.clone())?;
        let original = Request {
            name: "Saved request".into(),
            url: "https://example.test/old".into(),
            ..Request::default()
        };
        let mut changed = original.clone();
        changed.url = "{{base_url}}/new".into();
        changed.body = r#"{"changed":true}"#.into();
        changed.pre_request_script = "pm.variables.set('n', '123');".into();
        changed.post_response_script = "pm.environment.set('kid_id_42', 'kid');".into();
        changed.path_variables.push(crate::request::Pair {
            name: "code".into(),
            value: "{{theme}}".into(),
            enabled: true,
        });
        changed.headers.push(crate::request::Pair {
            name: "X-Disabled".into(),
            value: "value".into(),
            enabled: false,
        });
        let fresh = Request {
            url: "https://example.test/draft".into(),
            ..Request::default()
        };
        let mut entries = vec![
            Entry::new(0, Some(1), changed.clone(), Some(original)),
            Entry::new(1, Some(2), fresh.clone(), None),
        ];
        let collections = [
            Collection {
                parent_id: None,
                id: 1,
                name: "API".into(),
                variables: Vec::new(),
            },
            Collection {
                parent_id: Some(1),
                id: 2,
                name: "Nested folder".into(),
                variables: Vec::new(),
            },
        ];
        let environment = [Variable {
            name: "kid_id_42".into(),
            value: "kid".into(),
            enabled: true,
        }];
        let sidebar = SidebarState {
            collapsed_collections: [1, 2].into_iter().collect(),
            selected_request: Some(1),
            request_tabs: Some(vec![1, 0]),
        };
        save_drafts(
            &storage,
            &collections,
            &[],
            &environment,
            &mut entries,
            sidebar.clone(),
        )?;
        assert!(entries.iter().all(|entry| !entry.is_dirty()));
        drop(storage);
        let (_, restored) = Storage::open_in(path)?;
        assert_eq!(restored.requests.len(), 2);
        assert_eq!(restored.requests[0].request, changed);
        assert_eq!(restored.requests[1].request, fresh);
        assert_eq!(restored.requests[0].collection_id, Some(1));
        assert_eq!(restored.environment_variables, environment);
        assert_eq!(restored.sidebar, sidebar);
        Ok(())
    }

    #[test]
    fn failed_exit_save_keeps_previous_file_and_unsaved_state() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("workspace");
        let requests_path = path.join("requests.json");
        let backup = path.join("previous-requests.json");
        let (storage, _) = Storage::open_in(path)?;
        let mut entries = vec![Entry::new(
            0,
            None,
            Request {
                url: "https://example.test/original".into(),
                ..Request::default()
            },
            None,
        )];
        save_drafts(
            &storage,
            &[],
            &[],
            &[],
            &mut entries,
            SidebarState::default(),
        )?;
        let previous = std::fs::read(&requests_path)?;
        entries[0].draft.body = "unsaved changes".into();
        // An existing directory blocks the atomic replacement on each platform.
        std::fs::rename(&requests_path, &backup)?;
        std::fs::create_dir(&requests_path)?;
        let error = save_drafts(
            &storage,
            &[],
            &[],
            &[],
            &mut entries,
            SidebarState::default(),
        )
        .expect_err("atomic replacement should fail when the destination is a directory");
        assert!(
            error.to_string().starts_with("Could not replace "),
            "{error:#}"
        );
        assert!(entries[0].is_dirty());
        assert_eq!(std::fs::read(&backup)?, previous);
        Ok(())
    }
}
