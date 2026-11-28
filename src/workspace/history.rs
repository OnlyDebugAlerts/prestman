use std::{
    fs::File,
    io::{self, BufReader},
    path::PathBuf,
};

use anyhow::{Context as _, Result};
use chrono::{DateTime, Days, Local, NaiveDate, Utc};
use gpui_kit::component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    input::{Enter, Input},
    menu::{ContextMenuExt, DropdownMenu, PopupMenu},
};
use gpui_kit::prelude::FluentBuilder as _;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    request_menu,
    view::{BORDER, FAINT, MUTED, RED, SURFACE, column, method_color, row},
    *,
};
use crate::storage::{lineage, write_json};

const MAX_ENTRIES: usize = 200;

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CollectionRef {
    Local(usize),
    Team(Uuid),
}

#[derive(Deserialize, Serialize)]
struct HistoryEntry {
    id: Uuid,
    sent_at: i64,
    collection: Option<CollectionRef>,
    request: Request,
}

impl HistoryEntry {
    fn matches(&self, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        let request = &self.request;
        let searchable = format!(
            "{} {} {} {} {}",
            request.method_badge(),
            request.method,
            request.name,
            request.url,
            request
                .graphql
                .as_ref()
                .map_or("", |options| &options.operation_name),
        )
        .to_lowercase();
        query
            .split_whitespace()
            .all(|term| searchable.contains(term))
    }
}

struct HistoryLog {
    path: Option<PathBuf>,
    entries: Vec<HistoryEntry>,
    load_failed: bool,
    error: Option<String>,
}

impl HistoryLog {
    fn load(path: Option<PathBuf>) -> Self {
        let mut log = Self {
            path,
            entries: Vec::new(),
            load_failed: false,
            error: None,
        };
        if let Some(path) = &log.path {
            let loaded = (|| -> Result<Vec<HistoryEntry>> {
                let file = match File::open(path) {
                    Ok(file) => file,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
                    Err(error) => return Err(error.into()),
                };
                Ok(serde_json::from_reader(BufReader::new(file))?)
            })();
            match loaded {
                Ok(entries) => {
                    log.entries = entries;
                    log.trim();
                }
                Err(error) => {
                    log.load_failed = true;
                    log.error = Some(format!("Could not read history. {error:#}"));
                }
            }
        } else {
            log.error = Some("History is available for this session only.".into());
        }
        log
    }

    fn trim(&mut self) {
        let excess = self.entries.len().saturating_sub(MAX_ENTRIES);
        self.entries.drain(..excess);
    }

    fn record(&mut self, request: Request, collection: Option<CollectionRef>) {
        self.entries.push(HistoryEntry {
            id: Uuid::new_v4(),
            sent_at: Utc::now().timestamp_millis(),
            collection,
            request,
        });
        self.trim();
        self.save();
    }

    fn save(&mut self) -> bool {
        if self.load_failed {
            return true;
        }
        self.write(false)
    }

    fn write(&mut self, clear: bool) -> bool {
        let Some(path) = &self.path else {
            return true;
        };
        let entries = if clear {
            &[][..]
        } else {
            self.entries.as_slice()
        };
        let result = write_json(path, &entries).context("Could not save history");
        self.error = result.err().map(|error| format!("{error:#}"));
        self.error.is_none()
    }

    fn remove(&mut self, id: Uuid) {
        if let Some(index) = self.entries.iter().position(|entry| entry.id == id) {
            let entry = self.entries.remove(index);
            if !self.save() {
                self.entries.insert(index, entry);
            }
        }
    }

    fn clear(&mut self) {
        if self.write(true) {
            self.entries.clear();
            self.load_failed = false;
        }
    }
}

pub(super) struct HistoryPanel {
    log: HistoryLog,
    pub(super) visible: bool,
    pub(super) filter: Entity<InputState>,
    scroll: ScrollHandle,
    confirm_clear: bool,
    _subscription: Subscription,
}

