use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Disableable, Icon, Theme, ThemeMode, TitleBar,
    button::{Button, ButtonVariants},
    input::{Editor, Input},
    menu::ContextMenuExt,
    select::Select,
    spinner::Spinner,
};
use gpui_kit::prelude::FluentBuilder as _;

use super::*;
use crate::request::Authorization;

const BACKGROUND: u32 = 0x111118;
const SIDEBAR: u32 = 0x171720;
const SURFACE: u32 = 0x1c1c28;
const EDITOR: u32 = 0x13131c;
const BORDER: u32 = 0x2c2c3b;
const TEXT: u32 = 0xebebf5;
const MUTED: u32 = 0x9a99ac;
const ACCENT: u32 = 0xb4a0ff;
const SELECTED: u32 = 0x272039;
const GREEN: u32 = 0x79d8b0;
const AMBER: u32 = 0xf1bf79;
const RED: u32 = 0xf49aab;

#[derive(Clone)]
struct RequestDrag {
    id: usize,
    title: SharedString,
}

impl Render for RequestDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(220.))
            .rounded_md()
            .border_1()
            .border_color(rgb(ACCENT))
            .bg(rgb(SURFACE))
            .text_color(rgb(TEXT))
            .px_2()
            .py_1()
            .truncate()
            .child(self.title.clone())
    }
}

pub fn set_theme(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    Theme::update(cx, |theme| {
        theme.font_family = "Inter".into();
        theme.background = rgb(BACKGROUND).into();
        theme.foreground = rgb(TEXT).into();
        theme.border = rgb(BORDER).into();
        theme.input = rgb(BORDER).into();
        theme.muted_foreground = rgb(MUTED).into();
        theme.popover = rgb(SURFACE).into();
        theme.popover_foreground = rgb(TEXT).into();
        theme.secondary = rgb(SURFACE).into();
        theme.secondary_hover = rgb(SELECTED).into();
        theme.button_primary = rgb(0x704ad6).into();
        theme.button_primary_hover = rgb(0x8258ec).into();
        theme.button_primary_active = rgb(0x6341bf).into();
        theme.button_primary_foreground = rgb(0xffffff).into();
        theme.ring = rgb(ACCENT).into();
        theme.caret = rgb(ACCENT).into();
        theme.selection = rgb(0x433063).into();
    });
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        column()
            .key_context("Prestman")
            .on_action(cx.listener(|this, _: &NewRequest, window, cx| this.new_request(window, cx)))
            .on_action(cx.listener(|this, _: &SaveRequest, _, cx| this.save_request(cx)))
            .on_action(
                cx.listener(|this, _: &SendRequest, window, cx| this.send_request(window, cx)),
            )
            .on_action(cx.listener(|this, _: &FocusUrl, window, cx| this.focus_url(window, cx)))
            .on_action(cx.listener(|this, _: &FocusRequests, window, cx| {
                this.sidebar_focus.focus(window, cx)
            }))
            .size_full()
            .bg(rgb(BACKGROUND))
            .text_color(rgb(TEXT))
            .font_family("Inter")
            .text_size(px(13.))
            .child(
                TitleBar::new()
                    .bg(rgb(SIDEBAR))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        row()
                            .gap_2()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(icon(IconName::Globe, 16., ACCENT))
                            .child("Prestman"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.sidebar(cx))
                    .child(self.main_panel(cx)),
            )
    }
}

