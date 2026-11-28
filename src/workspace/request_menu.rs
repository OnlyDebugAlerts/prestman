use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder as _;

use super::*;

#[derive(Clone, Copy)]
enum MenuAction {
    CopyLink,
    Copy,
    Duplicate,
    Move(Option<usize>),
    Delete,
}

pub(super) fn item(
    label: impl Into<SharedString>,
    workspace: &WeakEntity<Workspace>,
    disabled: bool,
    action: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + Copy + 'static,
) -> PopupMenuItem {
    let workspace = workspace.clone();
    PopupMenuItem::new(label)
        .disabled(disabled)
        .on_click(move |_, window, cx| {
            let workspace = workspace.clone();
            window.defer(cx, move |window, cx| {
                _ = workspace.update(cx, |view, cx| action(view, window, cx));
            });
        })
}

fn request_item(
    label: impl Into<SharedString>,
    workspace: &WeakEntity<Workspace>,
    id: usize,
    action: MenuAction,
    disabled: bool,
) -> PopupMenuItem {
    item(label, workspace, disabled, move |view, window, cx| {
        view.apply_request_menu(id, action, window, cx);
    })
}

pub(super) fn build(
    menu: PopupMenu,
    workspace: WeakEntity<Workspace>,
    id: usize,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(view) = workspace.upgrade() else {
        return menu;
    };
    let view = view.read(cx);
    let Some(index) = view.request_index(id) else {
        return menu;
    };
    let entry = &view.requests[index];
    let collection_id = entry.collection_id;
    let has_url = !entry.draft.url.trim().is_empty();
    let can_delete = entry.saved.is_none() || (view.storage.is_ok() && view.team_can_edit());
    let mut destinations = view
        .collections
        .iter()
        .filter(|collection| Some(collection.id) != entry.collection_id)
        .map(|collection| {
            (
                Some(collection.id),
                crate::storage::lineage(&view.collections, collection.id)
                    .iter()
                    .map(|group| group.name.as_str())
                    .collect::<Vec<_>>()
                    .join(" / "),
            )
        })
        .collect::<Vec<_>>();
    if entry.collection_id.is_some() {
        destinations.push((None, "Ungrouped".into()));
    }

    let make_item = |label, action, disabled| request_item(label, &workspace, id, action, disabled);
    let menu = new_request_submenu(menu, workspace.clone(), collection_id, window, cx)
        .separator()
        .item(make_item("Copy link", MenuAction::CopyLink, !has_url))
        .separator()
        .item(make_item("Copy", MenuAction::Copy, false))
        .item(make_item("Duplicate", MenuAction::Duplicate, false))
        .item(item(
            "Rename",
            &workspace,
            false,
            move |view, window, cx| {
                view.rename_request(id, window, cx);
            },
        ));
    let menu = if destinations.is_empty() {
        menu
    } else {
        let workspace = workspace.clone();
        menu.submenu("Move to", window, cx, move |menu, _, _| {
            destinations
                .iter()
                .fold(menu, |menu, (destination, label)| {
                    menu.item(request_item(
                        label.as_str(),
                        &workspace,
                        id,
                        MenuAction::Move(*destination),
                        false,
                    ))
                })
        })
    };
    menu.separator()
        .item(make_item("Delete", MenuAction::Delete, !can_delete))
}

pub(super) fn build_collection(
    menu: PopupMenu,
    workspace: WeakEntity<Workspace>,
    id: usize,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(view) = workspace.upgrade() else {
        return menu;
    };
    let view = view.read(cx);
    let locked = view.storage.is_err() || !view.team_can_edit();
    let local = !view.in_team();
    let can_publish = local && !locked;
    let is_root = view
        .collections
        .iter()
        .any(|group| group.id == id && group.parent_id.is_none());
    new_request_submenu(menu, workspace.clone(), Some(id), window, cx)
        .item(item(
            "New folder",
            &workspace,
            locked,
            move |view, window, cx| view.edit_collection(CollectionEdit::NewFolder(id), window, cx),
        ))
        .item(item(
            "Rename",
            &workspace,
            locked,
            move |view, window, cx| {
                view.edit_collection(CollectionEdit::Rename(id), window, cx);
            },
        ))
        .when(local, |menu| {
            menu.item(item(
                "Variables",
                &workspace,
                false,
                move |view, window, cx| {
                    view.edit_variables(VariableScope::Collection(id), window, cx)
                },
            ))
        })
        .when(is_root && can_publish, |menu| {
            menu.item(item(
                "Publish to cloud…",
                &workspace,
                false,
                move |view, window, cx| {
                    view.open_cloud(Some(id), window, cx);
                },
            ))
        })
        .separator()
        .item(item(
            "Delete",
            &workspace,
            locked,
            move |view, window, cx| {
                view.remove_collection(id, window, cx);
            },
        ))
}

fn new_request_submenu(
    menu: PopupMenu,
    workspace: WeakEntity<Workspace>,
    collection_id: Option<usize>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    menu.submenu("New request", window, cx, move |menu, _, _| {
        request_types(menu, &workspace, collection_id)
    })
}