impl HistoryPanel {
    pub(super) fn new(
        storage: Option<&Storage>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Search history"));
        let subscription = cx.subscribe(&filter, |_, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        Self {
            log: HistoryLog::load(storage.map(Storage::history_path)),
            visible: false,
            filter,
            scroll: ScrollHandle::new(),
            confirm_clear: false,
            _subscription: subscription,
        }
    }

    pub(super) fn local_collection_ids(&self) -> impl Iterator<Item = usize> + '_ {
        self.log
            .entries
            .iter()
            .filter_map(|entry| match entry.collection {
                Some(CollectionRef::Local(id)) => Some(id),
                _ => None,
            })
    }

    fn matching(&self, cx: &App) -> impl Iterator<Item = &HistoryEntry> {
        let query = self.filter.read(cx).value().trim().to_lowercase();
        self.log
            .entries
            .iter()
            .rev()
            .filter(move |entry| entry.matches(&query))
    }
}

impl Workspace {
    pub(super) fn load_workspace_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self
            .team_history_path()
            .or_else(|| self.storage.as_ref().ok().map(Storage::history_path));
        self.history.log = HistoryLog::load(path);
        self.history
            .filter
            .update(cx, |filter, cx| filter.set_value("", window, cx));
        self.history.scroll = ScrollHandle::new();
        self.history.confirm_clear = false;
    }

    pub(super) fn record_history(&mut self, request: &Request) {
        let collection = self.active().collection_id.and_then(|id| {
            if self.in_team() {
                self.team_history_group(id).map(CollectionRef::Team)
            } else {
                Some(CollectionRef::Local(id))
            }
        });
        self.history.log.record(request.clone(), collection);
    }

    pub(super) fn show_history(
        &mut self,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.history.visible = visible;
        self.history.confirm_clear = false;
        if visible {
            self.history
                .filter
                .read(cx)
                .focus_handle(cx)
                .focus(window, cx);
        } else {
            self.focus_sidebar(window, cx);
        }
        cx.notify();
    }

    fn open_history_request(
        &mut self,
        id: Uuid,
        destination: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.history.log.entries.iter().find(|entry| entry.id == id) else {
            return;
        };
        let source = match entry.collection {
            Some(CollectionRef::Local(id)) => Some(id),
            Some(CollectionRef::Team(id)) => self.resolve_team_history_group(id),
            None => None,
        };
        let collection = destination.or(source).filter(|&id| {
            self.collections
                .iter()
                .any(|collection| collection.id == id)
        });
        if destination.is_some() && (destination != collection || !self.team_can_edit()) {
            return;
        }
        self.add_request(entry.request.clone(), collection, window, cx);
        if destination.is_some() {
            self.save_request(cx);
        }
    }

    pub(super) fn history_sidebar(&self, cx: &Context<Self>) -> Div {
        let today = Local::now().date_naive();
        let mut day = None;
        let mut rows = Vec::new();
        for entry in self.history.matching(cx) {
            let sent_at = DateTime::from_timestamp_millis(entry.sent_at)
                .map(|date| date.with_timezone(&Local));
            let date = sent_at.as_ref().map(DateTime::date_naive);
            if day != Some(date) {
                day = Some(date);
                rows.push(
                    div()
                        .px_2()
                        .pt_2()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(MUTED))
                        .child(history_day(date, today))
                        .into_any_element(),
                );
            }
            let time = sent_at
                .map(|date| date.format("%H:%M").to_string())
                .unwrap_or_default();
            rows.push(self.history_row(entry, time, cx).into_any_element());
        }
        let empty = if self.history.log.entries.is_empty() {
            "Sent requests will appear here."
        } else {
            "No matching requests."
        };
        column()
            .flex_1()
            .min_h_0()
            .child(
                row()
                    .px_2()
                    .pb_1()
                    .gap_1()
                    .child(
                        div()
                            .id("history-filter")
                            .flex_1()
                            .min_w_0()
                            .capture_action(cx.listener(|this, _: &Enter, window, cx| {
                                cx.stop_propagation();
                                let id = this.history.matching(cx).next().map(|entry| entry.id);
                                if let Some(id) = id {
                                    this.open_history_request(id, None, window, cx);
                                }
                            }))
                            .child(
                                Input::new(&self.history.filter)
                                    .small()
                                    .aria_label("Search history"),
                            ),
                    )
                    .child(
                        Button::new("history-clear")
                            .ghost()
                            .small()
                            .label("Clear")
                            .disabled(
                                self.history.log.entries.is_empty()
                                    && !self.history.log.load_failed,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.history.confirm_clear = true;
                                cx.notify();
                            })),
                    ),
            )
            .when(self.history.confirm_clear, |panel| {
                panel.child(
                    column()
                        .mx_2()
                        .my_1()
                        .p_2()
                        .gap_1()
                        .rounded_md()
                        .bg(rgb(SURFACE))
                        .child("Clear all request history?")
                        .child(
                            row()
                                .justify_end()
                                .gap_1()
                                .child(
                                    Button::new("history-cancel")
                                        .ghost()
                                        .small()
                                        .label("Cancel")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.history.confirm_clear = false;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("history-confirm")
                                        .danger()
                                        .small()
                                        .label("Clear all")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.history.log.clear();
                                            this.history.confirm_clear = false;
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
            })
            .children(self.history.log.error.as_ref().map(|error| {
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(RED))
                    .child(error.clone())
            }))
            .child(
                column()
                    .id("history-entries")
                    .flex_1()
                    .min_h_0()
                    .mx_1()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .overflow_y_scroll()
                    .track_scroll(&self.history.scroll)
                    .when(rows.is_empty(), |list| {
                        list.child(div().p_2().text_color(rgb(MUTED)).child(empty))
                    })
                    .children(rows),
            )
    }

    fn history_row(
        &self,
        entry: &HistoryEntry,
        time: String,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let id = entry.id;
        let method = entry.request.method_badge();
        let title = entry.request.title();
        let url = entry.request.url.trim();
        let workspace = cx.entity().downgrade();
        let menu_workspace = workspace.clone();
        row()
            .id(SharedString::from(format!("history-row-{id}")))
            .rounded_md()
            .px_1()
            .min_w_0()
            .flex_shrink_0()
            .hover(|style| style.bg(rgb(SURFACE)))
            .child(
                column()
                    .id("open")
                    .cursor_pointer()
                    .min_w_0()
                    .flex_1()
                    .p_1()
                    .role(Role::Button)
                    .aria_label(format!("Open {method} {title}"))
                    .child(
                        row()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(method_color(method)))
                                    .child(method.to_owned()),
                            )
                            .child(div().text_xs().text_color(rgb(FAINT)).child(time)),
                    )
                    .child(div().min_w_0().truncate().child(title.to_owned()))
                    .when(!url.is_empty() && url != title, |item| {
                        item.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .child(url.to_owned()),
                        )
                    })
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if event.click_count() == 1 {
                            this.open_history_request(id, None, window, cx);
                        }
                    })),
            )
            .child(
                Button::new("actions")
                    .ghost()
                    .small()
                    .compact()
                    .label("⋯")
                    .accessibility_label("History actions")
                    .tooltip("History actions")
                    .dropdown_caret(false)
                    .dropdown_menu(move |menu, window, cx| {
                        history_menu(menu, menu_workspace.clone(), id, window, cx)
                    }),
            )
            .context_menu(move |menu, window, cx| {
                history_menu(menu, workspace.clone(), id, window, cx)
            })
    }
}