impl Workspace {
    fn main_panel(&self, cx: &Context<Self>) -> impl IntoElement {
        column()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(self.toolbar(cx))
            .children(
                self.storage
                    .as_ref()
                    .err()
                    .or(self.save_error.as_ref())
                    .map(|error| {
                        div()
                            .px_5()
                            .py_2()
                            .bg(rgb(0x341f2c))
                            .text_color(rgb(RED))
                            .child(error.clone())
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(self.request_editor(cx))
                    .child(self.response_view(cx)),
            )
            .child(shortcuts())
    }

    fn sidebar(&self, cx: &Context<Self>) -> impl IntoElement {
        column()
            .w(px(234.))
            .flex_shrink_0()
            .min_h_0()
            .bg(rgb(SIDEBAR))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                row()
                    .px_4()
                    .gap_1()
                    .h(px(61.))
                    .flex_shrink_0()
                    .child(section_label("Collections"))
                    .child(div().flex_1())
                    .child(
                        Button::new("new-collection")
                            .ghost()
                            .compact()
                            .icon(IconName::FolderPlus)
                            .accessibility_label("New collection")
                            .tooltip("New collection")
                            .disabled(self.storage.is_err())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.edit_collection(CollectionEdit::New, window, cx);
                            })),
                    )
                    .child(
                        Button::new("new-request")
                            .ghost()
                            .compact()
                            .icon(IconName::Plus)
                            .label("New")
                            .accessibility_label("New request")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.new_request(window, cx)),
                            ),
                    ),
            )
            .children(
                self.collection_edit
                    .map(|edit| self.collection_form(edit, cx)),
            )
            .child(
                column()
                    .id("requests")
                    .flex_1()
                    .min_h_0()
                    .mx_2()
                    .px_1()
                    .pb_2()
                    .gap_1()
                    .key_context("RequestList")
                    .track_focus(&self.sidebar_focus)
                    .on_action(cx.listener(|this, _: &PreviousRequest, window, cx| {
                        this.move_selection(-1, window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &NextRequest, window, cx| {
                        this.move_selection(1, window, cx)
                    }))
                    .overflow_y_scroll()
                    .track_scroll(&self.sidebar_scroll)
                    .children(self.sidebar_rows().into_iter().map(|item| match item {
                        SidebarRow::Collection(index) => {
                            self.collection_row(index, cx).into_any_element()
                        }
                        SidebarRow::Ungrouped => self.ungrouped_row(cx).into_any_element(),
                        SidebarRow::Request(index) => {
                            self.request_row(index, cx).into_any_element()
                        }
                    })),
            )
            .child(
                row()
                    .h(px(32.))
                    .px_4()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(format!(
                        "{} saved",
                        self.requests
                            .iter()
                            .filter(|entry| entry.saved.is_some())
                            .count()
                    )),
            )
    }

    fn collection_form(&self, edit: CollectionEdit, cx: &Context<Self>) -> Div {
        let save_label = match edit {
            CollectionEdit::New => "Create collection",
            CollectionEdit::Rename(_) => "Rename collection",
        };
        row()
            .px_3()
            .pb_2()
            .gap_1()
            .flex_shrink_0()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.collection_name).aria_label("Collection name")),
            )
            .child(
                Button::new("save-collection")
                    .ghost()
                    .compact()
                    .icon(IconName::Check)
                    .accessibility_label(save_label)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.save_collection_edit(window, cx)),
                    ),
            )
            .child(
                Button::new("cancel-collection")
                    .ghost()
                    .compact()
                    .icon(IconName::Close)
                    .accessibility_label("Cancel")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.close_collection_edit(window, cx)),
                    ),
            )
    }

    fn collection_row(&self, index: usize, cx: &Context<Self>) -> impl IntoElement {
        let collection = &self.collections[index];
        let id = collection.id;
        let expanded = !self.collapsed_collections.contains(&id);
        let workspace = cx.entity().downgrade();
        row()
            .id(("collection", id))
            .h(px(36.))
            .px_2()
            .gap_2()
            .min_w_0()
            .flex_shrink_0()
            .rounded_md()
            .cursor_pointer()
            .hover(|style| style.bg(rgb(SURFACE)))
            .role(Role::Button)
            .aria_label(collection.name.clone())
            .aria_expanded(expanded)
            .child(icon(
                if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                },
                12.,
                MUTED,
            ))
            .child(icon(
                if expanded {
                    IconName::FolderOpen
                } else {
                    IconName::Folder
                },
                16.,
                ACCENT,
            ))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(collection.name.clone()),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if !this.collapsed_collections.remove(&id) {
                    this.collapsed_collections.insert(id);
                }
                this.sidebar_focus.focus(window, cx);
                cx.notify();
            }))
            .drag_over::<RequestDrag>(|style, _, _, _| style.bg(rgb(SELECTED)))
            .on_drop(cx.listener(move |this, drag: &RequestDrag, _, cx| {
                this.move_request(drag.id, Some(id), cx);
            }))
            .context_menu(move |menu, _, cx| {
                request_menu::build_collection(menu, workspace.clone(), id, cx)
            })
    }

    fn ungrouped_row(&self, cx: &Context<Self>) -> impl IntoElement {
        row()
            .id("ungrouped-requests")
            .h(px(30.))
            .px_2()
            .flex_shrink_0()
            .rounded_md()
            .child(section_label("Ungrouped"))
            .drag_over::<RequestDrag>(|style, _, _, _| style.bg(rgb(SELECTED)))
            .on_drop(cx.listener(|this, drag: &RequestDrag, _, cx| {
                this.move_request(drag.id, None, cx);
            }))
    }

    fn request_row(&self, index: usize, cx: &Context<Self>) -> impl IntoElement {
        let entry = &self.requests[index];
        let active = index == self.selected;
        let id = entry.id;
        let workspace = cx.entity().downgrade();
        let title = SharedString::from(entry.draft.title());
        row()
            .id(("request", id))
            .min_w_0()
            .h(px(38.))
            .flex_shrink_0()
            .cursor_pointer()
            .rounded_md()
            .border_l_2()
            .border_color(if active {
                rgb(ACCENT).into()
            } else {
                transparent_black()
            })
            .bg(rgb(if active { SELECTED } else { SIDEBAR }))
            .hover(|style| style.bg(rgb(if active { SELECTED } else { SURFACE })))
            .px_2()
            .gap_2()
            .when(entry.collection_id.is_some(), |row| row.ml_4())
            .child(
                div()
                    .w(px(51.))
                    .flex_shrink_0()
                    .text_size(px(10.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(method_color(&entry.draft.method)))
                    .child(entry.draft.method.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(rgb(if active { TEXT } else { MUTED }))
                    .child(title.clone()),
            )
            .when(entry.is_dirty(), |row| {
                row.child(
                    div()
                        .size(px(5.))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(rgb(AMBER)),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(index) = this.request_index(id) {
                    this.sidebar_focus.focus(window, cx);
                    this.select_request(index, window, cx);
                }
            }))
            .when(!self.collections.is_empty(), |row| {
                row.on_drag(RequestDrag { id, title }, |drag, _, _, cx| {
                    cx.new(|_| drag.clone())
                })
            })
            .context_menu(move |menu, window, cx| {
                request_menu::build(menu, workspace.clone(), id, window, cx)
            })
    }

    fn toolbar(&self, cx: &Context<Self>) -> impl IntoElement {
        let entry = self.active();
        let dirty = entry.is_dirty();
        let sending = entry.response.is_sending();
        column()
            .flex_shrink_0()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                row()
                    .h(px(61.))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .px_5()
                    .gap_3()
                    .child(
                        div().flex_1().min_w_0().max_w(px(380.)).child(
                            Input::new(&self.name)
                                .aria_label("Request name")
                                .text_size(px(18.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .appearance(false)
                                .pl_0(),
                        ),
                    )
                    .child(div().flex_1())
                    .child(
                        row()
                            .gap_2()
                            .text_xs()
                            .text_color(rgb(if dirty { AMBER } else { MUTED }))
                            .child(div().size(px(5.)).rounded_full().bg(rgb(if dirty {
                                AMBER
                            } else {
                                GREEN
                            })))
                            .child(if dirty { "Unsaved" } else { "Saved locally" }),
                    )
                    .child(
                        Button::new("save-request")
                            .outline()
                            .label("Save")
                            .disabled(!dirty || self.storage.is_err())
                            .on_click(cx.listener(|this, _, _, cx| this.save_request(cx))),
                    ),
            )
            .child(
                row()
                    .p_5()
                    .gap_3()
                    .child(
                        row()
                            .flex_1()
                            .min_w_0()
                            .h(px(40.))
                            .rounded_md()
                            .bg(rgb(SURFACE))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .child(
                                div()
                                    .w(px(114.))
                                    .flex_shrink_0()
                                    .border_r_1()
                                    .border_color(rgb(BORDER))
                                    .child(
                                        Select::new(&self.method)
                                            .w_full()
                                            .appearance(false)
                                            .accessibility_label("HTTP method")
                                            .text_color(rgb(method_color(&entry.draft.method))),
                                    ),
                            )
                            .child(
                                div().flex_1().min_w_0().child(
                                    Input::new(&self.url)
                                        .appearance(false)
                                        .aria_label("Request URL"),
                                ),
                            ),
                    )
                    .child(
                        Button::new("send-request")
                            .primary()
                            .h(px(40.))
                            .px_4()
                            .icon(IconName::ArrowRight)
                            .label(if sending { "Sending…" } else { "Send" })
                            .disabled(sending)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.send_request(window, cx)),
                            ),
                    ),
            )
    }

    fn request_editor(&self, cx: &Context<Self>) -> impl IntoElement {
        let tabs = [
            ("request-body", "Body", RequestPane::Body),
            (
                "request-query",
                "Query",
                RequestPane::Pairs(PairKind::Query),
            ),
            (
                "request-headers",
                "Headers",
                RequestPane::Pairs(PairKind::Headers),
            ),
            ("request-auth", "Authorization", RequestPane::Authorization),
        ];
        column()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                row()
                    .px_3()
                    .h(px(48.))
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .children(tabs.map(|(id, label, pane)| {
                        pane_button(id, label, self.request_pane == pane).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.request_pane = pane;
                                cx.notify();
                            },
                        ))
                    })),
            )
            .child(match self.request_pane {
                RequestPane::Body => self.body_editor(cx),
                RequestPane::Pairs(kind) => self.pairs_editor(kind, cx),
                RequestPane::Authorization => self.authorization_editor(),
            })
    }

    fn body_editor(&self, cx: &Context<Self>) -> Div {
        column()
            .flex_1()
            .min_h_0()
            .p_4()
            .gap_2()
            .child(editor_surface(
                Editor::new(&self.body).aria_label("Request body"),
            ))
            .child(
                row()
                    .gap_2()
                    .flex_shrink_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(rgb(RED))
                            .children(self.body_format_error.clone()),
                    )
                    .child(
                        Button::new("format-request-json")
                            .outline()
                            .compact()
                            .icon(IconName::Braces)
                            .label("Format JSON")
                            .disabled(self.active().draft.body.trim().is_empty())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.format_request_body(window, cx);
                            })),
                    ),
            )
    }

    fn authorization_editor(&self) -> Div {
        let field = |label, state: &Entity<InputState>, secret| {
            column().gap_2().child(section_label(label)).child(
                Input::new(state)
                    .aria_label(label)
                    .when(secret, |input| input.mask_toggle()),
            )
        };
        let form = column().flex_1().min_h_0().p_4().gap_4().child(
            column().gap_2().child(section_label("Type")).child(
                Select::new(&self.auth.kind)
                    .w_full()
                    .accessibility_label("Authorization type"),
            ),
        );
        match &self.active().draft.authorization {
            Authorization::None => form,
            Authorization::Bearer { .. } => form.child(field("Token", &self.auth.token, true)),
            Authorization::Basic { .. } => form
                .child(field("Username", &self.auth.username, false))
                .child(field("Password", &self.auth.password, true)),
        }
    }

    fn pairs_editor(&self, kind: PairKind, cx: &Context<Self>) -> Div {
        let (name_label, value_label, add_label, remove_label, empty_label) = match kind {
            PairKind::Query => (
                "Parameter name",
                "Parameter value",
                "Add parameter",
                "Remove parameter",
                "No query parameters",
            ),
            PairKind::Headers => (
                "Header name",
                "Header value",
                "Add header",
                "Remove header",
                "No headers",
            ),
        };
        let fields = self.pair_fields(kind);
        column()
            .flex_1()
            .min_h_0()
            .p_4()
            .gap_3()
            .child(
                row()
                    .justify_between()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .children(fields.is_empty().then_some(empty_label)),
                    )
                    .child(
                        Button::new("add-pair")
                            .outline()
                            .compact()
                            .icon(IconName::Plus)
                            .label(add_label)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.add_pair(kind, window, cx)
                            })),
                    ),
            )
            .when(!fields.is_empty(), |panel| {
                panel.child(
                    row()
                        .gap_2()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(div().flex_1().min_w_0().child("Name"))
                        .child(div().flex_1().min_w_0().child("Value"))
                        .child(div().w(px(28.)).flex_shrink_0()),
                )
            })
            .child(
                column()
                    .id("pair-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_2()
                    .pb_1()
                    .children(fields.iter().map(|fields| {
                        let row_id = fields.name.entity_id();
                        row()
                            .gap_2()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&fields.name).aria_label(name_label)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&fields.value).aria_label(value_label)),
                            )
                            .child(
                                Button::new(("remove-pair", row_id))
                                    .ghost()
                                    .compact()
                                    .w(px(28.))
                                    .icon(IconName::Close)
                                    .accessibility_label(remove_label)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.remove_pair(kind, row_id, window, cx)
                                    })),
                            )
                    })),
            )
    }

    fn response_view(&self, cx: &Context<Self>) -> impl IntoElement {
        let response = &self.active().response;
        let body = self.response_pane == Pane::Body;
        let received = matches!(response, RequestResponse::Finished { .. });
        column()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
                row()
                    .px_4()
                    .h(px(48.))
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(section_label("Response"))
                    .child(div().flex_1())
                    .child(
                        pane_button("response-body", "Body", body)
                            .disabled(!received)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.select_response_pane(Pane::Body, window, cx);
                            })),
                    )
                    .child(
                        pane_button("response-headers", "Headers", !body)
                            .disabled(!received)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.select_response_pane(Pane::Headers, window, cx);
                            })),
                    ),
            )
            .child(match response {
                RequestResponse::Idle => empty_panel(
                    icon(IconName::Inbox, 24., ACCENT),
                    "No response yet",
                    "Send a request to inspect its response.",
                ),
                RequestResponse::Sending { .. } => empty_panel(
                    Spinner::new(),
                    "Waiting for response",
                    "The request is in progress.",
                ),
                RequestResponse::Failed(error) => empty_panel(
                    icon(IconName::CircleAlert, 24., RED),
                    "Request failed",
                    error.clone(),
                ),
                RequestResponse::Finished { response, .. } => column()
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .gap_3()
                    .child(response_metadata(response))
                    .child(
                        editor_surface(
                            Editor::new(&self.response)
                                .readonly(true)
                                .aria_label(if body {
                                    "Response body"
                                } else {
                                    "Response headers"
                                })
                                .when(!body, |editor| editor.text_color(rgb(0xb8cbe9))),
                        )
                        .when(!body, |panel| panel.p_3()),
                    ),
            })
    }
}

