use std::path::PathBuf;

use anyhow::{Context as _, Result};

use super::*;
use crate::{
    collection_io,
    openapi::{self, ImportedCollection},
};

pub(super) struct OpenApiImport {
    pub(super) url: Entity<InputState>,
    pub(super) busy: bool,
    pub(super) error: Option<String>,
    _subscription: Subscription,
}

enum CollectionExport {
    Json(Vec<u8>),
    Directory(Vec<(usize, Request)>),
}

impl Workspace {
    pub(super) fn open_api_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.storage.is_err() || self.openapi_import.is_some() {
            return;
        }
        let url = cx.new(|cx| {
            InputState::new(window, cx).placeholder("https://api.example.com/openapi.json")
        });
        let subscription =
            cx.subscribe_in(&url, window, |view, _, event, window, cx| match event {
                InputEvent::Change => {
                    if let Some(import) = &mut view.openapi_import {
                        import.error = None;
                        cx.notify();
                    }
                }
                InputEvent::PressEnter { .. } => view.import_api_url(window, cx),
                _ => {}
            });
        url.read(cx).focus_handle(cx).focus(window, cx);
        self.openapi_import = Some(OpenApiImport {
            url,
            busy: false,
            error: None,
            _subscription: subscription,
        });
        self.variable_hover = None;
        self.sync_visualizer(window, cx);
        cx.notify();
    }

    pub(super) fn close_api_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.openapi_import = None;
        self.sync_visualizer(window, cx);
        if let Some(editor) = &self.variable_editor {
            editor.fields[0]
                .name
                .read(cx)
                .focus_handle(cx)
                .focus(window, cx);
        } else {
            self.focus_url(window, cx);
        }
        cx.notify();
    }

    pub(super) fn import_api_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(import) = self.openapi_import.as_mut().filter(|import| !import.busy) else {
            return;
        };
        import.busy = true;
        import.error = None;
        let id = import.url.entity_id();
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import Swagger / OpenAPI".into()),
        });
        cx.spawn_in(window, async move |view, cx| {
            let result = async {
                let paths = picker
                    .await
                    .context("The file dialog closed unexpectedly")??;
                let Some(path) = paths.and_then(|paths| paths.into_iter().next()) else {
                    return Ok(None);
                };
                cx.background_spawn(async move { openapi::read_file(&path) })
                    .await
                    .map(Some)
            }
            .await;
            _ = view.update_in(cx, |view, window, cx| {
                view.finish_api_import(id, result, window, cx)
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn import_api_url(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(import) = self.openapi_import.as_mut().filter(|import| !import.busy) else {
            return;
        };
        let address = import.url.read(cx).value().trim().to_owned();
        if address.is_empty() {
            return;
        }
        import.busy = true;
        import.error = None;
        let id = import.url.entity_id();
        cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_spawn(async move { openapi::read_url(&address) })
                .await
                .map(Some);
            _ = view.update_in(cx, |view, window, cx| {
                view.finish_api_import(id, result, window, cx)
            });
        })
        .detach();
        cx.notify();
    }

    fn finish_api_import(
        &mut self,
        id: EntityId,
        result: Result<Option<ImportedCollection>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .openapi_import
            .as_ref()
            .is_none_or(|import| import.url.entity_id() != id)
        {
            return;
        }
        let result = result.and_then(|collection| {
            if let Some(collection) = collection {
                self.insert_import(collection, window, cx)?;
            }
            Ok(())
        });
        if let Some(import) = &mut self.openapi_import {
            import.busy = false;
            import.error = result.err().map(|error| format!("{error:#}"));
        }
        cx.notify();
    }

    fn insert_import(
        &mut self,
        imported: ImportedCollection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let collection_id = self.next_collection_id;
        let next_collection_id = collection_id
            .checked_add(1 + imported.folders.len())
            .context("Too many collections")?;
        let next_request_id = self
            .next_request_id
            .checked_add(imported.requests.len())
            .context("Too many requests")?;
        let first = self.requests.len();
        let previous_groups = self.collections.len();
        let remap = |id: usize| collection_id.checked_add(id).context("Too many folders");
        let folders = imported
            .folders
            .into_iter()
            .map(|mut folder| {
                folder.id = remap(folder.id)?;
                folder.parent_id = Some(remap(folder.parent_id.unwrap_or(0))?);
                Ok(folder)
            })
            .collect::<Result<Vec<_>>>()?;
        let requests = imported
            .requests
            .into_iter()
            .enumerate()
            .map(|(index, (parent, request))| {
                Ok(Entry::new(
                    self.next_request_id + index,
                    Some(remap(parent)?),
                    request.clone(),
                    Some(request),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        self.collections.push(Collection {
            parent_id: None,
            id: collection_id,
            name: imported.name,
            variables: imported.variables,
        });
        self.collections.extend(folders);
        self.requests.extend(requests);
        // One atomic storage update: failure restores the in-memory state too.
        if !self.persist("Could not import collection", cx) {
            self.collections.truncate(previous_groups);
            self.requests.truncate(first);
            anyhow::bail!(
                "{}",
                self.save_error
                    .take()
                    .unwrap_or_else(|| "Could not save the collection".into())
            );
        }
        self.openapi_import = None;
        self.next_collection_id = next_collection_id;
        self.next_request_id = next_request_id;
        if first < self.requests.len() {
            self.request_pane = RequestPane::Body;
            self.select_request(first, window, cx);
        }
        Ok(())
    }

    pub(super) fn export_collection(
        &mut self,
        id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(collection) = self
            .collections
            .iter()
            .find(|collection| collection.id == id)
        else {
            return;
        };
        let mut collection = collection.clone();
        if let Some(editor) = self.variable_editor.as_ref().filter(
            |editor| matches!(editor.scope, VariableScope::Collection(scope) if scope == id),
        ) {
            match editor.variables(cx) {
                Ok(edited) => {
                    collection.variables = crate::variables::merge_edits(
                        &editor.original,
                        &edited,
                        &collection.variables,
                    )
                }
                Err(error) => {
                    self.save_error =
                        Some(format!("Could not export collection variables. {error:#}"));
                    cx.notify();
                    return;
                }
            }
        }
        // Export the current drafts, including requests that have not been saved yet.
        let ids = crate::storage::descendants(&self.collections, id);
        let mut groups = self.collections.clone();
        if let Some(editor) = &self.variable_editor {
            if let VariableScope::Collection(folder) = editor.scope {
                if folder != id && ids.contains(&folder) {
                    match editor.variables(cx) {
                        Ok(edited) => {
                            if let Some(group) = groups.iter_mut().find(|group| group.id == folder)
                            {
                                group.variables = crate::variables::merge_edits(
                                    &editor.original,
                                    &edited,
                                    &group.variables,
                                );
                            }
                        }
                        Err(error) => {
                            self.save_error =
                                Some(format!("Could not export folder variables. {error:#}"));
                            cx.notify();
                            return;
                        }
                    }
                }
            }
        }
        let drafts = self
            .requests
            .iter()
            .filter_map(|entry| {
                entry
                    .collection_id
                    .filter(|id| ids.contains(id))
                    .map(|id| (id, &entry.draft))
            })
            .collect::<Vec<_>>();
        let filename = collection_io::filename(&collection.name);
        let (filename, export) = if drafts
            .iter()
            .any(|(_, draft)| draft.grpc.is_some() || draft.graphql.is_some())
        {
            let filename = filename
                .trim_end_matches(".postman_collection.json")
                .to_owned();
            (
                filename,
                CollectionExport::Directory(
                    drafts
                        .into_iter()
                        .map(|(id, request)| (id, request.clone()))
                        .collect(),
                ),
            )
        } else {
            match collection_io::encode_tree(&collection, &groups, drafts.into_iter()) {
                Ok(bytes) => (filename, CollectionExport::Json(bytes)),
                Err(error) => {
                    self.save_error = Some(format!("Could not export collection. {error:#}"));
                    cx.notify();
                    return;
                }
            }
        };
        let directory = directories::UserDirs::new()
            .map(|dirs| dirs.document_dir().unwrap_or(dirs.home_dir()).to_owned())
            .unwrap_or_else(|| PathBuf::from("."));
        let picker = cx.prompt_for_new_path(&directory, Some(&filename));
        cx.spawn_in(window, async move |view, cx| {
            let result: Result<()> = async {
                let Some(path) = picker
                    .await
                    .context("The file dialog closed unexpectedly")??
                else {
                    return Ok(());
                };
                cx.background_spawn(async move {
                    match export {
                        CollectionExport::Json(bytes) => collection_io::write(&path, &bytes),
                        CollectionExport::Directory(requests) => {
                            collection_io::v3::write_directory(
                                &path,
                                &collection,
                                &groups,
                                &requests,
                            )
                        }
                    }
                })
                .await
            }
            .await;
            _ = view.update(cx, |view, cx| {
                if let Err(error) = result {
                    view.save_error = Some(format!("Could not export collection. {error:#}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }
    pub(super) fn export_request(
        &mut self,
        id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.request_index(id) else {
            return;
        };
        let request = self.requests[index].draft.clone();
        let directory = directories::UserDirs::new()
            .map(|dirs| dirs.document_dir().unwrap_or(dirs.home_dir()).to_owned())
            .unwrap_or_else(|| PathBuf::from("."));
        let picker = cx.prompt_for_new_path(
            &directory,
            Some(&collection_io::v3::request_filename(&request)),
        );
        cx.spawn_in(window, async move |view, cx| {
            let result: Result<()> = async {
                if let Some(path) = picker
                    .await
                    .context("The file dialog closed unexpectedly")??
                {
                    cx.background_spawn(async move {
                        collection_io::v3::write_request(&path, &request)
                    })
                    .await?;
                }
                Ok(())
            }
            .await;
            if let Err(error) = result {
                _ = view.update(cx, |view, cx| {
                    view.save_error = Some(format!("Could not export request. {error:#}"));
                    cx.notify();
                });
            }
        })
        .detach();
    }
}
