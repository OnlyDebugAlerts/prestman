use std::cell::Cell;

use gpui_kit::component::{
    WindowExt as _,
    command::{Command, CommandItem, CommandState},
};

use super::*;

pub(super) const WORKSPACE_CONTEXT: &str = "Prestman";
pub(super) const SIDEBAR_CONTEXT: &str = "RequestList";

actions!(
    prestman,
    [
        NewRequest,
        FindRequest,
        CloseRequestTab,
        NextRequestTab,
        PreviousRequestTab,
        SaveRequest,
        SendRequest,
        FocusUrl,
        FocusRequests,
        FocusBody,
        FocusResponse,
        QuitApplication
    ]
);

#[derive(Action, Clone, Copy, Debug, PartialEq)]
#[action(namespace = prestman, no_json)]
pub(super) enum SidebarNavigate {
    Up,
    Down,
    First,
    Last,
    Left,
    Right,
    Open,
}

pub fn bind_keys(cx: &mut App) {
    let workspace = Some(WORKSPACE_CONTEXT);
    let sidebar = Some(SIDEBAR_CONTEXT);
    macro_rules! primary {
        ($key:literal, $action:expr) => {
            KeyBinding::new(
                if cfg!(target_os = "macos") {
                    concat!("cmd-", $key)
                } else {
                    concat!("ctrl-", $key)
                },
                $action,
                workspace,
            )
        };
    }
    cx.bind_keys([
        primary!("n", NewRequest),
        primary!("t", NewRequest),
        primary!("k", FindRequest),
        primary!("w", CloseRequestTab),
        KeyBinding::new("ctrl-tab", NextRequestTab, workspace),
        KeyBinding::new("ctrl-shift-tab", PreviousRequestTab, workspace),
        primary!("s", SaveRequest),
        primary!("enter", SendRequest),
        primary!("l", FocusUrl),
        primary!("1", FocusRequests),
        primary!("2", FocusBody),
        primary!("3", FocusResponse),
        KeyBinding::new("up", SidebarNavigate::Up, sidebar),
        KeyBinding::new("down", SidebarNavigate::Down, sidebar),
        KeyBinding::new("home", SidebarNavigate::First, sidebar),
        KeyBinding::new("end", SidebarNavigate::Last, sidebar),
        KeyBinding::new("left", SidebarNavigate::Left, sidebar),
        KeyBinding::new("right", SidebarNavigate::Right, sidebar),
        KeyBinding::new("enter", SidebarNavigate::Open, sidebar),
        KeyBinding::new(
            "escape",
            gpui_kit::component::input::Escape,
            Some(super::cloud::CLOUD_CONTEXT),
        ),
    ]);
    #[cfg(target_os = "macos")]
    cx.bind_keys([KeyBinding::new("cmd-q", QuitApplication, None)]);
}

impl Workspace {
    pub(super) fn request_view_visible(&self) -> bool {
        !self.cloud_is_open()
            && !self.comparing_cloud()
            && self.openapi_import.is_none()
            && self.variable_editor.is_none()
    }

    pub(super) fn switch_request_tab(
        &mut self,
        backward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.request_view_visible() || self.request_tabs.len() < 2 {
            return;
        }
        let Some(position) = self
            .request_tabs
            .iter()
            .position(|&id| id == self.active().id)
        else {
            return;
        };
        let count = self.request_tabs.len();
        let offset = if backward { count - 1 } else { 1 };
        let id = self.request_tabs[(position + offset) % count];
        let Some(index) = self.request_index(id) else {
            return;
        };
        let in_sidebar = self.sidebar_focus.is_focused(window);
        let in_body = self
            .request_body_editor()
            .focus_handle(cx)
            .is_focused(window);
        let in_response = self.response_focus.contains_focused(window, cx);
        self.select_request(index, window, cx);
        let compact = Self::compact_layout(window);
        if in_sidebar {
            self.compact_sidebar_open = true;
            self.focus_sidebar(window, cx);
        } else if in_body
            && self.request_pane == RequestPane::Body
            && (!compact || self.compact_pane == CompactPane::Request)
        {
            self.focus_body(window, cx);
        } else if in_response && (!compact || self.compact_pane == CompactPane::Response) {
            self.focus_response_pane(window, cx);
        } else {
            self.focus_url(window, cx);
        }
    }