fn response_metadata(response: &Response) -> Div {
    let status_color = match response.status {
        200..=299 => GREEN,
        400..=599 => RED,
        _ => AMBER,
    };
    let status = format!("{} {}", response.status, response.reason)
        .trim_end()
        .to_owned();
    row()
        .min_w_0()
        .gap_3()
        .text_xs()
        .text_color(rgb(MUTED))
        .child(
            div()
                .px_2()
                .py_1()
                .rounded_md()
                .bg(rgb(SURFACE))
                .text_color(rgb(status_color))
                .font_weight(FontWeight::SEMIBOLD)
                .truncate()
                .child(status),
        )
        .child(
            div()
                .flex_shrink_0()
                .child(format!("{} ms", response.elapsed.as_millis())),
        )
        .child(
            div()
                .flex_shrink_0()
                .child(if response.body_bytes >= 1024 * 1024 {
                    format!("{:.1} MiB", response.body_bytes as f64 / (1024. * 1024.))
                } else if response.body_bytes >= 1024 {
                    format!("{:.1} KiB", response.body_bytes as f64 / 1024.)
                } else {
                    format!("{} B", response.body_bytes)
                }),
        )
}

fn empty_panel(
    symbol: impl IntoElement,
    title: &'static str,
    description: impl Into<SharedString>,
) -> Div {
    column()
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .gap_2()
        .p_6()
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(54.))
                .rounded_xl()
                .bg(rgb(SURFACE))
                .border_1()
                .border_color(rgb(BORDER))
                .mb_1()
                .child(symbol),
        )
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(
            div()
                .text_center()
                .text_color(rgb(MUTED))
                .max_w(px(310.))
                .child(description.into()),
        )
}

