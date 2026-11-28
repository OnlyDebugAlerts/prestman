mod auth;
mod autosave;
mod cloud;
mod collection_files;
mod collections;
mod curl;
mod graphql;
mod grpc;
mod history;
mod navigation;
mod pairs;
mod request_menu;
mod response_highlight;
mod response_links;
mod scripts;
mod updates;
mod url_highlight;
mod variable_hover;
mod variables;
mod view;
mod visualizer;

pub use navigation::{QuitApplication, bind_keys};
pub use view::set_theme;

use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use gpui_kit::{
    component::{
        IndexPath, Theme,
        input::{EditorState, InputEvent, InputHighlighter, InputState, TextareaState},
        select::{SelectEvent, SelectItem, SelectState},
    },
    *,
};
use reqwest::blocking::Client;

use auth::AuthFields;
use collections::{CollectionEdit, SidebarRow, SidebarTarget};
use pairs::{PairFields, PairKind, RequestPane};
use response_highlight::ResponseHighlight;
use scripts::ScriptPhase;
use variables::{VariableEditor, VariableScope};

use crate::{
    http::{self, Response},
    request::{
        METHODS, Request, format_json_body, merge_path_variables, merge_query_rows, query_rows,
        url_query,
    },
    scripts::{self as scripting, Report, Scopes},
    storage::{Collection, SidebarState, Storage, StoredRequest},
    variables::Variable,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Body,
    Messages,
    Visualizer,
    Headers,
    Trailers,
    Tests,
    Console,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum CompactPane {
    #[default]
    Request,
    Response,
}

#[derive(Clone, Copy)]
struct RequestView {
    request: RequestPane,
    response: Pane,
    compact: CompactPane,
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
        body_editor: Option<Entity<EditorState>>,
    },
    Failed(String),
}

impl RequestResponse {
    fn is_sending(&self) -> bool {
        matches!(self, Self::Sending { .. })
    }
}

struct RequestBodyEditor {
    state: Entity<EditorState>,
    _subscription: Subscription,
}

struct Entry {
    id: usize,
    collection_id: Option<usize>,
    draft: Request,
    saved: Option<Request>,
    body_editor: Option<RequestBodyEditor>,
    response: RequestResponse,
    script_report: Report,
    grpc_state: grpc::GrpcState,
    graphql_state: graphql::GraphqlState,
    view: RequestView,
    query_scroll: ScrollHandle,
    headers_scroll: ScrollHandle,
}

impl Entry {
    fn new(
        id: usize,
        collection_id: Option<usize>,
        mut draft: Request,
        mut saved: Option<Request>,
    ) -> Self {
        draft.path_variables = merge_path_variables(&draft.url, &draft.path_variables);
        if draft.query.is_empty() {
            draft.query = query_rows(&draft.url);
        }
        if let Some(saved) = &mut saved {
            saved.path_variables = merge_path_variables(&saved.url, &saved.path_variables);
            if saved.query.is_empty() {
                saved.query = query_rows(&saved.url);
            }
        }
        Self {
            id,
            collection_id,
            draft,
            saved,
            body_editor: None,
            response: RequestResponse::Idle,
            script_report: Report::default(),
            grpc_state: grpc::GrpcState::default(),
            graphql_state: graphql::GraphqlState::default(),
            view: RequestView {
                request: RequestPane::Body,
                response: Pane::Body,
                compact: CompactPane::Request,
            },
            query_scroll: ScrollHandle::new(),
            headers_scroll: ScrollHandle::new(),
        }
    }

    fn is_dirty(&self) -> bool {
        self.saved.as_ref() != Some(&self.draft)
    }

    fn replace_draft(&mut self, draft: Request) {
        if self.draft.body != draft.body || self.draft.graphql.is_some() != draft.graphql.is_some()
        {
            self.body_editor = None;
        }
        self.draft = draft;
    }

    fn stored(&self) -> Option<StoredRequest<&Request>> {
        Some(StoredRequest {
            collection_id: self.collection_id,
            request: self.saved.as_ref()?,
        })
    }
}

