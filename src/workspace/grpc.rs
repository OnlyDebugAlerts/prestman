use std::{collections::VecDeque, path::PathBuf, thread};

use anyhow::{Context as _, Result};
use futures::{
    StreamExt as _,
    channel::{mpsc, oneshot},
};
use prost_reflect::DescriptorPool;

use super::*;
use crate::{
    grpc,
    request::{GrpcOptions, GrpcProto},
    variables::resolve_request,
};

#[derive(Default)]
pub(super) struct GrpcState {
    schema: Option<(Request, DescriptorPool)>,
    pub(super) loading: bool,
    pub(super) error: Option<String>,
    pub(super) run: Option<ActiveCall>,
    pub(super) streaming: bool,
    pub(super) metadata: String,
    pub(super) messages: VecDeque<String>,
    message_bytes: usize,
}

pub(super) struct ActiveCall {
    pub(super) sender: Option<tokio::sync::mpsc::Sender<String>>,
    pub(super) can_send: bool,
    cancel: Option<oneshot::Sender<()>>,
}

impl GrpcState {
    pub(super) fn current_schema(&self, request: &Request) -> Option<&DescriptorPool> {
        let (target, pool) = self.schema.as_ref()?;
        let options = request.grpc.as_ref()?;
        if let Some(proto) = &options.proto {
            return (target.grpc.as_ref()?.proto.as_ref() == Some(proto)).then_some(pool);
        }
        (schema_target(request) == *target).then_some(pool)
    }

    fn record(&mut self, direction: &str, body: String) {
        const MAX_BYTES: usize = 2 * 1024 * 1024;
        if body.len() > MAX_BYTES {
            return;
        }
        let record = format!("{direction}\n{body}");
        self.message_bytes += record.len();
        self.messages.push_back(record);
        while self.message_bytes > MAX_BYTES || self.messages.len() > 200 {
            self.message_bytes -= self.messages.pop_front().expect("message was added").len();
        }
    }
}

pub(super) struct GrpcFields {
    pub(super) timeout: Entity<InputState>,
    pub(super) max_response_bytes: Entity<InputState>,
    pub(super) server_name: Entity<InputState>,
    _subscriptions: [Subscription; 3],
}

impl GrpcFields {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let timeout = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("0 = no deadline")
                .validate(|value, _| value.is_empty() || value.parse::<u64>().is_ok())
        });
        let max_response_bytes = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Default: 4194304")
                .validate(|value, _| value.is_empty() || value.parse::<usize>().is_ok())
        });
        let server_name = cx.new(|cx| InputState::new(window, cx).placeholder("Use server host"));
        let subscriptions = [&timeout, &max_response_bytes, &server_name].map(|field| {
            cx.subscribe_in(field, window, |view, _, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    let fields = &view.grpc_fields;
                    if let Some(options) = &mut view.requests[view.selected].draft.grpc {
                        options.timeout_ms =
                            fields.timeout.read(cx).value().parse().unwrap_or_default();
                        options.max_response_bytes =
                            fields.max_response_bytes.read(cx).value().parse().ok();
                        let server_name = fields.server_name.read(cx).value().to_string();
                        if options.server_name != server_name {
                            options.server_name = server_name;
                            view.sync_grpc_methods(window, cx);
                        }
                        cx.notify();
                    }
                }
            })
        });
        Self {
            timeout,
            max_response_bytes,
            server_name,
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn load(&self, options: &GrpcOptions, window: &mut Window, cx: &mut App) {
        self.timeout.update(cx, |input, cx| {
            input.set_value(options.timeout_ms.to_string(), window, cx)
        });
        self.max_response_bytes.update(cx, |input, cx| {
            input.set_value(
                options
                    .max_response_bytes
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
                window,
                cx,
            )
        });
        self.server_name.update(cx, |input, cx| {
            input.set_value(options.server_name.clone(), window, cx)
        });
    }
}

fn schema_target(request: &Request) -> Request {
    Request {
        url: request.url.clone(),
        grpc: request.grpc.as_ref().map(|options| GrpcOptions {
            secure: options.secure,
            verify_tls: options.verify_tls,
            server_name: options.server_name.clone(),
            proto: options.proto.clone(),
            ..GrpcOptions::default()
        }),
        headers: request.headers.clone(),
        authorization: request.authorization.clone(),
        body: String::new(),
        ..Request::grpc()
    }
}

enum CallUpdate {
    Progress(grpc::Event),
    Finished(scripting::Execution),
}

