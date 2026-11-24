use super::*;
use crate::request::{Pair, replace_query};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PairKind {
    Query,
    Headers,
    Path,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestPane {
    Pairs(PairKind),
    Body,
    Authorization,
    Scripts,
    ServiceDefinition,
    GraphqlVariables,
    GraphqlSchema,
}

pub(super) struct PairFields {
    pub(super) enabled: bool,
    pub(super) name: Entity<InputState>,
    pub(super) value: Entity<InputState>,
    _subscriptions: [Subscription; 2],
}

impl PairFields {
    fn new(kind: PairKind, pair: &Pair, window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Name")
                .default_value(pair.name.clone())
        });
        let value = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Value")
                .default_value(pair.value.clone())
        });
        let subscriptions = [&name, &value].map(|field| {
            cx.subscribe_in(field, window, move |this, field, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.edit_pair(kind, &field, window, cx);
                }
            })
        });
        Self {
            enabled: pair.enabled,
            name,
            value,
            _subscriptions: subscriptions,
        }
    }

    fn pair(&self, cx: &App) -> Pair {
        Pair {
            name: self.name.read(cx).value().to_string(),
            value: self.value.read(cx).value().to_string(),
            enabled: self.enabled,
        }
    }

    fn is_blank(&self, cx: &App) -> bool {
        self.name.read(cx).value().is_empty() && self.value.read(cx).value().is_empty()
    }
}

impl Workspace {
    pub(super) fn pair_fields(&self, kind: PairKind) -> &[PairFields] {
        match kind {
            PairKind::Query => &self.query,
            PairKind::Headers => &self.headers,
            PairKind::Path => &self.path_variables,
        }
    }

    fn pair_fields_mut(&mut self, kind: PairKind) -> &mut Vec<PairFields> {
        match kind {
            PairKind::Query => &mut self.query,
            PairKind::Headers => &mut self.headers,
            PairKind::Path => &mut self.path_variables,
        }
    }

    fn pairs_mut(&mut self, kind: PairKind) -> &mut Vec<Pair> {
        let entry = &mut self.requests[self.selected];
        match kind {
            PairKind::Query => &mut entry.draft.query,
            PairKind::Headers => &mut entry.draft.headers,
            PairKind::Path => &mut entry.draft.path_variables,
        }
    }

    pub(super) fn load_pairs(
        &mut self,
        kind: PairKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entry = self.active();
        let pairs = match kind {
            PairKind::Query => &entry.draft.query,
            PairKind::Headers => &entry.draft.headers,
            PairKind::Path => &entry.draft.path_variables,
        };
        *self.pair_fields_mut(kind) = pairs
            .iter()
            .filter(|pair| !pair.is_blank())
            .map(|pair| PairFields::new(kind, pair, window, cx))
            .collect();
        self.ensure_pair_placeholder(kind, window, cx);
    }

    fn ensure_pair_placeholder(
        &mut self,
        kind: PairKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .pair_fields(kind)
            .last()
            .is_none_or(|fields| !fields.is_blank(cx))
        {
            let fields = PairFields::new(kind, &Pair::default(), window, cx);
            self.pair_fields_mut(kind).push(fields);
        }
    }

    pub(super) fn remove_pair(
        &mut self,
        kind: PairKind,
        row_id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .pair_fields(kind)
            .iter()
            .position(|row| row.name.entity_id() == row_id)
        else {
            return;
        };
        let was_blank = self.pair_fields(kind)[index].is_blank(cx);
        self.pair_fields_mut(kind).remove(index);
        self.ensure_pair_placeholder(kind, window, cx);
        let fields = self.pair_fields(kind);
        if let Some(row) = fields.get(index).or_else(|| fields.last()) {
            row.name.read(cx).focus_handle(cx).focus(window, cx);
        } else {
            self.focus_url(window, cx);
        }
        if was_blank {
            cx.notify();
        } else {
            self.pairs_changed(kind, window, cx);
        }
    }

    fn edit_pair(
        &mut self,
        kind: PairKind,
        field: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .pair_fields(kind)
            .iter()
            .any(|fields| fields.name == *field || fields.value == *field)
        {
            return;
        }
        self.pairs_changed(kind, window, cx);
    }

    pub(super) fn set_pair_enabled(
        &mut self,
        kind: PairKind,
        row_id: EntityId,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .pair_fields(kind)
            .iter()
            .position(|row| row.name.entity_id() == row_id)
        else {
            return;
        };
        self.pair_fields_mut(kind)[index].enabled = enabled;
        if self.pair_fields(kind)[index].is_blank(cx) {
            cx.notify();
            return;
        }
        self.pairs_changed(kind, window, cx);
    }

    fn pairs_changed(&mut self, kind: PairKind, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_pair_placeholder(kind, window, cx);
        // The spare input row belongs to the UI; it never dirties or gets saved in a request.
        *self.pairs_mut(kind) = self
            .pair_fields(kind)
            .iter()
            .map(|fields| fields.pair(cx))
            .filter(|pair| !pair.is_blank())
            .collect();
        if kind == PairKind::Query {
            let entry = &mut self.requests[self.selected];
            let url = replace_query(&entry.draft.url, &entry.draft.query);
            entry.draft.url = url.clone();
            self.url.update(cx, |input, cx| {
                input.set_value(url.clone(), window, cx);
                input.set_decorations(url_highlight::decorations(&url), cx);
            });
        }
        cx.notify();
    }
}
