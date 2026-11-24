use std::collections::HashMap;

use super::navigation::SidebarNavigate;
use super::*;

#[derive(Clone, Copy)]
pub(super) enum CollectionEdit {
    New,
    NewFolder(usize),
    Rename(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SidebarRow {
    Collection(usize),
    Edit,
    Ungrouped,
    Request(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SidebarTarget {
    Collection(usize),
    Request(usize),
}

// Hold removed entries (including live request tasks) until the storage update succeeds.
struct RemovedCollection {
    collections: Vec<(usize, Collection)>,
    ids: HashSet<usize>,
    requests: Vec<(usize, Entry)>,
}

impl RemovedCollection {
    fn take(
        id: usize,
        collections: &mut Vec<Collection>,
        requests: &mut Vec<Entry>,
    ) -> Option<Self> {
        if !collections.iter().any(|group| group.id == id) {
            return None;
        }
        let ids = crate::storage::descendants(collections, id);
        let mut removed_groups = Vec::new();
        *collections = std::mem::take(collections)
            .into_iter()
            .enumerate()
            .filter_map(|(index, group)| {
                if ids.contains(&group.id) {
                    removed_groups.push((index, group));
                    None
                } else {
                    Some(group)
                }
            })
            .collect();
        let mut removed = Vec::new();
        *requests = std::mem::take(requests)
            .into_iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                if entry.collection_id.is_some_and(|id| ids.contains(&id)) {
                    removed.push((index, entry));
                    None
                } else {
                    Some(entry)
                }
            })
            .collect();
        Some(Self {
            collections: removed_groups,
            ids,
            requests: removed,
        })
    }

    fn restore(self, collections: &mut Vec<Collection>, requests: &mut Vec<Entry>) {
        for (index, collection) in self.collections {
            collections.insert(index, collection);
        }
        for (index, entry) in self.requests {
            requests.insert(index, entry);
        }
    }
}

fn build_sidebar_rows(
    collections: &[Collection],
    request_parents: impl IntoIterator<Item = Option<usize>>,
    collapsed: &HashSet<usize>,
    edit: Option<CollectionEdit>,
) -> Vec<SidebarRow> {
    let mut grouped = HashMap::<_, Vec<_>>::new();
    for (index, parent) in request_parents.into_iter().enumerate() {
        grouped
            .entry(parent)
            .or_default()
            .push(SidebarRow::Request(index));
    }
    fn append(
        collections: &[Collection],
        collapsed: &HashSet<usize>,
        edit: Option<CollectionEdit>,
        parent: Option<usize>,
        grouped: &mut HashMap<Option<usize>, Vec<SidebarRow>>,
        rows: &mut Vec<SidebarRow>,
    ) {
        for (index, collection) in collections
            .iter()
            .enumerate()
            .filter(|(_, group)| group.parent_id == parent)
        {
            if matches!(edit, Some(CollectionEdit::Rename(id)) if id == collection.id) {
                rows.push(SidebarRow::Edit);
            } else {
                rows.push(SidebarRow::Collection(index));
            }
            if matches!(edit, Some(CollectionEdit::NewFolder(id)) if id == collection.id) {
                rows.push(SidebarRow::Edit);
            }
            if !collapsed.contains(&collection.id) {
                append(
                    collections,
                    collapsed,
                    edit,
                    Some(collection.id),
                    grouped,
                    rows,
                );
                rows.extend(grouped.remove(&Some(collection.id)).into_iter().flatten());
            }
        }
    }
    let mut rows = Vec::new();
    if matches!(edit, Some(CollectionEdit::New)) {
        rows.push(SidebarRow::Edit);
    }
    append(collections, collapsed, edit, None, &mut grouped, &mut rows);
    if let Some(ungrouped) = grouped.remove(&None) {
        if !collections.is_empty() && !ungrouped.is_empty() {
            rows.push(SidebarRow::Ungrouped);
        }
        rows.extend(ungrouped);
    }
    rows
}

impl Workspace {
    pub(super) fn sidebar_rows(&self) -> Vec<SidebarRow> {
        build_sidebar_rows(
            &self.collections,
            self.requests.iter().map(|entry| entry.collection_id),
            &self.collapsed_collections,
            self.collection_edit,
        )
    }

    pub(super) fn reveal_selected_request(&mut self) {
        if let Some(id) = self.active().collection_id {
            for group in crate::storage::lineage(&self.collections, id) {
                self.collapsed_collections.remove(&group.id);
            }
        }
        self.scroll_to_selected_request();
    }

    pub(super) fn scroll_to_selected_request(&self) {
        if let Some(row) = self
            .sidebar_rows()
            .iter()
            .position(|row| *row == SidebarRow::Request(self.selected))
        {
            self.sidebar_scroll.scroll_to_item(row);
        }
    }

    pub(super) fn focus_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_cursor = self.active_sidebar_target();
        self.initialize_sidebar_cursor(cx);
        self.sidebar_focus.focus(window, cx);
    }

    pub(super) fn initialize_sidebar_cursor(&mut self, cx: &mut Context<Self>) {
        let items = self.sidebar_items();
        let target = self.sidebar_cursor.or_else(|| self.active_sidebar_target());
        let item = items
            .iter()
            .find(|&&(_, item)| Some(item) == target)
            .or_else(|| items.first());
        self.sidebar_cursor = item.map(|&(_, target)| target);
        if let Some(&(index, _)) = item {
            self.sidebar_scroll.scroll_to_item(index);
        }
        cx.notify();
    }

    fn active_sidebar_target(&self) -> Option<SidebarTarget> {
        if self.request_tabs.is_empty() {
            return None;
        }
        let entry = self.active();
        let collapsed = entry.collection_id.and_then(|id| {
            crate::storage::lineage(&self.collections, id)
                .into_iter()
                .find(|group| self.collapsed_collections.contains(&group.id))
        });
        Some(collapsed.map_or(SidebarTarget::Request(entry.id), |group| {
            SidebarTarget::Collection(group.id)
        }))
    }

    fn sidebar_items(&self) -> Vec<(usize, SidebarTarget)> {
        self.sidebar_rows()
            .into_iter()
            .enumerate()
            .filter_map(|(index, row)| {
                let target = match row {
                    SidebarRow::Collection(group) => {
                        SidebarTarget::Collection(self.collections[group].id)
                    }
                    SidebarRow::Request(request) => {
                        SidebarTarget::Request(self.requests[request].id)
                    }
                    SidebarRow::Ungrouped | SidebarRow::Edit => return None,
                };
                Some((index, target))
            })
            .collect()
    }

    fn sidebar_parent(&self, target: SidebarTarget) -> Option<usize> {
        match target {
            SidebarTarget::Collection(id) => self
                .collections
                .iter()
                .find(|group| group.id == id)
                .and_then(|group| group.parent_id),
            SidebarTarget::Request(id) => self
                .request_index(id)
                .and_then(|index| self.requests[index].collection_id),
        }
    }

    pub(super) fn toggle_sidebar_group(&mut self, id: usize, cx: &mut Context<Self>) {
        if !self.collapsed_collections.remove(&id) {
            self.collapsed_collections.insert(id);
        }
        cx.notify();
    }

    pub(super) fn navigate_sidebar(
        &mut self,
        action: SidebarNavigate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sidebar_focus.is_focused(window) {
            return;
        }
        let items = self.sidebar_items();
        if items.is_empty() {
            return;
        }
        let position = items
            .iter()
            .position(|&(_, target)| self.sidebar_cursor == Some(target));
        let current = position.unwrap_or(0);
        let target = items[current].1;
        let next = match (action, target) {
            (SidebarNavigate::Up, _) => items.get(current.saturating_sub(1)),
            (SidebarNavigate::Down, _) => items.get(position.map_or(0, |position| position + 1)),
            (SidebarNavigate::First, _) => items.first(),
            (SidebarNavigate::Last, _) => items.last(),
            (SidebarNavigate::Open, SidebarTarget::Request(id)) => {
                if let Some(index) = self.request_index(id) {
                    self.select_request(index, window, cx);
                }
                items.get(current)
            }
            (SidebarNavigate::Open, SidebarTarget::Collection(id)) => {
                self.toggle_sidebar_group(id, cx);
                items.get(current)
            }
            (SidebarNavigate::Left, SidebarTarget::Collection(id))
                if !self.collapsed_collections.contains(&id) =>
            {
                self.toggle_sidebar_group(id, cx);
                items.get(current)
            }
            (SidebarNavigate::Left, _) => self.sidebar_parent(target).and_then(|id| {
                items[..current]
                    .iter()
                    .rev()
                    .find(|&&(_, target)| target == SidebarTarget::Collection(id))
            }),
            (SidebarNavigate::Right, SidebarTarget::Collection(id)) => {
                if self.collapsed_collections.contains(&id) {
                    self.toggle_sidebar_group(id, cx);
                    items.get(current)
                } else {
                    items
                        .get(current + 1)
                        .filter(|&&(_, item)| self.sidebar_parent(item) == Some(id))
                }
            }
            (SidebarNavigate::Right, SidebarTarget::Request(_)) => None,
        };
        if let Some(&(index, target)) = next {
            self.sidebar_cursor = Some(target);
            self.sidebar_scroll.scroll_to_item(index);
            cx.notify();
        }
    }

    pub(super) fn edit_collection(
        &mut self,
        edit: CollectionEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.storage.is_err() || !self.team_can_edit() {
            return;
        }
        let name = match edit {
            CollectionEdit::New | CollectionEdit::NewFolder(_) => "",
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
        }
        .to_owned();
        self.collection_edit = Some(edit);
        self.cloud.rename_version = match edit {
            CollectionEdit::Rename(id) => self.team_group_version(id),
            _ => None,
        };
        let target = match edit {
            CollectionEdit::NewFolder(id) | CollectionEdit::Rename(id) => Some(id),
            _ => None,
        };
        if let Some(id) = target {
            for group in crate::storage::lineage(&self.collections, id) {
                self.collapsed_collections.remove(&group.id);
            }
        }
        let folder = match edit {
            CollectionEdit::NewFolder(_) => true,
            CollectionEdit::Rename(id) => self
                .collections
                .iter()
                .any(|group| group.id == id && group.parent_id.is_some()),
            _ => false,
        };
        self.collection_name.update(cx, |input, cx| {
            input.set_placeholder(
                if folder {
                    "Folder name"
                } else {
                    "Collection name"
                },
                window,
                cx,
            );
            input.set_value(name.clone(), window, cx);
            input.set_selected_range(0..name.len(), cx);
            input.focus_handle(cx).focus(window, cx);
        });
        if let Some(row) = self
            .sidebar_rows()
            .iter()
            .position(|row| *row == SidebarRow::Edit)
        {
            self.sidebar_scroll.scroll_to_item(row);
        }
        cx.notify();
    }

    pub(super) fn close_collection_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.collection_edit = None;
        self.cloud.rename_version = None;
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
            CollectionEdit::New | CollectionEdit::NewFolder(_) => {
                let parent_id = match edit {
                    CollectionEdit::NewFolder(id) => Some(id),
                    _ => None,
                };
                if parent_id.is_some_and(|id| !self.collections.iter().any(|group| group.id == id))
                {
                    return;
                }
                let id = self.next_collection_id;
                let Some(next_id) = id.checked_add(1) else {
                    return;
                };
                self.collections.push(Collection {
                    parent_id,
                    id,
                    name,
                    variables: Vec::new(),
                });
                if !self.persist("Could not create collection", cx) {
                    self.collections.pop();
                    return;
                }
                self.next_collection_id = next_id;
                if let Some(parent) = parent_id {
                    for group in crate::storage::lineage(&self.collections, parent) {
                        self.collapsed_collections.remove(&group.id);
                    }
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
                if !self.persist_at_revision(
                    "Could not rename collection",
                    self.cloud.rename_version,
                    cx,
                ) {
                    self.collections[index].name = previous;
                    return;
                }
            }
        }
        self.close_collection_edit(window, cx);
        if matches!(edit, CollectionEdit::New | CollectionEdit::NewFolder(_)) {
            if let Some(row) = self
                .sidebar_rows()
                .iter()
                .position(|row| *row == SidebarRow::Collection(self.collections.len() - 1))
            {
                self.sidebar_scroll.scroll_to_item(row);
            }
        }
    }