impl Workspace {
    pub(super) fn set_grpc_tls(
        &mut self,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entry = &mut self.requests[self.selected];
        let url = entry
            .draft
            .url
            .trim()
            .split_once("://")
            .and_then(|(scheme, authority)| {
                let scheme = match (scheme, enabled) {
                    ("grpc" | "grpcs", true) => "grpcs",
                    ("grpc" | "grpcs", false) => "grpc",
                    ("http" | "https", true) => "https",
                    ("http" | "https", false) => "http",
                    _ => return None,
                };
                Some(format!("{scheme}://{authority}"))
            });
        entry.draft.grpc.as_mut().expect("gRPC request").secure = url.is_none().then_some(enabled);
        entry.grpc_state.error = None;
        if let Some(url) = url {
            entry.draft.url = url.clone();
            self.url.update(cx, |input, cx| {
                input.set_value(url.clone(), window, cx);
                input.set_decorations(url_highlight::decorations(&url), cx);
            });
        }
        self.sync_grpc_methods(window, cx);
        self.discover_grpc_methods_if_needed(false, window, cx);
        cx.notify();
    }

    pub(super) fn sync_grpc_methods(&self, window: &mut Window, cx: &mut Context<Self>) {
        let entry = self.active();
        let mut paths = entry
            .grpc_state
            .current_schema(&entry.draft)
            .map(|pool| {
                grpc::methods(pool)
                    .iter()
                    .map(grpc::method_path)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !entry.draft.method.is_empty() && !paths.contains(&entry.draft.method) {
            paths.push(entry.draft.method.clone());
        }
        let selection = paths.iter().position(|path| path == &entry.draft.method);
        self.method.update(cx, |select, cx| {
            select.set_items(paths.into_iter().map(MethodOption).collect(), window, cx);
            select.set_selected_index(
                selection.map(|row| IndexPath::default().row(row)),
                window,
                cx,
            );
        });
    }

    pub(super) fn grpc_kind(&self) -> Option<&'static str> {
        let entry = self.active();
        let pool = entry.grpc_state.current_schema(&entry.draft)?;
        grpc::methods(pool)
            .iter()
            .find(|method| grpc::method_path(method) == entry.draft.method)
            .map(grpc::method_kind)
    }

    pub(super) fn resolve_grpc_target(&self) -> Result<Request> {
        let target = schema_target(&self.active().draft);
        if target
            .grpc
            .as_ref()
            .is_some_and(|grpc| grpc.proto.is_some())
        {
            return Ok(target);
        }
        let scopes = self.variable_scopes();
        let scoped = scopes
            .collection
            .into_iter()
            .chain(scopes.folder)
            .chain(scopes.environment)
            .collect::<Vec<_>>();
        resolve_request(&target, &scopes.globals, &scoped)
    }

    pub(super) fn discover_grpc_methods_if_needed(
        &mut self,
        retry_failed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entry = self.active();
        let Some(options) = &entry.draft.grpc else {
            return;
        };
        if (options.proto.is_some() || !entry.draft.url.trim().is_empty())
            && !entry.grpc_state.loading
            && !entry.response.is_sending()
            && (retry_failed || entry.grpc_state.error.is_none())
            && entry.grpc_state.current_schema(&entry.draft).is_none()
        {
            self.load_grpc_methods(window, cx);
        }
    }

    pub(super) fn load_grpc_methods(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.active().id;
        let draft = schema_target(&self.active().draft);
        let target = match self.resolve_grpc_target() {
            Ok(target) => target,
            Err(error) => {
                self.requests[self.selected].grpc_state.error = Some(format!("{error:#}"));
                cx.notify();
                return;
            }
        };
        self.requests[self.selected].grpc_state.loading = true;
        self.requests[self.selected].grpc_state.error = None;
        cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_spawn(async move { grpc::discover(&target) })
                .await;
            _ = view.update_in(cx, |view, window, cx| {
                let Some(index) = view.request_index(id) else {
                    return;
                };
                let entry = &mut view.requests[index];
                entry.grpc_state.loading = false;
                if draft == schema_target(&entry.draft) {
                    view.apply_grpc_schema(index, result.map(|pool| (draft, pool)), window, cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn apply_grpc_schema(
        &mut self,
        index: usize,
        result: Result<(Request, DescriptorPool)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entry = &mut self.requests[index];
        match result {
            Ok((target, pool)) => {
                let methods = grpc::methods(&pool);
                if !methods
                    .iter()
                    .any(|method| grpc::method_path(method) == entry.draft.method)
                {
                    entry.draft.method = if methods.len() == 1 {
                        grpc::method_path(&methods[0])
                    } else {
                        String::new()
                    };
                }
                entry.grpc_state.schema = Some((target, pool));
                entry.grpc_state.error = None;
            }
            Err(error) => entry.grpc_state.error = Some(format!("{error:#}")),
        }
        if index == self.selected {
            self.sync_grpc_methods(window, cx);
        }
        cx.notify();
    }

    pub(super) fn choose_grpc_proto(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.active().id;
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import .proto or descriptor set".into()),
        });
        self.requests[self.selected].grpc_state.loading = true;
        cx.spawn_in(window, async move |view, cx| {
            let result: Result<Option<(GrpcProto, DescriptorPool)>> = async {
                let Some(path) = picker
                    .await
                    .context("The file dialog closed unexpectedly")??
                    .and_then(|paths| paths.into_iter().next())
                else {
                    return Ok(None);
                };
                cx.background_spawn(async move { load_proto(path) })
                    .await
                    .map(Some)
            }
            .await;
            _ = view.update_in(cx, |view, window, cx| {
                let Some(index) = view.request_index(id) else {
                    return;
                };
                let entry = &mut view.requests[index];
                entry.grpc_state.loading = false;
                match result {
                    Ok(Some((proto, pool))) => {
                        entry.draft.grpc.as_mut().expect("gRPC request").proto = Some(proto);
                        let target = schema_target(&entry.draft);
                        view.apply_grpc_schema(index, Ok((target, pool)), window, cx);
                    }
                    Ok(None) => {}
                    Err(error) => view.apply_grpc_schema(index, Err(error), window, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn use_grpc_reflection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entry = &mut self.requests[self.selected];
        entry.draft.grpc.as_mut().expect("gRPC request").proto = None;
        entry.grpc_state.schema = None;
        self.sync_grpc_methods(window, cx);
        self.load_grpc_methods(window, cx);
    }

    pub(super) fn use_grpc_example(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entry = self.active();
        let Some(pool) = entry.grpc_state.current_schema(&entry.draft) else {
            return;
        };
        let Some(method) = grpc::methods(pool)
            .into_iter()
            .find(|method| grpc::method_path(method) == entry.draft.method)
        else {
            return;
        };
        match grpc::example(&method) {
            Ok(example) => self
                .request_body_editor()
                .update(cx, |editor, cx| editor.replace_all(example, window, cx)),
            Err(error) => self.body_format_error = Some(format!("{error:#}")),
        }
        cx.notify();
    }

    pub(super) fn invoke_grpc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.active().id;
        let draft = self.active().draft.clone();
        let scopes = self.variable_scopes();
        let (sender, receiver) = tokio::sync::mpsc::channel(32);
        let (cancel_sender, cancel) = oneshot::channel();
        let (events, progress) = mpsc::channel(64);
        let known_schema = draft
            .grpc
            .as_ref()
            .and_then(|options| options.proto.as_ref())
            .and_then(|_| self.active().grpc_state.current_schema(&draft))
            .cloned();
        let call = grpc::Call {
            events,
            messages: receiver,
            cancel,
            known_schema,
        };
        let client = match &self.client {
            Ok(client) => client.clone(),
            Err(error) => {
                self.requests[self.selected].response = RequestResponse::Failed(error.clone());
                cx.notify();
                return;
            }
        };
        let (finished, completion) = oneshot::channel();
        let collection_id = scopes.collection_id;
        let worker = thread::Builder::new()
            .name("prestman-grpc".into())
            .spawn(move || {
                let execution = scripting::execute_with(
                    &client,
                    draft,
                    scopes,
                    |request, globals, scoped, observe| call.run(request, globals, scoped, observe),
                );
                _ = finished.send(execution);
            });
        if let Err(error) = worker {
            self.requests[self.selected].response = RequestResponse::Failed(error.to_string());
            cx.notify();
            return;
        }
        let task = cx.spawn_in(window, async move |view, cx| {
            let updates = call_updates(progress, completion).ready_chunks(32);
            futures::pin_mut!(updates);
            while let Some(update) = updates.next().await {
                if !view
                    .update_in(cx, |view, window, cx| {
                        let Some(index) = view.request_index(id) else {
                            return false;
                        };
                        for update in update {
                            view.apply_grpc_update(index, collection_id, update, cx);
                        }
                        if index == view.selected {
                            view.load_response(window, cx);
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false)
                {
                    break;
                }
            }
        });
        let state = &mut self.requests[self.selected];
        state.response = RequestResponse::Sending { _task: task };
        state.script_report = Report::default();
        state.grpc_state.run = Some(ActiveCall {
            sender: Some(sender),
            can_send: false,
            cancel: Some(cancel_sender),
        });
        state.grpc_state.messages.clear();
        state.grpc_state.message_bytes = 0;
        state.grpc_state.metadata.clear();
        state.grpc_state.streaming = false;
        self.response_pane = Pane::Body;
        self.load_response(window, cx);
        cx.notify();
    }

    fn apply_grpc_update(
        &mut self,
        index: usize,
        collection_id: Option<usize>,
        update: CallUpdate,
        cx: &mut Context<Self>,
    ) {
        let active = index == self.selected;
        let state = &mut self.requests[index].grpc_state;
        match update {
            CallUpdate::Progress(grpc::Event::Ready {
                client_streaming,
                streaming,
            }) => {
                if let Some(run) = &mut state.run {
                    run.can_send = client_streaming;
                }
                state.streaming = streaming;
                if active && streaming && self.response_pane == Pane::Body {
                    self.response_pane = Pane::Messages;
                }
            }
            CallUpdate::Progress(grpc::Event::Metadata(metadata)) => state.metadata = metadata,
            CallUpdate::Progress(grpc::Event::Sent(body)) => {
                if active {
                    self.body_format_error = None;
                }
                state.record("Sent", body);
            }
            CallUpdate::Progress(grpc::Event::Received(body)) => state.record("Received", body),
            CallUpdate::Progress(grpc::Event::InputError(error)) => {
                if active {
                    self.body_format_error = Some(error.clone());
                }
                state.record("Message error", error);
            }
            CallUpdate::Finished(execution) => {
                state.run = None;
                self.apply_script_mutations(collection_id, &execution.mutations, cx);
                let entry = &mut self.requests[index];
                entry.script_report = execution.report;
                entry.response = match execution.response {
                    Ok(mut response) => {
                        if response.headers.is_empty() {
                            response.headers.clone_from(&entry.grpc_state.metadata);
                        }
                        let highlight = (response.is_json && response.body.len() <= 256 * 1024)
                            .then(|| {
                                ResponseHighlight::json(
                                    &response.body,
                                    &*Theme::global(cx).highlight_theme,
                                )
                            });
                        RequestResponse::Finished {
                            response,
                            highlight,
                            body_editor: None,
                        }
                    }
                    Err(error) => RequestResponse::Failed(error),
                };
            }
        }
    }

    pub(super) fn send_grpc_message(&mut self, cx: &mut Context<Self>) {
        let entry = &self.requests[self.selected];
        if let Some(sender) = entry
            .grpc_state
            .run
            .as_ref()
            .filter(|run| run.can_send)
            .and_then(|run| run.sender.as_ref())
        {
            self.body_format_error = sender
                .try_send(entry.draft.body.clone())
                .err()
                .map(|error| {
                    if matches!(error, tokio::sync::mpsc::error::TrySendError::Full(_)) {
                        "The message queue is full. Try again shortly.".into()
                    } else {
                        "The gRPC stream is closed.".into()
                    }
                });
            cx.notify();
        }
    }

    pub(super) fn end_grpc_stream(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = &mut self.requests[self.selected].grpc_state.run {
            run.sender = None;
            cx.notify();
        }
    }

    pub(super) fn cancel_grpc_call(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = &mut self.requests[self.selected].grpc_state.run {
            if let Some(cancel) = run.cancel.take() {
                _ = cancel.send(());
            }
            run.sender = None;
            cx.notify();
        }
    }
}

fn load_proto(path: PathBuf) -> Result<(GrpcProto, DescriptorPool)> {
    let proto = grpc::import_proto(&path)
        .with_context(|| format!("Could not import {}", path.display()))?;
    let mut request = Request::grpc();
    request.grpc.as_mut().expect("gRPC request").proto = Some(proto.clone());
    Ok((proto, grpc::discover(&request)?))
}

fn call_updates(
    progress: mpsc::Receiver<grpc::Event>,
    completion: oneshot::Receiver<scripting::Execution>,
) -> impl futures::Stream<Item = CallUpdate> {
    progress
        .map(CallUpdate::Progress)
        .chain(futures::stream::once(async {
            let execution = completion.await.unwrap_or_else(|_| scripting::Execution {
                response: Err("The gRPC worker stopped unexpectedly".into()),
                report: Report::default(),
                mutations: Vec::new(),
            });
            CallUpdate::Finished(execution)
        }))
}
