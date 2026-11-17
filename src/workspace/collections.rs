use std::collections::HashMap;

use super::*;

#[derive(Clone, Copy)]
pub(super) enum CollectionEdit {
    New,
    Rename(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SidebarRow {
    Collection(usize),
    Ungrouped,
    Request(usize),
}

impl Workspace {
    pub(super) fn sidebar_rows(&self) -> Vec<SidebarRow> {
        let mut grouped = HashMap::<_, Vec<_>>::new();
        for (index, entry) in self.requests.iter().enumerate() {
            grouped
                .entry(entry.collection_id)
                .or_default()
                .push(SidebarRow::Request(index));
        }

        let mut rows = Vec::with_capacity(self.collections.len() + self.requests.len() + 1);
        for (index, collection) in self.collections.iter().enumerate() {
            rows.push(SidebarRow::Collection(index));
            if !self.collapsed_collections.contains(&collection.id) {
                rows.extend(grouped.remove(&Some(collection.id)).into_iter().flatten());
            }
        }
        if !self.collections.is_empty() {
            rows.push(SidebarRow::Ungrouped);
        }
        rows.extend(grouped.remove(&None).into_iter().flatten());
        rows
    }

    pub(super) fn reveal_selected_request(&mut self) {
        if let Some(id) = self.active().collection_id {
            self.collapsed_collections.remove(&id);
        }
        if let Some(row) = self
            .sidebar_rows()
            .iter()
            .position(|row| *row == SidebarRow::Request(self.selected))
        {
            self.sidebar_scroll.scroll_to_item(row);
        }
    }

    pub(super) fn move_selection(
        &mut self,
        direction: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = self.sidebar_rows();
        let Some(current) = rows.iter().position(|row| match row {
            SidebarRow::Request(index) => *index == self.selected,
            SidebarRow::Collection(index) => {
                let id = self.collections[*index].id;
                self.active().collection_id == Some(id) && self.collapsed_collections.contains(&id)
            }
            SidebarRow::Ungrouped => false,
        }) else {
            return;
        };
        let request_index = |row: &SidebarRow| match *row {
            SidebarRow::Request(index) => Some(index),
            _ => None,
        };
        let next = if direction < 0 {
            rows[..current].iter().rev().find_map(request_index)
        } else {
            rows[current + 1..].iter().find_map(request_index)
        };
        if let Some(index) = next {
            self.select_request(index, window, cx);
        }
    }

    pub(super) fn edit_collection(
        &mut self,
        edit: CollectionEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.storage.is_err() {
            return;
        }
        let name = match edit {
            CollectionEdit::New => "",
            CollectionEdit::Rename(id) => {
                let Some(collection) = self
                    .collections
                    .iter()
                    .find(|collection| collection.id == id)
                else {
                    return;
                };
                &collection.name
            }
        };
        self.collection_edit = Some(edit);
        self.collection_name.update(cx, |input, cx| {
            input.set_value(name, window, cx);
            input.set_selected_range(0..name.len(), cx);
            input.focus_handle(cx).focus(window, cx);
        });
        cx.notify();
    }

    pub(super) fn close_collection_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.collection_edit = None;
        self.sidebar_focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn save_collection_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.collection_edit else {
            return;
        };
        let name = self.collection_name.read(cx).value().trim().to_owned();
        if name.is_empty() {
            self.collection_name
                .read(cx)
                .focus_handle(cx)
                .focus(window, cx);
            return;
        }
        match edit {
            CollectionEdit::New => {
                let id = self.next_collection_id;
                let Some(next_id) = id.checked_add(1) else {
                    return;
                };
                self.collections.push(Collection { id, name });
                if !self.persist("Could not create collection", cx) {
                    self.collections.pop();
                    return;
                }
                self.next_collection_id = next_id;
                if let Some(row) = self
                    .sidebar_rows()
                    .iter()
                    .position(|row| *row == SidebarRow::Collection(self.collections.len() - 1))
                {
                    self.sidebar_scroll.scroll_to_item(row);
                }
            }
            CollectionEdit::Rename(id) => {
                let Some(index) = self
                    .collections
                    .iter()
                    .position(|collection| collection.id == id)
                else {
                    return;
                };
                let previous = std::mem::replace(&mut self.collections[index].name, name);
                if !self.persist("Could not rename collection", cx) {
                    self.collections[index].name = previous;
                    return;
                }
            }
        }
        self.close_collection_edit(window, cx);
    }

    pub(super) fn collection_is_empty(&self, id: usize) -> bool {
        !self
            .requests
            .iter()
            .any(|entry| entry.collection_id == Some(id))
    }

    pub(super) fn remove_collection(
        &mut self,
        id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.collection_is_empty(id) {
            return;
        }
        let Some(index) = self
            .collections
            .iter()
            .position(|collection| collection.id == id)
        else {
            return;
        };
        let collection = self.collections.remove(index);
        if !self.persist("Could not remove collection", cx) {
            self.collections.insert(index, collection);
            return;
        }
        self.collapsed_collections.remove(&id);
        if matches!(self.collection_edit, Some(CollectionEdit::Rename(editing)) if editing == id) {
            self.close_collection_edit(window, cx);
        }
    }

    pub(super) fn move_request(
        &mut self,
        id: usize,
        destination: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.request_index(id) else {
            return;
        };
        if destination.is_some_and(|id| {
            !self
                .collections
                .iter()
                .any(|collection| collection.id == id)
        }) {
            return;
        }
        let previous = self.requests[index].collection_id;
        if previous == destination {
            return;
        }
        self.requests[index].collection_id = destination;
        if self.requests[index].saved.is_some() && !self.persist("Could not move request", cx) {
            self.requests[index].collection_id = previous;
            return;
        }
        if let Some(id) = destination {
            self.collapsed_collections.remove(&id);
        }
        if self.selected == index {
            self.reveal_selected_request();
        }
        cx.notify();
    }
}
