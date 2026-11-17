mod auth;
mod collections;
mod pairs;
mod request_menu;
mod response_highlight;
mod view;

pub use view::set_theme;

use std::{collections::HashSet, rc::Rc};

use gpui_kit::{
    component::{
        IndexPath, Theme,
        input::{EditorState, InputEvent, InputHighlighter, InputState},
        select::{SelectEvent, SelectState},
    },
    *,
};
use reqwest::blocking::Client;

use auth::AuthFields;
use collections::{CollectionEdit, SidebarRow};
use pairs::{PairFields, PairKind, RequestPane};
use response_highlight::ResponseHighlight;

use crate::{
    http::{self, Response},
    request::{METHODS, Pair, Request, format_json_body, query_rows, url_query},
    storage::{Collection, Storage, StoredRequest},
};

actions!(
    prestman,
    [
        NewRequest,
        SaveRequest,
        SendRequest,
        FocusUrl,
        FocusRequests,
        PreviousRequest,
        NextRequest
    ]
);

pub fn bind_keys(cx: &mut App) {
    let [new, save, send, url, list] = if cfg!(target_os = "macos") {
        ["cmd-n", "cmd-s", "cmd-enter", "cmd-l", "cmd-1"]
    } else {
        ["ctrl-n", "ctrl-s", "ctrl-enter", "ctrl-l", "ctrl-1"]
    };
    let workspace = Some("Prestman");
    cx.bind_keys([
        KeyBinding::new(new, NewRequest, workspace),
        KeyBinding::new(save, SaveRequest, workspace),
        KeyBinding::new(send, SendRequest, workspace),
        KeyBinding::new(url, FocusUrl, workspace),
        KeyBinding::new(list, FocusRequests, workspace),
        KeyBinding::new("up", PreviousRequest, Some("RequestList")),
        KeyBinding::new("down", NextRequest, Some("RequestList")),
    ]);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Body,
    Headers,
}

fn wrap_json_response(text: &str) -> bool {
    const MAX_WRAPPED_BYTES: usize = 256 * 1024;
    const MAX_WRAPPED_LINE_BYTES: usize = 8 * 1024;

    text.len() <= MAX_WRAPPED_BYTES
        && text
            .lines()
            .all(|line| line.len() <= MAX_WRAPPED_LINE_BYTES)
}

#[derive(Default)]
enum RequestResponse {
    #[default]
    Idle,
    Sending {
        _task: Task<()>,
    },
    Finished {
        response: Response,
        highlight: Option<ResponseHighlight>,
    },
    Failed(String),
}

impl RequestResponse {
    fn is_sending(&self) -> bool {
        matches!(self, Self::Sending { .. })
    }
}

struct Entry {
    id: usize,
    collection_id: Option<usize>,
    draft: Request,
    query: Vec<Pair>,
    saved: Option<Request>,
    response: RequestResponse,
}

impl Entry {
    fn new(
        id: usize,
        collection_id: Option<usize>,
        draft: Request,
        saved: Option<Request>,
    ) -> Self {
        Self {
            id,
            collection_id,
            query: query_rows(&draft.url),
            draft,
            saved,
            response: RequestResponse::Idle,
        }
    }

    fn is_dirty(&self) -> bool {
        self.saved.as_ref() != Some(&self.draft)
    }

    fn stored(&self) -> Option<StoredRequest<&Request>> {
        Some(StoredRequest {
            collection_id: self.collection_id,
            request: self.saved.as_ref()?,
        })
    }
}

type MethodSelect = SelectState<Vec<&'static str>>;

