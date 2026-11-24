use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    assets::IconName,
    component::{
        Disableable, ElementExt, Sizable,
        button::{Button, ButtonVariants},
        input::{Escape, Textarea},
    },
};
use std::{cell::Cell, ops::Range, rc::Rc, time::Duration};

use super::*;
use crate::request::{path_parameter, path_range};

#[derive(Clone)]
pub(super) enum VariableField {
    Input(Entity<InputState>),
    Url(Entity<TextareaState>),
    Editor(Entity<EditorState>),
}

impl From<&Entity<InputState>> for VariableField {
    fn from(value: &Entity<InputState>) -> Self {
        Self::Input(value.clone())
    }
}
impl From<&Entity<TextareaState>> for VariableField {
    fn from(value: &Entity<TextareaState>) -> Self {
        Self::Url(value.clone())
    }
}
impl From<&Entity<EditorState>> for VariableField {
    fn from(value: &Entity<EditorState>) -> Self {
        Self::Editor(value.clone())
    }
}

impl VariableField {
    fn focus(&self, window: &mut Window, cx: &mut App) {
        match self {
            Self::Input(field) => field.read(cx).focus_handle(cx).focus(window, cx),
            Self::Url(field) => field.read(cx).focus_handle(cx).focus(window, cx),
            Self::Editor(field) => field.read(cx).focus_handle(cx).focus(window, cx),
        }
    }
    fn id(&self) -> EntityId {
        match self {
            Self::Input(field) => field.entity_id(),
            Self::Url(field) => field.entity_id(),
            Self::Editor(field) => field.entity_id(),
        }
    }
    fn text(&self, cx: &App) -> SharedString {
        match self {
            Self::Input(field) => field.read(cx).value(),
            Self::Url(field) => field.read(cx).value(),
            Self::Editor(field) => field.read(cx).value(),
        }
    }
    fn bounds(&self, range: &Range<usize>, cx: &App) -> Option<Bounds<Pixels>> {
        match self {
            Self::Input(field) => field.read(cx).range_to_bounds(range),
            Self::Url(field) => field.read(cx).range_to_bounds(range),
            Self::Editor(field) => field.read(cx).range_to_bounds(range),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Span {
    range: Range<usize>,
    name: String,
    path: bool,
}

fn spans(text: &str, url: bool) -> Vec<Span> {
    let mut result = Vec::new();
    let mut offset = 0;
    while let Some(start) = text[offset..].find("{{") {
        let start = offset + start;
        let Some(end) = text[start + 2..].find("}}") else {
            break;
        };
        let end = start + 2 + end;
        let name = text[start + 2..end].trim();
        if !name.is_empty() && !name.contains(['{', '}']) {
            result.push(Span {
                range: start..end + 2,
                name: name.into(),
                path: false,
            });
        }
        offset = end + 2;
    }
    if url {
        let range = path_range(text);
        let mut offset = range.start;
        for segment in text[range].split('/') {
            if let Some(name) = path_parameter(segment) {
                result.push(Span {
                    range: offset..offset + segment.len(),
                    name: name.into(),
                    path: true,
                });
            }
            offset += segment.len() + 1;
        }
    }
    result
}

pub(super) struct HoverVariable {
    field: VariableField,
    span: Span,
    token: String,
    point: Point<Pixels>,
    ready: bool,
    _task: Task<()>,
    close_task: Option<Task<()>>,
    popup_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    value_state: Option<Entity<TextareaState>>,
    rendered_value: String,
}

impl Workspace {
    fn hover_popup_focused(&self, window: &Window, cx: &App) -> bool {
        self.variable_hover
            .as_ref()
            .and_then(|hover| hover.value_state.as_ref())
            .is_some_and(|field| field.read(cx).focus_handle(cx).is_focused(window))
    }

    fn defer_hover_close(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hover) = self
            .variable_hover
            .as_ref()
            .filter(|hover| hover.field.id() == id)
        else {
            return;
        };
        if !hover.ready {
            self.variable_hover = None;
            cx.notify();
            return;
        }
        let key = hover.span.clone();
        let task = cx.spawn_in(window, async move |view, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(450))
                .await;
            _ = view.update_in(cx, |view, window, cx| {
                let Some(hover) = view
                    .variable_hover
                    .as_ref()
                    .filter(|hover| hover.field.id() == id && hover.span == key)
                else {
                    return;
                };
                let point = window.mouse_position();
                let over_popup = hover
                    .popup_bounds
                    .get()
                    .is_some_and(|bounds| bounds.contains(&point));
                let over_source = hover
                    .field
                    .bounds(&hover.span.range, cx)
                    .is_some_and(|bounds| bounds.contains(&point));
                if !over_popup && !over_source && !view.hover_popup_focused(window, cx) {
                    view.variable_hover = None;
                    cx.notify();
                }
            });
        });
        if let Some(hover) = &mut self.variable_hover {
            hover.close_task = Some(task);
        }
    }
    pub(super) fn variable_field(
        &self,
        field: impl Into<VariableField>,
        child: impl IntoElement,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let field = field.into();
        let id = field.id();
        let text = field.text(cx);
        let candidates = spans(&text, matches!(field, VariableField::Url(_)));
        let full_height = matches!(field, VariableField::Editor(_));
        div()
            .id(("variable-field", id))
            .w_full()
            .min_w_0()
            .when(full_height, |panel| panel.h_full().min_h_0())
            .on_hover(cx.listener(move |view, hovered: &bool, window, cx| {
                if !hovered
                    && view
                        .variable_hover
                        .as_ref()
                        .is_some_and(|hover| hover.field.id() == id)
                {
                    view.defer_hover_close(id, window, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| {
                    if view.variable_hover.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(
                cx.listener(move |view, event: &MouseMoveEvent, window, cx| {
                    if view.hover_popup_focused(window, cx) {
                        return;
                    }
                    let hovered = event
                        .pressed_button
                        .is_none()
                        .then(|| {
                            candidates.iter().find(|span| {
                                if matches!(field, VariableField::Editor(_)) {
                                    text[span.range.clone()].char_indices().any(|(offset, ch)| {
                                        let start = span.range.start + offset;
                                        field
                                            .bounds(&(start..start + ch.len_utf8()), cx)
                                            .is_some_and(|bounds| bounds.contains(&event.position))
                                    })
                                } else {
                                    field
                                        .bounds(&span.range, cx)
                                        .is_some_and(|bounds| bounds.contains(&event.position))
                                }
                            })
                        })
                        .flatten();
                    let Some(span) = hovered else {
                        if view
                            .variable_hover
                            .as_ref()
                            .is_some_and(|hover| hover.field.id() == id)
                        {
                            view.defer_hover_close(id, window, cx);
                        }
                        return;
                    };
                    if view
                        .variable_hover
                        .as_ref()
                        .is_some_and(|hover| hover.field.id() == id && hover.span == *span)
                    {
                        if let Some(hover) = &mut view.variable_hover {
                            hover.close_task = None;
                        }
                        return;
                    }
                    let key = span.clone();
                    let task = cx.spawn_in(window, async move |view, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(350))
                            .await;
                        _ = view.update(cx, |view, cx| {
                            if let Some(hover) = &mut view.variable_hover {
                                if hover.field.id() == id && hover.span == key {
                                    hover.ready = true;
                                    cx.notify();
                                }
                            }
                        });
                    });
                    view.variable_hover = Some(HoverVariable {
                        field: field.clone(),
                        span: span.clone(),
                        token: text[span.range.clone()].into(),
                        point: event.position,
                        ready: false,
                        _task: task,
                        close_task: None,
                        popup_bounds: Rc::new(Cell::new(None)),
                        value_state: None,
                        rendered_value: String::new(),
                    });
                    cx.notify();
                }),
            )
            .child(child)
    }

    pub(super) fn variable_popup(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let hover = self.variable_hover.as_ref().filter(|hover| hover.ready)?;
        if hover.field.text(cx).get(hover.span.range.clone()) != Some(hover.token.as_str()) {
            return None;
        }
        let group = self
            .variable_editor
            .as_ref()
            .and_then(|editor| match editor.scope {
                VariableScope::Collection(id) => Some(id),
                _ => None,
            })
            .or(self.active().collection_id);
        let mut scopes = self.variable_scopes_for(group);
        if let Some(editor) = &self.variable_editor {
            if let Ok(edited) = editor.variables(cx) {
                match editor.scope {
                    VariableScope::Global => {
                        scopes.globals = crate::variables::merge_edits(
                            &editor.original,
                            &edited,
                            &scopes.globals,
                        )
                    }
                    VariableScope::Environment => {
                        scopes.environment = crate::variables::merge_edits(
                            &editor.original,
                            &edited,
                            &scopes.environment,
                        )
                    }
                    VariableScope::Collection(id) => {
                        if scopes.collection_id == Some(id) {
                            scopes.collection = crate::variables::merge_edits(
                                &editor.original,
                                &edited,
                                &scopes.collection,
                            );
                        } else if group.is_some_and(|leaf| {
                            crate::storage::lineage(&self.collections, leaf)
                                .iter()
                                .any(|group| group.id == id)
                        }) {
                            let mut groups = self.collections.clone();
                            if let Some(group) = groups.iter_mut().find(|group| group.id == id) {
                                group.variables = crate::variables::merge_edits(
                                    &editor.original,
                                    &edited,
                                    &group.variables,
                                );
                            }
                            scopes.folder = group
                                .map(|id| {
                                    crate::storage::lineage(&groups, id)
                                        .iter()
                                        .skip(1)
                                        .flat_map(|group| group.variables.iter().cloned())
                                        .collect()
                                })
                                .unwrap_or_default();
                        }
                    }
                }
            }
        }
        let all = scopes
            .globals
            .iter()
            .chain(&scopes.collection)
            .chain(&scopes.folder)
            .chain(&scopes.environment)
            .cloned()
            .collect::<Vec<_>>();
        let (scope, result) = if hover.span.path {
            (
                "Path variable",
                self.active()
                    .draft
                    .path_variables
                    .iter()
                    .find(|row| row.name == hover.span.name && row.enabled)
                    .ok_or_else(|| anyhow::anyhow!("Path variable is missing or disabled"))
                    .and_then(|row| {
                        crate::variables::resolve_text(&row.value, all.clone().into_iter())
                    }),
            )
        } else {
            let scope = [
                ("Environment", &scopes.environment),
                ("Folder", &scopes.folder),
                ("Collection", &scopes.collection),
                ("Globals", &scopes.globals),
            ]
            .into_iter()
            .find(|(_, rows)| {
                rows.iter()
                    .any(|row| row.enabled && row.name == hover.span.name)
            })
            .map_or("Undefined", |(name, _)| name);
            (
                scope,
                crate::variables::resolve_text(
                    &format!("{{{{{}}}}}", hover.span.name),
                    all.into_iter(),
                ),
            )
        };
        let failed = result.is_err();
        let value = result.unwrap_or_else(|error| error.to_string());
        let copy_value = value.clone();
        let id = hover.field.id();
        let span = hover.span.clone();
        let point = hover.point;
        let token = hover.token.clone();
        let source = hover.field.clone();
        let popup_bounds = hover.popup_bounds.clone();
        let estimated_rows = value
            .lines()
            .map(|line| line.chars().count().div_ceil(40).max(1))
            .sum::<usize>()
            .clamp(1, 6);
        let popup_height = popup_bounds
            .get()
            .map_or(px(60. + estimated_rows as f32 * 18.), |bounds| {
                bounds.size.height
            });
        let anchor = source
            .bounds(&span.range, cx)
            .unwrap_or(Bounds::new(point, size(px(1.), px(1.))));
        let value_state = if let Some(state) = &hover.value_state {
            if hover.rendered_value != value {
                state.update(cx, |state, cx| state.set_value(value.clone(), window, cx));
            }
            state.clone()
        } else {
            let state = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(1, 6)
                    .soft_wrap(true)
                    .placeholder("(empty string)")
                    .default_value(value.clone())
            });
            self.variable_hover.as_mut()?.value_state = Some(state.clone());
            state
        };
        self.variable_hover.as_mut()?.rendered_value = value;
        let viewport = window.viewport_size();
        let below = anchor.origin.y + anchor.size.height + px(6.);
        let top = if below + popup_height <= viewport.height - px(8.) {
            below
        } else {
            (anchor.origin.y - popup_height - px(6.)).max(px(8.))
        };
        Some(
            div()
                .id(("variable-hover-popup", id))
                .absolute()
                .occlude()
                .left(point.x.min((viewport.width - px(380.)).max(px(8.))))
                .top(top)
                .w(px(360.))
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(rgb(0x494459))
                .bg(rgb(0x252230))
                .text_color(rgb(0xebebf5))
                .shadow_md()
                .on_prepaint(move |bounds, _, _| popup_bounds.set(Some(bounds)))
                .on_hover(cx.listener(move |view, hovered: &bool, window, cx| {
                    if view
                        .variable_hover
                        .as_ref()
                        .is_some_and(|hover| hover.field.id() == id && hover.span == span)
                    {
                        if *hovered {
                            if let Some(hover) = &mut view.variable_hover {
                                hover.close_task = None;
                            }
                        } else {
                            view.defer_hover_close(id, window, cx);
                        }
                    }
                }))
                .on_mouse_down_out(cx.listener(|view, _, _, cx| {
                    if view.variable_hover.take().is_some() {
                        cx.notify();
                    }
                }))
                .capture_action(cx.listener(move |view, _: &Escape, window, cx| {
                    view.variable_hover = None;
                    source.focus(window, cx);
                    cx.stop_propagation();
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(rgb(0xb4a0ff))
                                .child(format!("{} · {}", token, scope)),
                        )
                        .child(
                            Button::new("copy-variable-value")
                                .ghost()
                                .compact()
                                .icon(IconName::Copy)
                                .accessibility_label("Copy value")
                                .tooltip("Copy value")
                                .disabled(failed)
                                .tab_stop(false)
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copy_value.clone(),
                                    ))
                                }),
                        ),
                )
                .child(
                    Textarea::new(&value_state)
                        .small()
                        .readonly(true)
                        .appearance(false)
                        .bordered(false)
                        .w_full()
                        .aria_label("Resolved variable value")
                        .text_color(rgb(if failed { 0xf49aab } else { 0xebebf5 })),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::spans;
    #[test]
    fn hover_spans_ignore_url_ports_and_query_colons_and_keep_utf8_offsets() {
        let text = "{{хост}}:8080/themes/:id?value={{ токен }}&other=:no#/:no";
        let parsed = spans(text, true);
        assert_eq!(parsed.len(), 3);
        assert_eq!(
            parsed
                .iter()
                .map(|span| span.name.as_str())
                .collect::<Vec<_>>(),
            ["хост", "токен", "id"]
        );
        assert_eq!(&text[parsed[2].range.clone()], ":id");
        assert_eq!(spans(text, false).len(), 2);
    }
}