    pub(super) fn remove_collection(
        &mut self,
        id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected_id = self.active().id;
        let previous_selection = self.selected;
        let Some(removed) = RemovedCollection::take(id, &mut self.collections, &mut self.requests)
        else {
            return;
        };
        self.selected = self
            .request_index(selected_id)
            .unwrap_or(previous_selection.min(self.requests.len().saturating_sub(1)));
        if !self.persist("Could not remove collection", cx) {
            removed.restore(&mut self.collections, &mut self.requests);
            self.selected = previous_selection;
            return;
        }
        let removed_ids = removed.ids.clone();
        drop(removed);
        self.collapsed_collections
            .retain(|id| !removed_ids.contains(id));
        if self
            .variable_editor
            .as_ref()
            .is_some_and(|editor| matches!(editor.scope, VariableScope::Collection(id) if removed_ids.contains(&id)))
        {
            self.variable_editor = None;
        }
        if matches!(self.collection_edit, Some(CollectionEdit::Rename(editing) | CollectionEdit::NewFolder(editing)) if removed_ids.contains(&editing))
        {
            self.close_collection_edit(window, cx);
        }
        self.reconcile_request_tabs();
        if self.requests.is_empty() {
            self.new_request(window, cx);
        } else {
            self.show_selection(window, cx);
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

    pub(super) fn move_folder(&mut self, id: usize, parent: usize, cx: &mut Context<Self>) {
        let Some(index) = self
            .collections
            .iter()
            .position(|group| group.id == id && group.parent_id.is_some())
        else {
            return;
        };
        if !self.collections.iter().any(|group| group.id == parent)
            || crate::storage::descendants(&self.collections, id).contains(&parent)
        {
            return;
        }
        let previous = self.collections[index].parent_id.replace(parent);
        if !self.persist("Could not move folder", cx) {
            self.collections[index].parent_id = previous;
            return;
        }
        self.collapsed_collections.remove(&parent);
        self.reveal_selected_request();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Collection, CollectionEdit, Entry, RemovedCollection, Request, SidebarRow,
        build_sidebar_rows,
    };
    use std::collections::HashSet;

    #[test]
    fn folder_creation_form_follows_its_parent_at_every_depth_and_rename_replaces_its_row() {
        let groups = vec![
            Collection {
                id: 1,
                name: "Root".into(),
                parent_id: None,
                variables: Vec::new(),
            },
            Collection {
                id: 2,
                name: "Other".into(),
                parent_id: None,
                variables: Vec::new(),
            },
            Collection {
                id: 3,
                name: "Folder".into(),
                parent_id: Some(1),
                variables: Vec::new(),
            },
            Collection {
                id: 4,
                name: "Nested".into(),
                parent_id: Some(3),
                variables: Vec::new(),
            },
        ];
        let parents = [Some(4), Some(2), None];
        for (id, index) in [(1, 0), (3, 2), (4, 3)] {
            let rows = build_sidebar_rows(
                &groups,
                parents,
                &HashSet::new(),
                Some(CollectionEdit::NewFolder(id)),
            );
            let parent = rows
                .iter()
                .position(|row| *row == SidebarRow::Collection(index))
                .unwrap();
            assert_eq!(rows[parent + 1], SidebarRow::Edit);
            assert_eq!(
                rows.iter().filter(|row| **row == SidebarRow::Edit).count(),
                1
            );
            assert_ne!(rows[0], SidebarRow::Edit);
        }
        let renamed = build_sidebar_rows(
            &groups,
            parents,
            &HashSet::new(),
            Some(CollectionEdit::Rename(3)),
        );
        assert_eq!(renamed[0], SidebarRow::Collection(0));
        assert_eq!(renamed[1], SidebarRow::Edit);
        assert_eq!(renamed[2], SidebarRow::Collection(3));
        assert!(!renamed.contains(&SidebarRow::Collection(2)));
        let root = build_sidebar_rows(&groups, parents, &HashSet::new(), Some(CollectionEdit::New));
        assert_eq!(root[0], SidebarRow::Edit);
        let collapsed = build_sidebar_rows(&groups, parents, &HashSet::from([1]), None);
        assert_eq!(
            collapsed,
            vec![
                SidebarRow::Collection(0),
                SidebarRow::Collection(1),
                SidebarRow::Request(1),
                SidebarRow::Ungrouped,
                SidebarRow::Request(2)
            ]
        );
    }

    fn fixture() -> (Vec<Collection>, Vec<Entry>) {
        let collections = [1, 2]
            .into_iter()
            .map(|id| Collection {
                parent_id: None,
                id,
                name: format!("Collection {id}"),
                variables: Vec::new(),
            })
            .collect();
        let requests = [Some(1), None, Some(1), Some(2)]
            .into_iter()
            .enumerate()
            .map(|(id, collection_id)| {
                let request = Request {
                    name: format!("Request {id}"),
                    url: format!("https://example.test/{id}"),
                    ..Request::default()
                };
                let saved = (id != 2).then(|| request.clone());
                Entry::new(id, collection_id, request, saved)
            })
            .collect();
        (collections, requests)
    }

    #[test]
    fn deleting_a_folder_removes_only_its_subtree_and_rollback_restores_everything() {
        let (mut groups, mut requests) = fixture();
        groups.extend([
            Collection {
                id: 3,
                name: "Folder".into(),
                parent_id: Some(1),
                variables: Vec::new(),
            },
            Collection {
                id: 4,
                name: "Nested".into(),
                parent_id: Some(3),
                variables: Vec::new(),
            },
            Collection {
                id: 5,
                name: "Sibling".into(),
                parent_id: Some(1),
                variables: Vec::new(),
            },
        ]);
        for (id, parent) in [(4, 3), (5, 4), (6, 5)] {
            requests.push(Entry::new(id, Some(parent), Request::default(), None));
        }
        let before = groups.clone();
        let removed = RemovedCollection::take(3, &mut groups, &mut requests).unwrap();
        assert_eq!(
            removed
                .requests
                .iter()
                .map(|(_, entry)| entry.id)
                .collect::<Vec<_>>(),
            [4, 5]
        );
        assert_eq!(
            groups.iter().map(|group| group.id).collect::<Vec<_>>(),
            [1, 2, 5]
        );
        assert!(requests.iter().any(|entry| entry.collection_id == Some(5)));
        removed.restore(&mut groups, &mut requests);
        assert_eq!(groups, before);
        assert_eq!(
            requests.iter().map(|entry| entry.id).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4, 5, 6]
        );
        let removed = RemovedCollection::take(1, &mut groups, &mut requests).unwrap();
        assert_eq!(removed.collections.len(), 4);
        assert!(
            requests
                .iter()
                .all(|entry| entry.collection_id.is_none_or(|id| id == 2))
        );
    }

    #[test]
    fn removing_populated_collection_includes_saved_requests_and_drafts() {
        let (mut collections, mut requests) = fixture();
        let removed = RemovedCollection::take(1, &mut collections, &mut requests).unwrap();
        assert_eq!(
            collections
                .iter()
                .map(|collection| collection.id)
                .collect::<Vec<_>>(),
            [2]
        );
        assert_eq!(
            requests.iter().map(|entry| entry.id).collect::<Vec<_>>(),
            [1, 3]
        );
        assert_eq!(removed.requests.len(), 2);
        assert!(removed.requests[0].1.saved.is_some());
        assert!(removed.requests[1].1.saved.is_none());
        assert!(requests.iter().all(|entry| entry.collection_id != Some(1)));
    }

    #[test]
    fn failed_collection_deletion_restores_request_data_and_original_order() {
        let (mut collections, mut requests) = fixture();
        let original: Vec<_> = requests
            .iter()
            .map(|entry| {
                (
                    entry.id,
                    entry.collection_id,
                    entry.draft.clone(),
                    entry.saved.clone(),
                )
            })
            .collect();
        let removed = RemovedCollection::take(1, &mut collections, &mut requests).unwrap();
        removed.restore(&mut collections, &mut requests);
        assert_eq!(
            collections
                .iter()
                .map(|collection| collection.id)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(
            requests
                .iter()
                .map(|entry| (
                    entry.id,
                    entry.collection_id,
                    entry.draft.clone(),
                    entry.saved.clone()
                ))
                .collect::<Vec<_>>(),
            original
        );
        assert!(RemovedCollection::take(999, &mut collections, &mut requests).is_none());
        assert_eq!(requests.len(), 4);
    }
}
