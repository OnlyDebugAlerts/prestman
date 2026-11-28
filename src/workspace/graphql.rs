use super::*;
use crate::{
    graphql,
    request::{GraphqlOptions, Request},
};

pub(super) struct GraphqlFields {
    pub(super) variables: Entity<EditorState>,
    pub(super) operation: Entity<InputState>,
    pub(super) schema: Entity<EditorState>,
    _subscriptions: [Subscription; 2],
}

impl GraphqlFields {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let variables = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("jsonc")
                .line_number(true)
        });
        let operation =
            cx.new(|cx| InputState::new(window, cx).placeholder("Operation name (optional)"));
        let schema = cx.new(|cx| EditorState::new(window, cx).line_number(true));
        let variables_change = cx.subscribe(&variables, |view, state, event, cx| {
            if matches!(event, InputEvent::Change) {
                if let Some(options) = &mut view.requests[view.selected].draft.graphql {
                    options.variables = state.read(cx).value().to_string();
                    cx.notify();
                }
            }
        });
        let operation_change = cx.subscribe(&operation, |view, state, event, cx| {
            if matches!(event, InputEvent::Change) {
                if let Some(options) = &mut view.requests[view.selected].draft.graphql {
                    options.operation_name = state.read(cx).value().to_string();
                    cx.notify();
                }
            }
        });
        Self {
            variables,
            operation,
            schema,
            _subscriptions: [variables_change, operation_change],
        }
    }

    pub(super) fn load(&self, options: &GraphqlOptions, window: &mut Window, cx: &mut App) {
        self.variables.update(cx, |editor, cx| {
            editor.set_value(options.variables.clone(), window, cx)
        });
        self.operation.update(cx, |input, cx| {
            input.set_value(options.operation_name.clone(), window, cx)
        });
    }
}

#[derive(Default)]
pub(super) struct GraphqlState {
    pub(super) loading: bool,
    schema: Option<(Request, Result<String, String>)>,
}

impl GraphqlState {
    pub(super) fn result(&self, draft: &Request) -> Option<&Result<String, String>> {
        self.schema
            .as_ref()
            .and_then(|(target, schema)| same_endpoint(target, draft).then_some(schema))
    }
}

fn same_endpoint(target: &Request, draft: &Request) -> bool {
    draft.graphql.is_some()
        && target.url == draft.url
        && target.headers == draft.headers
        && target.authorization == draft.authorization
        && target.query == draft.query
        && target.path_variables == draft.path_variables
}

impl Workspace {
    pub(super) fn sync_graphql_schema(&self, window: &mut Window, cx: &mut Context<Self>) {
        let entry = self.active();
        let text = entry
            .graphql_state
            .result(&entry.draft)
            .and_then(|result| result.as_ref().ok())
            .cloned()
            .unwrap_or_default();
        self.graphql_fields
            .schema
            .update(cx, |editor, cx| editor.set_value(text, window, cx));
    }

    pub(super) fn load_graphql_schema(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entry = self.active();
        if entry.graphql_state.loading
            || entry.draft.graphql.is_none()
            || entry.draft.url.trim().is_empty()
        {
            return;
        }
        let id = entry.id;
        let target = graphql::introspection_request(&entry.draft);
        let scopes = self.variable_scopes();
        let client = match &self.client {
            Ok(client) => client.clone(),
            Err(error) => {
                let error = error.clone();
                self.requests[self.selected].graphql_state.schema = Some((target, Err(error)));
                cx.notify();
                return;
            }
        };
        self.requests[self.selected].graphql_state.loading = true;
        cx.spawn_in(window, async move |view, cx| {
            let request = target.clone();
            let result = cx
                .background_spawn(async move {
                    let request = scopes.resolve_request(&request)?;
                    graphql::load_schema(&client, request)
                })
                .await
                .map_err(|error| format!("{error:#}"));
            _ = view.update_in(cx, |view, window, cx| {
                let Some(index) = view.request_index(id) else {
                    return;
                };
                let entry = &mut view.requests[index];
                entry.graphql_state.loading = false;
                if same_endpoint(&target, &entry.draft) {
                    entry.graphql_state.schema = Some((target, result));
                }
                if view.selected == index {
                    view.sync_graphql_schema(window, cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