fn request_types(
    menu: PopupMenu,
    workspace: &WeakEntity<Workspace>,
    collection_id: Option<usize>,
) -> PopupMenu {
    [
        ("HTTP Request", Request::default as fn() -> Request),
        ("gRPC Request", Request::grpc),
        ("GraphQL Request", Request::graphql),
    ]
    .into_iter()
    .fold(menu, |menu, (label, request)| {
        menu.item(item(label, workspace, false, move |view, window, cx| {
            if collection_id.is_none_or(|id| view.collections.iter().any(|group| group.id == id)) {
                view.add_request(request(), collection_id, window, cx);
            }
        }))
    })
}

fn collection_item(workspace: &WeakEntity<Workspace>, cx: &App) -> PopupMenuItem {
    item(
        "New collection",
        workspace,
        storage_locked(workspace, cx),
        |view, window, cx| view.edit_collection(CollectionEdit::New, window, cx),
    )
}

fn openapi_item(workspace: &WeakEntity<Workspace>, cx: &App) -> PopupMenuItem {
    item(
        "Import Swagger / OpenAPI…",
        workspace,
        storage_locked(workspace, cx),
        |view, window, cx| view.open_api_import(window, cx),
    )
}

fn storage_locked(workspace: &WeakEntity<Workspace>, cx: &App) -> bool {
    workspace.upgrade().is_none_or(|view| {
        let view = view.read(cx);
        view.storage.is_err() || !view.team_can_edit()
    })
}

pub(super) fn build_add(menu: PopupMenu, workspace: WeakEntity<Workspace>, cx: &App) -> PopupMenu {
    request_types(menu, &workspace, None)
        .separator()
        .item(collection_item(&workspace, cx))
}

pub(super) fn build_settings(
    menu: PopupMenu,
    workspace: WeakEntity<Workspace>,
    cx: &App,
) -> PopupMenu {
    let use_local_variables = workspace
        .upgrade()
        .and_then(|view| view.read(cx).team_uses_local_workspace_variables());
    let menu = match use_local_variables {
        Some(enabled) => menu.item(
            item(
                "Use Local workspace variables",
                &workspace,
                false,
                |view, _, cx| view.toggle_team_local_workspace_variables(cx),
            )
            .checked(enabled),
        ),
        None => menu
            .item(item("Globals", &workspace, false, |view, window, cx| {
                view.edit_variables(VariableScope::Global, window, cx);
            }))
            .item(item(
                "Environment",
                &workspace,
                false,
                |view, window, cx| {
                    view.edit_variables(VariableScope::Environment, window, cx);
                },
            )),
    };
    menu.separator()
        .item(openapi_item(&workspace, cx))
        .separator()
        .item(item(
            format!("Check for updates… (v{})", crate::updater::VERSION),
            &workspace,
            workspace
                .upgrade()
                .is_none_or(|view| !view.read(cx).updates.can_check()),
            |view, window, cx| view.check_for_updates(true, window, cx),
        ))
}

pub(super) fn build_sidebar(
    menu: PopupMenu,
    workspace: WeakEntity<Workspace>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    new_request_submenu(menu, workspace.clone(), None, window, cx)
        .item(collection_item(&workspace, cx))
}

impl Workspace {
    pub(super) fn rename_request(
        &mut self,
        id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.request_index(id) else {
            return;
        };
        if !self.select_request(index, window, cx) {
            return;
        }
        self.request_rename = Some(RequestRename {
            id,
            original: self.requests[index].draft.name.clone(),
        });
        let title = self.requests[index].draft.title().to_owned();
        self.name.update(cx, |name, cx| {
            name.set_value(title, window, cx);
            name.set_selected_range(0..name.value().len(), cx);
            name.focus_handle(cx).focus(window, cx);
        });
        cx.notify();
    }

    fn apply_request_menu(
        &mut self,
        id: usize,
        action: MenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.request_index(id) else {
            return;
        };
        match action {
            MenuAction::CopyLink => {
                let url = self.requests[index].draft.url.trim().to_owned();
                cx.write_to_clipboard(ClipboardItem::new_string(url));
            }
            MenuAction::Copy => {
                if let Ok(json) = serde_json::to_string_pretty(&self.requests[index].draft) {
                    cx.write_to_clipboard(ClipboardItem::new_string(json));
                }
            }
            MenuAction::Duplicate => {
                let mut duplicate = self.requests[index].draft.clone();
                duplicate.name = format!("{} (copy)", duplicate.title());
                self.add_request(duplicate, self.requests[index].collection_id, window, cx);
            }
            MenuAction::Move(destination) => self.move_request(id, destination, cx),
            MenuAction::Delete => self.delete_request(index, window, cx),
        }
    }

    fn delete_request(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let previous_selection = self.selected;
        let entry = self.requests.remove(index);
        if index < previous_selection {
            self.selected -= 1;
        } else {
            self.selected = previous_selection.min(self.requests.len().saturating_sub(1));
        }
        if (entry.saved.is_some() || self.in_team())
            && !self.persist("Could not delete request", cx)
        {
            self.requests.insert(index, entry);
            self.selected = previous_selection;
            return;
        }

        self.save_error = None;
        self.reconcile_request_tabs();
        if self.requests.is_empty() {
            self.new_request(window, cx);
            return;
        }
        if previous_selection == index {
            self.restore_request_view();
            self.show_selection(window, cx);
        }
        cx.notify();
    }
}
