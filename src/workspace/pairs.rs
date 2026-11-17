use super::*;
use crate::request::replace_query;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PairKind {
    Query,
    Headers,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestPane {
    Pairs(PairKind),
    Body,
    Authorization,
}

pub(super) struct PairFields {
    pub(super) name: Entity<InputState>,
    pub(super) value: Entity<InputState>,
    _subscriptions: [Subscription; 2],
}

impl PairFields {
    fn new(kind: PairKind, pair: &Pair, window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).default_value(pair.name.clone()));
        let value = cx.new(|cx| InputState::new(window, cx).default_value(pair.value.clone()));
        let subscriptions = [&name, &value].map(|field| {
            cx.subscribe_in(field, window, move |this, field, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.edit_pair(kind, &field, window, cx);
                }
            })
        });
        Self {
            name,
            value,
            _subscriptions: subscriptions,
        }
    }
}

impl Workspace {
    pub(super) fn pair_fields(&self, kind: PairKind) -> &[PairFields] {
        match kind {
            PairKind::Query => &self.query,
            PairKind::Headers => &self.headers,
        }
    }

    fn pair_fields_mut(&mut self, kind: PairKind) -> &mut Vec<PairFields> {
        match kind {
            PairKind::Query => &mut self.query,
            PairKind::Headers => &mut self.headers,
        }
    }

    fn pairs_mut(&mut self, kind: PairKind) -> &mut Vec<Pair> {
        let entry = &mut self.requests[self.selected];
        match kind {
            PairKind::Query => &mut entry.query,
            PairKind::Headers => &mut entry.draft.headers,
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
            PairKind::Query => &entry.query,
            PairKind::Headers => &entry.draft.headers,
        };
        *self.pair_fields_mut(kind) = pairs
            .iter()
            .map(|pair| PairFields::new(kind, pair, window, cx))
            .collect();
    }

    pub(super) fn add_pair(&mut self, kind: PairKind, window: &mut Window, cx: &mut Context<Self>) {
        let pair = Pair::default();
        let fields = PairFields::new(kind, &pair, window, cx);
        fields.name.read(cx).focus_handle(cx).focus(window, cx);
        self.pairs_mut(kind).push(pair);
        self.pair_fields_mut(kind).push(fields);
        cx.notify();
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
        self.pair_fields_mut(kind).remove(index);
        self.pairs_mut(kind).remove(index);
        let fields = self.pair_fields(kind);
        if let Some(row) = fields.get(index).or_else(|| fields.last()) {
            row.name.read(cx).focus_handle(cx).focus(window, cx);
        } else {
            self.focus_url(window, cx);
        }
        self.pairs_changed(kind, window, cx);
    }

    fn edit_pair(
        &mut self,
        kind: PairKind,
        field: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((index, fields)) = self
            .pair_fields(kind)
            .iter()
            .enumerate()
            .find(|(_, fields)| fields.name == *field || fields.value == *field)
        else {
            return;
        };
        let is_name = fields.name == *field;
        let pair = &mut self.pairs_mut(kind)[index];
        let value = field.read(cx).value().to_string();
        if is_name {
            pair.name = value;
        } else {
            pair.value = value;
        }
        self.pairs_changed(kind, window, cx);
    }

    fn pairs_changed(&mut self, kind: PairKind, window: &mut Window, cx: &mut Context<Self>) {
        if kind == PairKind::Query {
            let entry = &mut self.requests[self.selected];
            let url = replace_query(&entry.draft.url, &entry.query);
            entry.draft.url = url.clone();
            self.url
                .update(cx, |input, cx| input.set_value(url, window, cx));
        }
        cx.notify();
    }
}
