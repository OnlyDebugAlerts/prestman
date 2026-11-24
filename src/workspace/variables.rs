use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum VariableScope {
    Global,
    Environment,
    Collection(usize),
}

pub(super) struct VariableEditor {
    pub(super) scope: VariableScope,
    pub(super) fields: Vec<VariableFields>,
    pub(super) error: Option<String>,
    pub(super) original: Vec<Variable>,
    team_version: Option<crate::cloud::ItemRevision>,
}

pub(super) struct VariableFields {
    pub(super) name: Entity<InputState>,
    pub(super) value: Entity<InputState>,
    pub(super) enabled: bool,
    _subscriptions: [Subscription; 2],
}

impl VariableFields {
    fn new(variable: &Variable, window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Name")
                .default_value(variable.name.clone())
        });
        let value = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Value")
                .default_value(variable.value.clone())
        });
        let subscriptions = [&name, &value].map(|field| {
            cx.subscribe_in(field, window, |view, _, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    if let Some(editor) = &mut view.variable_editor {
                        editor.error = None;
                    }
                    view.ensure_variable_placeholder(window, cx);
                    cx.notify();
                }
            })
        });
        Self {
            name,
            value,
            enabled: variable.enabled,
            _subscriptions: subscriptions,
        }
    }

    fn is_blank(&self, cx: &App) -> bool {
        self.name.read(cx).value().is_empty() && self.value.read(cx).value().is_empty()
    }

    fn variable(&self, cx: &App) -> Variable {
        Variable {
            name: self.name.read(cx).value().trim().to_owned(),
            value: self.value.read(cx).value().to_string(),
            enabled: self.enabled,
        }
    }
}

impl VariableEditor {
    pub(super) fn variables(&self, cx: &App) -> anyhow::Result<Vec<Variable>> {
        let variables = self
            .fields
            .iter()
            .map(|fields| fields.variable(cx))
            .filter(|variable| !variable.name.is_empty() || !variable.value.is_empty())
            .collect::<Vec<_>>();
        let mut names = HashSet::new();
        for variable in &variables {
            anyhow::ensure!(
                !variable.name.is_empty(),
                "Enter a name for every variable with a value."
            );
            anyhow::ensure!(
                !variable.name.contains(['{', '}']),
                "Enter variable names without braces. Use {{{{name}}}} in requests."
            );
            anyhow::ensure!(
                names.insert(&variable.name),
                "Duplicate variable name: {}",
                variable.name
            );
        }
        Ok(variables)
    }
}

impl Workspace {
    pub(super) fn edit_variables(
        &mut self,
        scope: VariableScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.variable_hover = None;
        self.openapi_import = None;
        let variables = match scope {
            VariableScope::Global => &self.global_variables,
            VariableScope::Environment => &self.environment_variables,
            VariableScope::Collection(id) => {
                let Some(collection) = self
                    .collections
                    .iter()
                    .find(|collection| collection.id == id)
                else {
                    return;
                };
                &collection.variables
            }
        };
        let original = variables.clone();
        let mut fields = variables
            .iter()
            .map(|variable| VariableFields::new(variable, window, cx))
            .collect::<Vec<_>>();
        if fields.last().is_none_or(|fields| !fields.is_blank(cx)) {
            fields.push(VariableFields::new(&Variable::default(), window, cx));
        }
        fields[0].name.read(cx).focus_handle(cx).focus(window, cx);
        self.variable_editor = Some(VariableEditor {
            scope,
            fields,
            error: None,
            original,
            team_version: match scope {
                VariableScope::Collection(id) => self.team_group_version(id),
                _ => None,
            },
        });
        self.sync_visualizer(window, cx);
        self.save_error = None;
        cx.notify();
    }

    pub(super) fn close_variables(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.variable_hover = None;
        self.variable_editor = None;
        self.sync_visualizer(window, cx);
        self.focus_url(window, cx);
        cx.notify();
    }

    fn ensure_variable_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.variable_editor.as_ref().is_some_and(|editor| {
            editor
                .fields
                .last()
                .is_none_or(|fields| !fields.is_blank(cx))
        }) {
            let fields = VariableFields::new(&Variable::default(), window, cx);
            if let Some(editor) = &mut self.variable_editor {
                editor.fields.push(fields);
            }
        }
    }

    pub(super) fn remove_variable(
        &mut self,
        row_id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &mut self.variable_editor else {
            return;
        };
        let Some(index) = editor
            .fields
            .iter()
            .position(|fields| fields.name.entity_id() == row_id)
        else {
            return;
        };
        editor.fields.remove(index);
        editor.error = None;
        self.ensure_variable_placeholder(window, cx);
        let editor = self.variable_editor.as_ref().unwrap();
        if let Some(fields) = editor.fields.get(index).or_else(|| editor.fields.last()) {
            fields.name.read(cx).focus_handle(cx).focus(window, cx);
        } else {
            self.sidebar_focus.focus(window, cx);
        }
        cx.notify();
    }

    pub(super) fn set_variable_enabled(
        &mut self,
        row_id: EntityId,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = &mut self.variable_editor {
            if let Some(fields) = editor
                .fields
                .iter_mut()
                .find(|fields| fields.name.entity_id() == row_id)
            {
                fields.enabled = enabled;
            }
            editor.error = None;
        }
        cx.notify();
    }

    pub(super) fn save_variables(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.variable_editor else {
            return;
        };
        let scope = editor.scope;
        let team_version = editor.team_version;
        let variables = match editor.variables(cx) {
            Ok(variables) => variables,
            Err(error) => {
                editor.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let destination = match scope {
            VariableScope::Global => &mut self.global_variables,
            VariableScope::Environment => &mut self.environment_variables,
            VariableScope::Collection(id) => {
                let Some(collection) = self
                    .collections
                    .iter_mut()
                    .find(|collection| collection.id == id)
                else {
                    return;
                };
                &mut collection.variables
            }
        };
        let variables = crate::variables::merge_edits(&editor.original, &variables, destination);
        let previous = std::mem::replace(destination, variables);
        let saved = if self.in_team()
            && matches!(scope, VariableScope::Global | VariableScope::Environment)
        {
            self.save_error = self
                .save_team_cache(false, None)
                .err()
                .map(|error| format!("Could not save variables. {error:#}"));
            self.save_error.is_none()
        } else {
            self.persist_at_revision("Could not save variables", team_version, cx)
        };
        if !saved {
            match scope {
                VariableScope::Global => self.global_variables = previous,
                VariableScope::Environment => self.environment_variables = previous,
                VariableScope::Collection(id) => {
                    if let Some(collection) = self
                        .collections
                        .iter_mut()
                        .find(|collection| collection.id == id)
                    {
                        collection.variables = previous;
                    }
                }
            }
            if let Some(editor) = &mut self.variable_editor {
                editor.error = self.save_error.clone();
            }
            return;
        }
        self.close_variables(window, cx);
    }
}
