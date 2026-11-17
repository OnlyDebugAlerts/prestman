use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};

use super::*;

#[derive(Clone, Copy)]
enum MenuAction {
    CopyLink,
    Rename,
    Copy,
    Duplicate,
    Move(Option<usize>),
    Delete,
}

fn item(
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
    let has_url = !entry.draft.url.trim().is_empty();
    let can_delete = entry.saved.is_none() || view.storage.is_ok();
    let mut destinations = view
        .collections
        .iter()
        .filter(|collection| Some(collection.id) != entry.collection_id)
        .map(|collection| (Some(collection.id), collection.name.clone()))
        .collect::<Vec<_>>();
    if entry.collection_id.is_some() {
        destinations.push((None, "Ungrouped".into()));
    }

    let make_item = |label, action, disabled| request_item(label, &workspace, id, action, disabled);
    let menu = menu
        .item(make_item("Copy link", MenuAction::CopyLink, !has_url))
        .separator()
        .item(make_item("Rename", MenuAction::Rename, false))
        .item(make_item("Copy", MenuAction::Copy, false))
        .item(make_item("Duplicate", MenuAction::Duplicate, false));
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
    cx: &App,
) -> PopupMenu {
    let Some(view) = workspace.upgrade() else {
        return menu;
    };
    let view = view.read(cx);
    let locked = view.storage.is_err();
    menu.item(item(
        "New request",
        &workspace,
        false,
        move |view, window, cx| {
            if view
                .collections
                .iter()
                .any(|collection| collection.id == id)
            {
                view.add_request(Request::default(), Some(id), window, cx);
            }
        },
    ))
    .item(item(
        "Rename",
        &workspace,
        locked,
        move |view, window, cx| {
            view.edit_collection(CollectionEdit::Rename(id), window, cx);
        },
    ))
    .separator()
    .item(item(
        "Delete empty collection",
        &workspace,
        locked || !view.collection_is_empty(id),
        move |view, window, cx| {
            view.remove_collection(id, window, cx);
        },
    ))
}

impl Workspace {
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
            MenuAction::Rename => {
                self.select_request(index, window, cx);
                self.name.update(cx, |name, cx| {
                    name.set_selected_range(0..name.value().len(), cx);
                    name.focus_handle(cx).focus(window, cx);
                });
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
        let entry = self.requests.remove(index);
        if entry.saved.is_some() && !self.persist("Could not delete request", cx) {
            self.requests.insert(index, entry);
            return;
        }

        self.save_error = None;
        if self.requests.is_empty() {
            self.new_request(window, cx);
            return;
        }
        if self.selected == index {
            self.selected = index.min(self.requests.len() - 1);
            self.show_selection(window, cx);
        } else if index < self.selected {
            self.selected -= 1;
        }
        cx.notify();
    }
}