#[derive(Clone)]
struct MethodOption(String);

impl SelectItem for MethodOption {
    type Value = String;

    fn value(&self) -> &String {
        &self.0
    }

    fn title(&self) -> SharedString {
        self.0
            .rsplit_once('/')
            .map_or(self.0.as_str(), |(_, method)| method)
            .to_owned()
            .into()
    }
}

type MethodSelect = SelectState<Vec<MethodOption>>;

struct RequestRename {
    id: usize,
    original: String,
}

pub struct Workspace {
    storage: Result<Storage, String>,
    save_error: Option<String>,
    updates: updates::UpdateState,
    cloud: cloud::CloudState,
    client: Result<Client, String>,
    requests: Vec<Entry>,
    next_request_id: usize,
    collections: Vec<Collection>,
    global_variables: Vec<Variable>,
    environment_variables: Vec<Variable>,
    variable_editor: Option<VariableEditor>,
    variable_editor_focus: FocusHandle,
    openapi_import: Option<collection_files::OpenApiImport>,
    variable_hover: Option<variable_hover::HoverVariable>,
    next_collection_id: usize,
    collapsed_collections: HashSet<usize>,
    collection_edit: Option<CollectionEdit>,
    collection_name: Entity<InputState>,
    selected: usize,
    request_tabs: Vec<usize>,
    request_tabs_scroll: ScrollHandle,
    sidebar_width: Pixels,
    request_pane_fraction: f32,
    variables_width: Pixels,
    compact_sidebar_open: bool,
    compact_pane: CompactPane,
    last_compact_layout: bool,
    sidebar_focus: FocusHandle,
    sidebar_scroll: ScrollHandle,
    sidebar_cursor: Option<SidebarTarget>,
    history: history::HistoryPanel,
    name: Entity<InputState>,
    request_rename: Option<RequestRename>,
    url: Entity<TextareaState>,
    method: Entity<MethodSelect>,
    auth: AuthFields,
    grpc_fields: grpc::GrpcFields,
    graphql_fields: graphql::GraphqlFields,
    headers: Vec<PairFields>,
    query: Vec<PairFields>,
    path_variables: Vec<PairFields>,
    body_format_error: Option<String>,
    pre_script: Entity<EditorState>,
    post_script: Entity<EditorState>,
    on_message_script: Entity<EditorState>,
    script_phase: ScriptPhase,
    response_details: Entity<EditorState>,
    response_focus: FocusHandle,
    request_pane: RequestPane,
    response_pane: Pane,
    visualizer: Option<Entity<gpui_webview::WebView>>,
    visualizer_document: Option<(usize, String)>,
    visualizer_error: Option<String>,
    exit_saved: bool,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (storage, snapshot) = match Storage::open() {
            Ok((storage, snapshot)) => (Ok(storage), snapshot),
            Err(error) => (Err(format!("{error:#}")), Default::default()),
        };
        let history = history::HistoryPanel::new(storage.as_ref().ok(), window, cx);
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
            .map(|item| item.id)
            .chain(history.local_collection_ids())
            .max()
            .map_or(0, |id| id.saturating_add(1));
        let (request_tabs, selected) = snapshot
            .sidebar
            .restore_request_tabs(|&index| (index < requests.len()).then_some(index));
        if requests.is_empty() {
            requests.push(Entry::new(0, None, Request::default(), None));
        }
        let collection_name =
            cx.new(|cx| InputState::new(window, cx).placeholder("Collection name"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Untitled request"));
        let url = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(1)
                .soft_wrap(false)
                .submit_on_enter(true)
                .placeholder("https://api.example.com/v1")
        });
        let method = cx.new(|cx| {
            SelectState::new(
                METHODS
                    .into_iter()
                    .map(|method| MethodOption(method.into()))
                    .collect::<Vec<_>>(),
                Some(IndexPath::default()),
                window,
                cx,
            )
        });
        let auth = AuthFields::new(window, cx);
        let grpc_fields = grpc::GrpcFields::new(window, cx);
        let graphql_fields = graphql::GraphqlFields::new(window, cx);
        let response_details = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx).language("").line_number(false);
            editor.set_highlighter_factory(Rc::new(|_| None), cx);
            editor
        });
        let pre_script = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("javascript")
                .line_number(true)
                .placeholder(
                    "// Runs before sending the request\n// pm.variables.set('name', 'value');",
                )
        });
        let post_script = cx.new(|cx| EditorState::new(window, cx).language("javascript").line_number(true)
            .placeholder("// Runs after receiving the response\n// pm.test('Status is 200', () => pm.response.to.have.status(200));"));

        let on_message_script = cx.new(|cx| {
            EditorState::new(window, cx).language("javascript").line_number(true)
                .placeholder("// Runs for each message from the server\n// pm.expect(pm.message.data).to.be.an('object');")
        });

        let mut subscriptions = vec![
            cx.subscribe_in(&collection_name, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.save_collection_edit(window, cx);
                }
            }),
            cx.subscribe_in(
                &name,
                window,
                |this, state, event, window, cx| match event {
                    InputEvent::Change => {
                        if let Some(index) = this
                            .request_rename
                            .as_ref()
                            .and_then(|rename| this.request_index(rename.id))
                        {
                            this.requests[index].draft.name = state.read(cx).value().to_string();
                            cx.notify();
                        }
                    }
                    InputEvent::PressEnter { .. } => {
                        this.request_rename = None;
                        this.sidebar_focus.focus(window, cx);
                        cx.notify();
                    }
                    InputEvent::Blur => {
                        this.request_rename = None;
                        cx.notify();
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(&url, window, |this, state, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = state.read(cx).value().to_string();
                    state.update(cx, |state, cx| {
                        state.set_decorations(url_highlight::decorations(&value), cx)
                    });
                    this.edit_url(value, window, cx);
                } else if matches!(event, InputEvent::Blur) {
                    if this.active().draft.grpc.is_some() {
                        this.discover_grpc_methods_if_needed(false, window, cx);
                    } else {
                        this.sync_path_variables(window, cx);
                    }
                }
            }),
            cx.subscribe(&method, |this, _, event, cx| {
                if let SelectEvent::Confirm(Some(method)) = event {
                    this.requests[this.selected].draft.method = method.clone();
                    cx.notify();
                }
            }),
            cx.subscribe(&pre_script, |this, state, event, cx| {
                if matches!(event, InputEvent::Change) {
                    this.requests[this.selected].draft.pre_request_script =
                        state.read(cx).value().to_string();
                    cx.notify();
                }
            }),
            cx.subscribe(&post_script, |this, state, event, cx| {
                if matches!(event, InputEvent::Change) {
                    this.requests[this.selected].draft.post_response_script =
                        state.read(cx).value().to_string();
                    cx.notify();
                }
            }),
            cx.subscribe(&on_message_script, |this, state, event, cx| {
                if matches!(event, InputEvent::Change) {
                    if let Some(grpc) = &mut this.requests[this.selected].draft.grpc {
                        grpc.on_message_script = state.read(cx).value().to_string();
                        cx.notify();
                    }
                }
            }),
        ];

        // App-level dispatch also handles the native macOS menu while a WebView
        // or popup has focus. Saving here can cancel exit if persistence fails.
        let workspace_handle = cx.entity().downgrade();
        App::on_action(cx, move |_: &QuitApplication, cx| {
            if workspace_handle
                .update(cx, |this, cx| this.save_before_close(cx))
                .unwrap_or(true)
            {
                cx.quit();
            }
        });

        subscriptions.push(cx.on_app_quit(|this, cx| {
            if !this.save_before_close(cx) {
                eprintln!(
                    "{}",
                    this.save_error
                        .as_deref()
                        .unwrap_or("Could not autosave Prestman")
                );
            }
            std::future::ready(())
        }));
        // Also covers close paths that remove a GPUI window directly.
        subscriptions.push(cx.on_release(|this, cx| {
            if let Err(error) = this.save_on_exit(cx) {
                eprintln!("Could not autosave Prestman: {error:#}");
            }
        }));
        let workspace_handle = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            workspace_handle
                .update(cx, |this, cx| this.save_before_close(cx))
                .unwrap_or(true)
        });

        let sidebar_focus = cx.focus_handle().tab_stop(true);
        subscriptions.push(cx.on_focus(&sidebar_focus, window, |this, _, cx| {
            this.initialize_sidebar_cursor(cx);
        }));
        subscriptions.push(cx.on_blur(&sidebar_focus, window, |_, _, cx| cx.notify()));

        let mut workspace = Self {
            storage,
            save_error: None,
            updates: updates::UpdateState::default(),
            cloud: cloud::CloudState::default(),
            client: http::client().map_err(|error| format!("{error:#}")),
            next_request_id: requests.len(),
            requests,
            collections: snapshot.collections,
            global_variables: snapshot.global_variables,
            environment_variables: snapshot.environment_variables,
            variable_editor: None,
            variable_editor_focus: cx.focus_handle(),
            openapi_import: None,
            variable_hover: None,
            next_collection_id,
            collapsed_collections: snapshot.sidebar.collapsed_collections.into_iter().collect(),
            collection_edit: None,
            collection_name,
            selected,
            request_tabs,
            request_tabs_scroll: ScrollHandle::new(),
            sidebar_width: px(234.),
            request_pane_fraction: 0.5,
            variables_width: px(420.),
            compact_sidebar_open: false,
            compact_pane: CompactPane::Request,
            last_compact_layout: Self::compact_layout(window),
            sidebar_focus,
            sidebar_scroll: ScrollHandle::new(),
            sidebar_cursor: None,
            history,
            name,
            request_rename: None,
            url,
            method,
            auth,
            grpc_fields,
            graphql_fields,
            headers: Vec::new(),
            query: Vec::new(),
            path_variables: Vec::new(),
            body_format_error: None,
            pre_script,
            post_script,
            on_message_script,
            script_phase: ScriptPhase::Pre,
            response_details,
            response_focus: cx.focus_handle().tab_stop(true),
            request_pane: RequestPane::Body,
            response_pane: Pane::Body,
            visualizer: None,
            visualizer_document: None,
            visualizer_error: None,
            exit_saved: false,
            _subscriptions: subscriptions,
        };
        workspace.restore_selection(window, cx);
        workspace.focus_url(window, cx);
        workspace.restore_active_cloud_workspace(window, cx);
        workspace.start_update_checks(window, cx);
        workspace
    }

    fn active(&self) -> &Entry {
        &self.requests[self.selected]
    }

    fn focus_url(&self, window: &mut Window, cx: &mut App) {
        if self.request_tabs.is_empty() {
            if self.history.visible {
                self.history
                    .filter
                    .read(cx)
                    .focus_handle(cx)
                    .focus(window, cx);
            } else {
                self.sidebar_focus.focus(window, cx);
            }
        } else {
            self.url.read(cx).focus_handle(cx).focus(window, cx);
        }
    }

    fn sync_path_variables(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entry = &mut self.requests[self.selected];
        if entry.draft.grpc.is_some() {
            return;
        }
        let path_variables = merge_path_variables(&entry.draft.url, &entry.draft.path_variables);
        if path_variables != entry.draft.path_variables {
            entry.draft.path_variables = path_variables;
            self.load_pairs(PairKind::Path, window, cx);
            cx.notify();
        }
    }

    fn edit_url(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        let entry = &mut self.requests[self.selected];
        if entry.draft.grpc.is_some() {
            crate::grpc::edit_address(&mut entry.draft, url);
            entry.grpc_state.error = None;
            self.sync_grpc_methods(window, cx);
            cx.notify();
            return;
        }
        if url_query(&entry.draft.url) != url_query(&url) {
            let query = query_rows(&url);
            if query != entry.draft.query {
                if !query.is_empty() {
                    self.request_pane = RequestPane::Pairs(PairKind::Query);
                }
                // Keep disabled and unfinished rows when the URL is edited directly.
                entry.draft.query = merge_query_rows(&url, &entry.draft.query);
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
        self.remember_request_view();
        self.openapi_import = None;
        self.variable_editor = None;
        self.requests
            .push(Entry::new(self.next_request_id, collection_id, draft, None));
        self.next_request_id += 1;
        self.selected = self.requests.len() - 1;
        self.restore_request_view();
        self.compact_sidebar_open = false;
        self.open_selected_tab();
        self.show_selection(window, cx);
        self.focus_url(window, cx);
    }

    fn request_index(&self, id: usize) -> Option<usize> {
        self.requests.iter().position(|entry| entry.id == id)
    }

    fn remember_request_view(&mut self) {
        if let Some(entry) = self.requests.get_mut(self.selected) {
            entry.view = RequestView {
                request: self.request_pane,
                response: self.response_pane,
                compact: self.compact_pane,
            };
        }
    }

    fn restore_request_view(&mut self) {
        let view = self.active().view;
        self.request_pane = view.request;
        self.response_pane = view.response;
        self.compact_pane = view.compact;
    }

    fn select_request(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.dismiss_cloud_panel() {
            return false;
        }
        let from_compact_sidebar = self.compact_sidebar_open && Self::compact_layout(window);
        self.compact_sidebar_open = false;
        self.variable_hover = None;
        self.openapi_import = None;
        self.variable_editor = None;
        cx.notify();
        if index == self.selected {
            self.open_selected_tab();
            self.sync_visualizer(window, cx);
            if from_compact_sidebar {
                self.focus_url(window, cx);
            }
            return true;
        }
        self.remember_request_view();
        self.selected = index;
        self.restore_request_view();
        self.open_selected_tab();
        self.show_selection(window, cx);
        if from_compact_sidebar {
            self.focus_url(window, cx);
        }
        true
    }

    fn open_selected_tab(&mut self) {
        let id = self.active().id;
        let position = self.request_tabs.iter().position(|&tab| tab == id);
        let index = position.unwrap_or_else(|| {
            self.request_tabs.push(id);
            self.request_tabs.len() - 1
        });
        self.request_tabs_scroll.scroll_to_item(index);
    }

    fn move_request_tab(&mut self, id: usize, target: usize, after: bool, cx: &mut Context<Self>) {
        let (Some(from), Some(target)) = (
            self.request_tabs.iter().position(|&tab| tab == id),
            self.request_tabs.iter().position(|&tab| tab == target),
        ) else {
            return;
        };
        let insert = target + usize::from(after);
        let to = insert - usize::from(from < insert);
        if from == to {
            return;
        }
        self.request_tabs.remove(from);
        self.request_tabs.insert(to, id);
        self.request_tabs_scroll.scroll_to_item(to);
        cx.notify();
    }

    fn close_request_tab(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(position) = self.request_tabs.iter().position(|&tab| tab == id) else {
            return;
        };
        self.request_tabs.remove(position);
        if self.active().id == id {
            if let Some(index) = self
                .request_tabs
                .get(position.min(self.request_tabs.len().saturating_sub(1)))
                .and_then(|&id| self.request_index(id))
            {
                self.select_request(index, window, cx);
            } else {
                self.variable_hover = None;
                self.sync_visualizer(window, cx);
            }
            self.focus_url(window, cx);
        }
        cx.notify();
    }

    fn reconcile_request_tabs(&mut self) {
        if self.request_tabs.is_empty() {
            return;
        }
        let ids: HashSet<_> = self.requests.iter().map(|entry| entry.id).collect();
        self.request_tabs.retain(|id| ids.contains(id));
        if self.requests.is_empty() {
            return;
        }
        if !self.request_tabs.contains(&self.active().id) {
            if let Some(index) = self
                .request_tabs
                .last()
                .and_then(|&id| self.request_index(id))
            {
                self.selected = index;
            }
            self.open_selected_tab();
        }
    }

    fn show_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_selected_request();
        self.load_selected_editor(window, cx);
    }

    fn restore_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(position) = self
            .request_tabs
            .iter()
            .position(|&id| id == self.active().id)
        {
            self.request_tabs_scroll.scroll_to_item(position);
        }
        self.scroll_to_selected_request();
        self.load_selected_editor(window, cx);
    }

    fn load_selected_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.variable_hover = None;
        self.save_error = None;
        self.load_editor(window, cx);
        self.discover_grpc_methods_if_needed(true, window, cx);
        cx.notify();
    }

    fn load_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.body_format_error = None;
        self.load_request_body_editor(window, cx);
        let request = &self.active().draft;
        self.url.update(cx, |input, cx| {
            input.set_placeholder(
                if request.grpc.is_some() {
                    "localhost:50051"
                } else if request.graphql.is_some() {
                    "https://api.example.com/graphql"
                } else {
                    "https://api.example.com/v1"
                },
                window,
                cx,
            );
            input.set_value(request.url.clone(), window, cx);
            input.set_decorations(url_highlight::decorations(&request.url), cx);
        });
        if request.grpc.is_some() {
            if self.active().response.is_sending()
                && self.active().grpc_state.streaming
                && self.response_pane == Pane::Body
            {
                self.response_pane = Pane::Messages;
            }
            self.sync_grpc_methods(window, cx);
        } else {
            if self.script_phase == ScriptPhase::OnMessage {
                self.script_phase = ScriptPhase::Pre;
            }
            if self.request_pane == RequestPane::ServiceDefinition {
                self.request_pane = RequestPane::Body;
            }
            if matches!(self.response_pane, Pane::Trailers | Pane::Messages) {
                self.response_pane = Pane::Body;
            }
            let request = &self.active().draft;
            let mut methods = METHODS.to_vec();
            if !methods.contains(&request.method.as_str()) {
                methods.push(&request.method);
            }
            let method_index = methods.iter().position(|method| *method == request.method);
            self.method.update(cx, |select, cx| {
                select.set_items(
                    methods
                        .into_iter()
                        .map(|method| MethodOption(method.into()))
                        .collect(),
                    window,
                    cx,
                );
                select.set_selected_index(
                    method_index.map(|index| IndexPath::default().row(index)),
                    window,
                    cx,
                );
            });
        }
        if self.active().draft.grpc.is_some()
            && self.request_pane == RequestPane::Pairs(PairKind::Query)
        {
            self.request_pane = RequestPane::Body;
        }
        if self.active().draft.graphql.is_none()
            && matches!(
                self.request_pane,
                RequestPane::GraphqlVariables | RequestPane::GraphqlSchema
            )
        {
            self.request_pane = RequestPane::Body;
        }
        let request = &self.active().draft;
        self.auth.load(&request.authorization, window, cx);
        if let Some(options) = &request.grpc {
            self.grpc_fields.load(options, window, cx);
        }
        if let Some(options) = &request.graphql {
            self.graphql_fields.load(options, window, cx);
            self.sync_graphql_schema(window, cx);
        }
        self.pre_script.update(cx, |input, cx| {
            input.set_value(request.pre_request_script.clone(), window, cx)
        });
        self.post_script.update(cx, |input, cx| {
            input.set_value(request.post_response_script.clone(), window, cx)
        });
        self.on_message_script.update(cx, |input, cx| {
            input.set_value(
                request
                    .grpc
                    .as_ref()
                    .map(|grpc| grpc.on_message_script.clone())
                    .unwrap_or_default(),
                window,
                cx,
            );
        });
        self.load_pairs(PairKind::Headers, window, cx);
        self.load_pairs(PairKind::Query, window, cx);
        self.load_pairs(PairKind::Path, window, cx);
        self.load_response(window, cx);
    }

    fn load_request_body_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entry = &mut self.requests[self.selected];
        if entry.body_editor.is_some() {
            return;
        }
        let id = entry.id;
        let draft = &entry.draft;
        let language = if draft.graphql.is_some() { "" } else { "jsonc" };
        let state = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx)
                .language(language)
                .line_number(true);
            editor.set_value(draft.body.clone(), window, cx);
            editor
        });
        let subscription = cx.subscribe(&state, move |this, state, event, cx| {
            if matches!(event, InputEvent::Change)
                && let Some(index) = this.request_index(id)
            {
                this.requests[index].draft.body = state.read(cx).value().to_string();
                if this.selected == index {
                    this.body_format_error = None;
                }
                cx.notify();
            }
        });
        entry.body_editor = Some(RequestBodyEditor {
            state,
            _subscription: subscription,
        });
    }

    fn request_body_editor(&self) -> &Entity<EditorState> {
        &self
            .active()
            .body_editor
            .as_ref()
            .expect("Loaded request editor")
            .state
    }

    fn format_request_body(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = &self.requests[self.selected].draft.body;
        match format_json_body(body) {
            Ok(formatted) => {
                self.body_format_error = None;
                if formatted != *body {
                    self.request_body_editor().update(cx, |editor, cx| {
                        editor.replace_all(formatted, window, cx);
                    });
                }
            }
            Err(error) => self.body_format_error = Some(format!("Could not format JSON. {error}")),
        }
        self.request_body_editor()
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
        cx.notify();
    }

    fn load_response(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entry = &mut self.requests[self.selected];
        if self.response_pane == Pane::Body {
            if let RequestResponse::Finished {
                response,
                highlight,
                body_editor,
            } = &mut entry.response
            {
                body_editor.get_or_insert_with(|| {
                    let highlight = highlight.clone();
                    let language = if highlight.is_some() { "json" } else { "" };
                    let wrap = !response.is_json || wrap_json_response(&response.body);
                    cx.new(|cx| {
                        let mut editor = EditorState::new(window, cx)
                            .language(language)
                            .line_number(true);
                        editor.set_highlighter_factory(
                            Rc::new(move |_| {
                                highlight
                                    .clone()
                                    .map(|snapshot| Box::new(snapshot) as Box<dyn InputHighlighter>)
                            }),
                            cx,
                        );
                        // Disable wrapping before loading large JSON responses.
                        editor.set_soft_wrap(wrap, window, cx);
                        editor.set_value(SharedString::from(&response.body), window, cx);
                        editor
                    })
                });
            }
        } else {
            let text: SharedString = match (self.response_pane, &entry.response) {
                (Pane::Messages, _) => entry
                    .grpc_state
                    .messages
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join("\n\n")
                    .into(),
                (Pane::Headers, RequestResponse::Sending { .. }) => {
                    entry.grpc_state.metadata.as_str().into()
                }
                (Pane::Headers, RequestResponse::Finished { response, .. }) => {
                    response.headers.as_str().into()
                }
                (Pane::Trailers, RequestResponse::Finished { response, .. }) => {
                    response.trailers.as_str().into()
                }
                _ => SharedString::default(),
            };
            let wrap = self.response_pane != Pane::Messages;
            self.response_details.update(cx, |editor, cx| {
                if !wrap {
                    editor.set_soft_wrap(false, window, cx);
                }
                editor.set_value(text, window, cx);
                if wrap {
                    editor.set_soft_wrap(true, window, cx);
                }
            });
        }
        self.sync_visualizer(window, cx);
    }

    fn response_editor(&self) -> &Entity<EditorState> {
        if self.response_pane == Pane::Body
            && let RequestResponse::Finished {
                body_editor: Some(editor),
                ..
            } = &self.active().response
        {
            editor
        } else {
            &self.response_details
        }
    }

    fn select_response_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        if self.response_pane != pane {
            self.response_pane = pane;
            self.load_response(window, cx);
            cx.notify();
        }
    }

    fn save_request(&mut self, cx: &mut Context<Self>) {
        if self.request_tabs.is_empty() {
            return;
        }
        if self.in_team() {
            let result = self.save_team_cache(false, None);
            if !self.team_can_edit() || result.is_err() {
                self.save_error = result
                    .err()
                    .map(|error| format!("Could not save your draft. {error:#}"));
                cx.notify();
                return;
            }
        }
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
        self.persist_at_revision(message, None, cx)
    }

    fn local_sidebar(&self, include_drafts: bool) -> SidebarState<usize> {
        // Local requests are stored as an array and receive new runtime IDs on startup.
        let indexes: HashMap<_, _> = self
            .requests
            .iter()
            .filter(|entry| include_drafts || entry.saved.is_some())
            .enumerate()
            .map(|(index, entry)| (entry.id, index))
            .collect();
        SidebarState {
            collapsed_collections: self.collapsed_collections.iter().copied().collect(),
            selected_request: self
                .requests
                .get(self.selected)
                .and_then(|entry| indexes.get(&entry.id).copied()),
            request_tabs: Some(
                self.request_tabs
                    .iter()
                    .filter_map(|id| indexes.get(id).copied())
                    .collect(),
            ),
        }
    }

    fn persist_at_revision(
        &mut self,
        message: &str,
        base: Option<crate::cloud::ItemRevision>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.in_team() {
            self.save_error = self
                .save_team_cache(true, base)
                .err()
                .map(|error| format!("{message}. {error:#}"));
            cx.notify();
            return self.save_error.is_none();
        }
        let Ok(storage) = &self.storage else {
            return false;
        };
        self.save_error = storage
            .save(
                &self.collections,
                &self.global_variables,
                &self.environment_variables,
                self.requests.iter().filter_map(Entry::stored),
                self.local_sidebar(false),
            )
            .err()
            .map(|error| format!("{message}. {error:#}"));
        cx.notify();
        self.save_error.is_none()
    }

    fn variable_scopes(&self) -> Scopes {
        self.variable_scopes_for(self.active().collection_id)
    }

    fn variable_scopes_for(&self, group: Option<usize>) -> Scopes {
        if let Some(linked) = self.team_uses_local_workspace_variables() {
            return Scopes::for_cloud(linked, &self.global_variables, &self.environment_variables);
        }
        Scopes::for_request(
            &self.global_variables,
            &self.environment_variables,
            &self.collections,
            group,
        )
    }

    fn send_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.request_tabs.is_empty()
            || self.cloud_is_open()
            || self.comparing_cloud()
            || self.variable_editor.is_some()
            || self.openapi_import.is_some()
        {
            return;
        }
        if self.active().response.is_sending() {
            if self.active().draft.grpc.is_some() {
                self.send_grpc_message(cx);
            }
            return;
        }
        if self.active().draft.grpc.is_some() {
            self.invoke_grpc(window, cx);
            return;
        }
        self.sync_path_variables(window, cx);
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
        self.record_history(&request);
        let scripted = !request.pre_request_script.trim().is_empty()
            || !request.post_response_script.trim().is_empty();
        let scopes = self.variable_scopes();
        let collection_id = scopes.collection_id;
        let theme = Theme::global(cx).highlight_theme.clone();
        let task = cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_spawn(async move {
                    let execution = scripting::execute(&client, request, scopes);
                    let highlight = execution
                        .response
                        .as_ref()
                        .ok()
                        .filter(|response| response.is_json)
                        .map(|response| ResponseHighlight::json(&response.body, &*theme));
                    (execution, highlight)
                })
                .await;
            _ = view.update_in(cx, |view, window, cx| {
                let Some(index) = view.request_index(id) else {
                    return;
                };
                let (execution, highlight) = result;
                view.apply_script_mutations(collection_id, &execution.mutations, cx);
                view.requests[index].script_report = execution.report;
                view.requests[index].response = match execution.response {
                    Ok(response) => RequestResponse::Finished {
                        response,
                        highlight,
                        body_editor: None,
                    },
                    Err(error) => RequestResponse::Failed(error),
                };
                if view.selected == index {
                    if scripted
                        && (!view.requests[index].script_report.errors.is_empty()
                            || view.requests[index]
                                .script_report
                                .tests
                                .iter()
                                .any(|test| !test.passed))
                    {
                        view.response_pane = Pane::Tests;
                    }
                    view.load_response(window, cx);
                }
                cx.notify();
            });
        });
        self.requests[self.selected].response = RequestResponse::Sending { _task: task };
        self.requests[self.selected].script_report = Report::default();
        self.visualizer_document = None;
        self.sync_visualizer(window, cx);
        cx.notify();
    }
}