fn history_day(date: Option<NaiveDate>, today: NaiveDate) -> String {
    match date {
        Some(date) if date == today => "Today".into(),
        Some(date) if today.checked_sub_days(Days::new(1)) == Some(date) => "Yesterday".into(),
        Some(date) => date.format("%d %b %Y").to_string(),
        None => "Unknown date".into(),
    }
}

fn history_menu(
    mut menu: PopupMenu,
    workspace: WeakEntity<Workspace>,
    id: Uuid,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(view) = workspace.upgrade() else {
        return menu;
    };
    let view = view.read(cx);
    if view.storage.is_ok() && view.team_can_edit() && !view.collections.is_empty() {
        let destinations = view
            .collections
            .iter()
            .map(|collection| {
                let name = lineage(&view.collections, collection.id)
                    .iter()
                    .map(|group| group.name.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ");
                (collection.id, name)
            })
            .collect::<Vec<_>>();
        let workspace = workspace.clone();
        menu = menu.submenu("Save to collection", window, cx, move |menu, _, _| {
            destinations.iter().fold(menu, |menu, (destination, name)| {
                let destination = *destination;
                menu.item(request_menu::item(
                    name.as_str(),
                    &workspace,
                    false,
                    move |view, window, cx| {
                        view.open_history_request(id, Some(destination), window, cx);
                    },
                ))
            })
        });
    }
    menu.item(request_menu::item(
        "Delete from history",
        &workspace,
        false,
        move |view, _, cx| {
            view.history.log.remove(id);
            cx.notify();
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::{CollectionRef, HistoryLog, MAX_ENTRIES, Request, Result, Uuid};
    use crate::request::{Authorization, GraphqlOptions, GrpcOptions, Pair};

    #[test]
    fn history_keeps_independent_snapshots_and_deletions_across_reopens() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let local = directory.path().join("local.json");
        let team = directory.path().join("team.json");
        let mut local_log = HistoryLog::load(Some(local.clone()));
        let mut team_log = HistoryLog::load(Some(team.clone()));
        let mut request = Request {
            name: "Create item".into(),
            method: "POST".into(),
            url: "{{base_url}}/items/:id".into(),
            body: r#"{"value": "{{item_value}}"}"#.into(),
            authorization: Authorization::Bearer {
                token: "{{token}}".into(),
            },
            path_variables: vec![Pair {
                name: "id".into(),
                value: "next".into(),
                enabled: true,
            }],
            pre_request_script: "pm.variables.set('item_value', 'test');".into(),
            ..Request::default()
        };
        let original = request.clone();
        local_log.record(request.clone(), Some(CollectionRef::Local(12)));
        request.body.clear();
        local_log.record(request, None);
        let team_id = Uuid::new_v4();
        let grpc = Request {
            url: "localhost:50051".into(),
            method: "example.Items/Create".into(),
            grpc: Some(GrpcOptions::default()),
            ..Request::default()
        };
        team_log.record(grpc.clone(), Some(CollectionRef::Team(team_id)));

        let mut local_log = HistoryLog::load(Some(local.clone()));
        assert_eq!(local_log.entries.len(), 2);
        let entry = &local_log.entries[0];
        assert_eq!(entry.request, original);
        assert!(matches!(entry.collection, Some(CollectionRef::Local(12))));
        local_log.remove(entry.id);
        let mut local_log = HistoryLog::load(Some(local.clone()));
        assert_eq!(local_log.entries.len(), 1);
        assert!(local_log.entries[0].request.body.is_empty());
        local_log.clear();
        assert!(HistoryLog::load(Some(local)).entries.is_empty());
        let team_log = HistoryLog::load(Some(team));
        assert_eq!(team_log.entries.len(), 1);
        assert_eq!(team_log.entries[0].request, grpc);
        assert!(
            matches!(team_log.entries[0].collection, Some(CollectionRef::Team(id)) if id == team_id)
        );
        Ok(())
    }

    #[test]
    fn history_filters_and_retains_the_most_recent_attempts() {
        let mut log = HistoryLog::load(None);
        for index in 0..=MAX_ENTRIES {
            log.record(
                Request {
                    name: format!("Request {index}"),
                    url: "https://example.test/graphql".into(),
                    graphql: Some(GraphqlOptions {
                        operation_name: "ListItems".into(),
                        ..GraphqlOptions::default()
                    }),
                    ..Request::default()
                },
                None,
            );
        }
        assert_eq!(log.entries.len(), MAX_ENTRIES);
        assert_eq!(log.entries[0].request.name, "Request 1");
        let latest = log.entries.last().unwrap();
        assert_eq!(latest.request.name, format!("Request {MAX_ENTRIES}"));
        assert!(latest.matches("gql example.test listitems"));
        assert!(!latest.matches("gql unrelated"));
    }

    #[test]
    fn unreadable_history_survives_sends_until_explicitly_cleared() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("history.json");
        std::fs::write(&path, "invalid")?;
        let mut log = HistoryLog::load(Some(path.clone()));
        assert!(log.error.is_some());
        log.record(Request::default(), None);
        assert_eq!(std::fs::read_to_string(&path)?, "invalid");
        assert_eq!(log.entries.len(), 1);
        log.clear();
        let log = HistoryLog::load(Some(path));
        assert!(log.error.is_none());
        assert!(log.entries.is_empty());
        Ok(())
    }
}