    pub(super) fn focus_body(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.request_view_visible() || self.request_tabs.is_empty() {
            return;
        }
        self.compact_sidebar_open = false;
        self.compact_pane = CompactPane::Request;
        self.request_pane = RequestPane::Body;
        self.request_body_editor()
            .focus_handle(cx)
            .focus(window, cx);
        if self.response_pane == Pane::Visualizer && Self::compact_layout(window) {
            self.sync_visualizer(window, cx);
        }
        cx.notify();
    }

    pub(super) fn focus_response(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.request_view_visible() || self.request_tabs.is_empty() {
            return;
        }
        self.compact_sidebar_open = false;
        self.compact_pane = CompactPane::Response;
        let pane = if self.active().response.is_sending() && self.active().grpc_state.streaming {
            Pane::Messages
        } else {
            Pane::Body
        };
        self.select_response_pane(pane, window, cx);
        self.focus_response_pane(window, cx);
        cx.notify();
    }

    fn focus_response_pane(&self, window: &mut Window, cx: &mut App) {
        let entry = self.active();
        let editor_visible = match &entry.response {
            RequestResponse::Finished { response, .. } => match self.response_pane {
                Pane::Body => !response.body.is_empty(),
                Pane::Headers | Pane::Messages | Pane::Trailers => true,
                _ => false,
            },
            RequestResponse::Sending { .. } => {
                entry.grpc_state.streaming
                    && matches!(self.response_pane, Pane::Messages | Pane::Headers)
            }
            _ => false,
        };
        if editor_visible {
            self.response_editor().focus_handle(cx).focus(window, cx);
        } else {
            self.response_focus.focus(window, cx);
        }
    }

    pub(super) fn open_request_finder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.request_view_visible() || window.has_active_dialog(cx) {
            return;
        }
        let (ids, items): (Vec<_>, Vec<_>) = self
            .requests
            .iter()
            .map(|entry| {
                let draft = &entry.draft;
                let mut label = format!("{}  {}", draft.method_badge(), draft.title());
                if let Some(collection) = entry.collection_id {
                    let path = crate::storage::lineage(&self.collections, collection)
                        .into_iter()
                        .map(|group| group.name.as_str())
                        .collect::<Vec<_>>()
                        .join(" / ");
                    label.push_str("  ·  ");
                    label.push_str(&path);
                }
                let item = CommandItem::new()
                    .label(label)
                    .keywords([draft.url.clone()]);
                (entry.id, item)
            })
            .unzip();
        let ids = Rc::new(ids);
        let state = cx.new(|cx| CommandState::new(window, cx));
        let initial_focus = Cell::new(true);
        let workspace = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                state.update(cx, |state, cx| state.focus(window, cx));
            }
            let confirm = workspace.clone();
            let dismiss = workspace.clone();
            let ids = ids.clone();
            dialog
                .title("Open request")
                .width(px(580.))
                .on_close(move |_, window, cx| {
                    let workspace = dismiss.clone();
                    window.defer(cx, move |window, cx| {
                        let _ = workspace.update(cx, |this, cx| this.sync_visualizer(window, cx));
                    });
                })
                .child(
                    Command::new(&state)
                        .bordered(false)
                        .placeholder("Search by name, method, URL or folder")
                        .highlight_matches(true)
                        .items(items.iter().cloned())
                        .on_confirm(move |index, window, cx| {
                            window.close_dialog(cx);
                            let _ = confirm.update(cx, |this, cx| {
                                if let Some(index) =
                                    ids.get(index.row).and_then(|&id| this.request_index(id))
                                {
                                    this.select_request(index, window, cx);
                                    this.focus_url(window, cx);
                                } else {
                                    this.sync_visualizer(window, cx);
                                }
                            });
                        }),
                )
        });
        self.sync_visualizer(window, cx);
    }
}