pub struct Workspace {
    storage: Result<Storage, String>,
    save_error: Option<String>,
    client: Result<Client, String>,
    requests: Vec<Entry>,
    next_request_id: usize,
    collections: Vec<Collection>,
    next_collection_id: usize,
    collapsed_collections: HashSet<usize>,
    collection_edit: Option<CollectionEdit>,
    collection_name: Entity<InputState>,
    selected: usize,
    sidebar_focus: FocusHandle,
    sidebar_scroll: ScrollHandle,
    name: Entity<InputState>,
    url: Entity<InputState>,
    method: Entity<MethodSelect>,
    auth: AuthFields,
    headers: Vec<PairFields>,
    query: Vec<PairFields>,
    body: Entity<EditorState>,
    body_format_error: Option<String>,
    response: Entity<EditorState>,
    request_pane: RequestPane,
    response_pane: Pane,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (storage, snapshot) = match Storage::open() {
            Ok((storage, snapshot)) => (Ok(storage), snapshot),
            Err(error) => (Err(format!("{error:#}")), Default::default()),
        };
        let mut requests = snapshot
            .requests
            .into_iter()
            .enumerate()
            .map(|(id, item)| {
                Entry::new(
                    id,
                    item.collection_id,
                    item.request.clone(),
                    Some(item.request),
                )
            })
            .collect::<Vec<_>>();
        let next_collection_id = snapshot
            .collections
            .iter()
            .map(|item| item.id + 1)
            .max()
            .unwrap_or_default();
        if requests.is_empty() {
            requests.push(Entry::new(0, None, Request::default(), None));
        }
        let collection_name =
            cx.new(|cx| InputState::new(window, cx).placeholder("Collection name"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Untitled request"));
        let url =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://api.example.com/v1"));
        let method =
            cx.new(|cx| SelectState::new(METHODS.to_vec(), Some(IndexPath::default()), window, cx));
        let auth = AuthFields::new(window, cx);
        let body = cx.new(|cx| {
            EditorState::new(window, cx)
                .line_number(true)
                .placeholder("Enter a request body")
        });
        let response = cx.new(|cx| EditorState::new(window, cx).line_number(true));

        let subscriptions = vec![
            cx.subscribe_in(&collection_name, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.save_collection_edit(window, cx);
                }
            }),
            cx.subscribe(&name, |this, state, event, cx| {
                if matches!(event, InputEvent::Change) {
                    this.requests[this.selected].draft.name = state.read(cx).value().to_string();
                    cx.notify();
                }
            }),
            cx.subscribe_in(&url, window, |this, state, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.edit_url(state.read(cx).value().to_string(), window, cx);
                }
            }),
            cx.subscribe(&method, |this, _, event, cx| {
                if let SelectEvent::Confirm(Some(method)) = event {
                    this.requests[this.selected].draft.method = (*method).into();
                    cx.notify();
                }
            }),
            cx.subscribe(&body, |this, state, event, cx| {
                if matches!(event, InputEvent::Change) {
                    this.requests[this.selected].draft.body = state.read(cx).value().to_string();
                    this.body_format_error = None;
                    cx.notify();
                }
            }),
        ];

        let mut workspace = Self {
            storage,
            save_error: None,
            client: http::client().map_err(|error| format!("{error:#}")),
            next_request_id: requests.len(),
            requests,
            collections: snapshot.collections,
            next_collection_id,
            collapsed_collections: HashSet::new(),
            collection_edit: None,
            collection_name,
            selected: 0,
            sidebar_focus: cx.focus_handle().tab_stop(true),
            sidebar_scroll: ScrollHandle::new(),
            name,
            url,
            method,
            auth,
            headers: Vec::new(),
            query: Vec::new(),
            body,
            body_format_error: None,
            response,
            request_pane: RequestPane::Body,
            response_pane: Pane::Body,
            _subscriptions: subscriptions,
        };
        workspace.reveal_selected_request();
        workspace.load_editor(window, cx);
        workspace.focus_url(window, cx);
        workspace
    }

    fn active(&self) -> &Entry {
        &self.requests[self.selected]
    }

    fn focus_url(&self, window: &mut Window, cx: &mut App) {
        self.url.read(cx).focus_handle(cx).focus(window, cx);
    }

    fn edit_url(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        let entry = &mut self.requests[self.selected];
        if url_query(&entry.draft.url) != url_query(&url) {
            let query = query_rows(&url);
            if query != entry.query {
                if !query.is_empty() {
                    self.request_pane = RequestPane::Pairs(PairKind::Query);
                }
                entry.query = query;
                self.load_pairs(PairKind::Query, window, cx);
            }
        }
        self.requests[self.selected].draft.url = url;
        cx.notify();
    }

    fn new_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.add_request(Request::default(), None, window, cx);
    }

    fn add_request(
        &mut self,
        draft: Request,
        collection_id: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.requests
            .push(Entry::new(self.next_request_id, collection_id, draft, None));
        self.next_request_id += 1;
        self.request_pane = RequestPane::Body;
        self.selected = self.requests.len() - 1;
        self.show_selection(window, cx);
        self.focus_url(window, cx);
    }

    fn request_index(&self, id: usize) -> Option<usize> {
        self.requests.iter().position(|entry| entry.id == id)
    }

    fn select_request(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index == self.selected {
            return;
        }
        self.selected = index;
        self.show_selection(window, cx);
    }

    fn show_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_error = None;
        self.reveal_selected_request();
        self.load_editor(window, cx);
        cx.notify();
    }

    fn load_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.body_format_error = None;
        let request = &self.active().draft;
        self.name.update(cx, |input, cx| {
            input.set_value(request.name.clone(), window, cx)
        });
        self.url.update(cx, |input, cx| {
            input.set_value(request.url.clone(), window, cx)
        });
        let method_index = METHODS.iter().position(|method| *method == request.method);
        self.method.update(cx, |select, cx| {
            select.set_selected_index(
                method_index.map(|index| IndexPath::default().row(index)),
                window,
                cx,
            );
        });
        self.auth.load(&request.authorization, window, cx);
        self.body.update(cx, |input, cx| {
            input.set_value(request.body.clone(), window, cx)
        });
        self.load_pairs(PairKind::Headers, window, cx);
        self.load_pairs(PairKind::Query, window, cx);
        self.load_response(window, cx);
    }

    fn format_request_body(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = &self.requests[self.selected].draft.body;
        match format_json_body(body) {
            Ok(formatted) => {
                self.body_format_error = None;
                if formatted != *body {
                    self.body.update(cx, |editor, cx| {
                        editor.replace_all(formatted, window, cx);
                    });
                }
            }
            Err(error) => self.body_format_error = Some(format!("Could not format JSON. {error}")),
        }
        self.body.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn load_response(&self, window: &mut Window, cx: &mut Context<Self>) {
        let (text, highlight, wrap) = match &self.active().response {
            RequestResponse::Finished {
                response,
                highlight,
            } => match self.response_pane {
                Pane::Body => (
                    SharedString::from(&response.body),
                    highlight.clone(),
                    !response.is_json || wrap_json_response(&response.body),
                ),
                Pane::Headers => (response.headers.as_str().into(), None, true),
            },
            _ => (SharedString::default(), None, true),
        };
        let language = if highlight.is_some() { "json" } else { "" };
        self.response.update(cx, |editor, cx| {
            if editor.language_name().as_ref() != language {
                editor.set_highlighter(language, cx);
            }
            editor.set_highlighter_factory(
                Rc::new(move |_| {
                    highlight
                        .clone()
                        .map(|snapshot| Box::new(snapshot) as Box<dyn InputHighlighter>)
                }),
                cx,
            );
            editor.set_line_number(self.response_pane == Pane::Body, window, cx);

            // Set the mode on the small document so GPUI never wraps the large one.
            if !wrap {
                editor.set_soft_wrap(false, window, cx);
            }
            editor.set_value(text, window, cx);
            if wrap {
                editor.set_soft_wrap(true, window, cx);
            }
        });
    }

    fn select_response_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        if self.response_pane != pane {
            self.response_pane = pane;
            self.load_response(window, cx);
            cx.notify();
        }
    }

    fn save_request(&mut self, cx: &mut Context<Self>) {
        if !self.active().is_dirty() {
            return;
        }
        let entry = &mut self.requests[self.selected];
        let previous = entry.saved.replace(entry.draft.clone());
        if !self.persist("Could not save request", cx) {
            self.requests[self.selected].saved = previous;
        }
    }

    fn persist(&mut self, message: &str, cx: &mut Context<Self>) -> bool {
        let Ok(storage) = &self.storage else {
            return false;
        };
        self.save_error = storage
            .save(
                &self.collections,
                self.requests.iter().filter_map(Entry::stored),
            )
            .err()
            .map(|error| format!("{message}. {error:#}"));
        cx.notify();
        self.save_error.is_none()
    }

    fn send_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active().response.is_sending() {
            return;
        }
        let client = match &self.client {
            Ok(client) => client.clone(),
            Err(error) => {
                self.requests[self.selected].response = RequestResponse::Failed(error.clone());
                cx.notify();
                return;
            }
        };
        let id = self.active().id;
        let request = self.active().draft.clone();
        let theme = Theme::global(cx).highlight_theme.clone();
        let task = cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_spawn(async move {
                    let response = http::send(&client, request)?;
                    let highlight = response
                        .is_json
                        .then(|| ResponseHighlight::json(&response.body, &*theme));
                    Ok::<_, anyhow::Error>((response, highlight))
                })
                .await;
            _ = view.update_in(cx, |view, window, cx| {
                let Some(index) = view.request_index(id) else {
                    return;
                };
                view.requests[index].response = match result {
                    Ok((response, highlight)) => RequestResponse::Finished {
                        response,
                        highlight,
                    },
                    Err(error) => RequestResponse::Failed(format!("{error:#}")),
                };
                if view.selected == index {
                    view.load_response(window, cx);
                }
                cx.notify();
            });
        });
        self.requests[self.selected].response = RequestResponse::Sending { _task: task };
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::wrap_json_response;

    #[test]
    fn large_json_documents_and_long_lines_use_horizontal_scrolling() {
        let row = "  {\"name\": \"тест\"},\n";
        assert!(wrap_json_response(&row.repeat(100)));
        assert!(!wrap_json_response(&row.repeat(20_000)));

        let long_line = format!("{{\n  \"value\": \"{}\"\n}}", "я".repeat(4096));
        assert!(!wrap_json_response(&long_line));
    }
}