fn editor_surface(editor: Editor) -> Div {
    div()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .rounded_lg()
        .overflow_hidden()
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(EDITOR))
        .child(editor.size_full().bordered(false))
}

fn pane_button(id: &'static str, label: &'static str, selected: bool) -> Button {
    Button::new(id)
        .ghost()
        .compact()
        .label(label)
        .bg(rgb(if selected { SELECTED } else { BACKGROUND }))
        .text_color(rgb(if selected { ACCENT } else { MUTED }))
}

fn shortcuts() -> Div {
    let (list, url, send) = if cfg!(target_os = "macos") {
        ("⌘1", "⌘L", "⌘↵")
    } else {
        ("Ctrl+1", "Ctrl+L", "Ctrl+Enter")
    };
    row()
        .h(px(32.))
        .flex_shrink_0()
        .px_4()
        .gap_4()
        .border_t_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SIDEBAR))
        .text_size(px(11.))
        .text_color(rgb(MUTED))
        .children(
            [
                (list, "Requests"),
                ("↑ ↓", "Switch"),
                (url, "URL"),
                (send, "Send"),
            ]
            .map(|(keys, label)| {
                row()
                    .gap_1()
                    .child(div().text_color(rgb(TEXT)).child(keys))
                    .child(label)
            }),
        )
}

fn section_label(label: &'static str) -> Div {
    div()
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
}

fn method_color(method: &str) -> u32 {
    match method {
        "GET" => GREEN,
        "POST" => AMBER,
        "PUT" => 0x8cb9f6,
        "PATCH" => ACCENT,
        "DELETE" => RED,
        _ => MUTED,
    }
}

fn icon(name: IconName, size: f32, color: u32) -> Icon {
    Icon::from(name).size(px(size)).text_color(rgb(color))
}

fn row() -> Div {
    div().flex().items_center()
}

fn column() -> Div {
    div().flex().flex_col()
}
