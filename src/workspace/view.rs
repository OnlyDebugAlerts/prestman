use std::cell::Cell;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Disableable, Icon, Selectable, Sizable, Theme, ThemeMode, TitleBar,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Editor, Enter, Escape, Input, Textarea},
    menu::{ContextMenuExt, DropdownMenu},
    select::Select,
    spinner::Spinner,
};
use gpui_kit::prelude::FluentBuilder as _;

use super::navigation::*;
use super::*;
use crate::request::{Authorization, path_parameter, path_range};

pub(super) const BACKGROUND: u32 = 0x10151d;
pub(super) const SIDEBAR: u32 = 0x131c27;
pub(super) const SURFACE: u32 = 0x192430;
pub(super) const RAISED: u32 = 0x232f3d;
pub(super) const EDITOR: u32 = 0x0d141d;
pub(super) const BORDER: u32 = 0x344658;
pub(super) const TEXT: u32 = 0xeef4fd;
pub(super) const MUTED: u32 = 0xabbacb;
pub(super) const FAINT: u32 = 0x8499af;
pub(super) const ACCENT: u32 = 0x72edc6;
const ON_ACCENT: u32 = 0x102921;
pub(super) const SELECTED: u32 = 0x1b3834;
pub(super) const VIOLET: u32 = 0xc4adff;
pub(super) const VIOLET_BG: u32 = 0x302743;
pub(super) const GREEN: u32 = ACCENT;
pub(super) const AMBER: u32 = 0xffd391;
pub(super) const AMBER_BG: u32 = 0x3d3024;
pub(super) const RED: u32 = 0xffacaa;
pub(super) const ERROR_BG: u32 = 0x422b32;

const REQUEST_TAB_BAR_HEIGHT: Pixels = px(34.);
const REQUEST_TAB_HEIGHT: Pixels = px(28.);
const REQUEST_TAB_WIDTH: Pixels = px(134.);
const PANE_BAR_HEIGHT: Pixels = px(32.);
pub(super) const REQUEST_CONTROL_HEIGHT: Pixels = px(30.);
const PAIR_ROW_HEIGHT: Pixels = px(30.);
const PAIR_HEADER_HEIGHT: Pixels = px(25.);
const PAIR_TABLE_MAX_WIDTH: Pixels = px(440.);
const COMPACT_WINDOW_WIDTH: Pixels = px(1080.);

// Sidebar density, in logical pixels.
const SIDEBAR_ROW_HEIGHT: f32 = 28.;
const SIDEBAR_ROW_GAP: f32 = 1.;
const SIDEBAR_ROW_PADDING: f32 = 5.;
const SIDEBAR_ITEM_GAP: f32 = 5.;
const SIDEBAR_COLLECTION_INDENT: f32 = 10.;
const SIDEBAR_METHOD_WIDTH: f32 = 46.;
const DIVIDER_WIDTH: Pixels = px(6.);
const MIN_SIDEBAR_WIDTH: Pixels = px(180.);
const MAX_SIDEBAR_WIDTH: Pixels = px(480.);
const MIN_WORKBENCH_WIDTH: Pixels = px(600.);
const MIN_EDITOR_WIDTH: Pixels = px(240.);
const MIN_VARIABLES_WIDTH: Pixels = px(260.);

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColumnSplit {
    Sidebar,
    RequestResponse,
    RequestVariables,
}

struct ColumnResize {
    split: ColumnSplit,
    origin: Cell<Pixels>,
}

#[derive(Clone)]
struct RequestDrag {
    id: usize,
    title: SharedString,
}

struct RequestTabDrag(RequestDrag);

#[derive(Clone)]
struct FolderDrag {
    id: usize,
    title: SharedString,
}

impl Render for FolderDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(SURFACE))
            .text_color(rgb(TEXT))
            .child(self.title.clone())
    }
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
        theme.title_bar = rgb(SIDEBAR).into();
        theme.title_bar_border = rgb(BORDER).into();
        theme.popover = rgb(SURFACE).into();
        theme.popover_foreground = rgb(TEXT).into();
        theme.secondary = rgb(RAISED).into();
        theme.secondary_hover = rgb(BORDER).into();
        theme.primary = rgb(ACCENT).into();
        theme.primary_foreground = rgb(ON_ACCENT).into();
        theme.button_primary = rgb(ACCENT).into();
        theme.button_primary_hover = rgb(0x9bf6d9).into();
        theme.button_primary_active = rgb(0x62d7b0).into();
        theme.button_primary_foreground = rgb(ON_ACCENT).into();
        theme.ring = rgb(ACCENT).into();
        theme.caret = rgb(ACCENT).into();
        theme.selection = rgb(0x284b52).into();
        let editor = &mut std::sync::Arc::make_mut(&mut theme.highlight_theme).style;
        editor.editor_background = Some(rgb(EDITOR).into());
        editor.editor_gutter_background = Some(rgb(EDITOR).into());
    });
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let compact = Self::compact_layout(window);
        if self.last_compact_layout != compact {
            self.last_compact_layout = compact;
            if self.visualizer.is_some() {
                self.sync_visualizer(window, cx);
            }
        }
        let variable_popup = self.variable_popup(window, cx);
        let show_sidebar = !self.cloud_is_open() && (!compact || self.compact_sidebar_open);
        column()
            .relative()
            .key_context(WORKSPACE_CONTEXT)
            .on_action(cx.listener(|this, _: &NewRequest, window, cx| {
                if !this.cloud_is_open() && !this.comparing_cloud() {
                    this.new_request(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FindRequest, window, cx| {
                this.open_request_finder(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseRequestTab, window, cx| {
                if this.request_view_visible() && !this.request_tabs.is_empty() {
                    this.close_request_tab(this.active().id, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NextRequestTab, window, cx| {
                this.switch_request_tab(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &PreviousRequestTab, window, cx| {
                this.switch_request_tab(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SaveRequest, window, cx| {
                if this.cloud_is_open() || this.comparing_cloud() || this.openapi_import.is_some() {
                    return;
                }
                if this.variable_editor.is_some()
                    && (this.request_tabs.is_empty()
                        || this.variable_editor_focus.contains_focused(window, cx))
                {
                    this.save_variables(window, cx);
                } else {
                    this.save_request(cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &SendRequest, window, cx| this.send_request(window, cx)),
            )
            .on_action(cx.listener(|this, _: &FocusUrl, window, cx| {
                if this.cloud_is_open() {
                    return;
                }
                if let Some(import) = &this.openapi_import {
                    import.url.read(cx).focus_handle(cx).focus(window, cx);
                } else if this.variable_editor.is_some() {
                    this.close_variables(window, cx);
                } else {
                    this.focus_url(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FocusRequests, window, cx| {
                if this.cloud_is_open() {
                    return;
                }
                this.compact_sidebar_open = Self::compact_layout(window);
                this.focus_sidebar(window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusBody, window, cx| {
                this.focus_body(window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusResponse, window, cx| {
                this.focus_response(window, cx);
            }))
            .size_full()
            .bg(rgb(BACKGROUND))
            .text_color(rgb(TEXT))
            .font_family("Inter")
            .text_size(px(13.))
            .child(
                TitleBar::new()
                    .on_close_window(cx.listener(|this, _, window, cx| {
                        if this.save_before_close(cx) {
                            window.remove_window();
                        }
                    }))
                    .h(px(42.))
                    .bg(rgb(SIDEBAR))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(self.workspace_bar(cx)),
            )
            .children(self.update_status(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .when(show_sidebar, |area| {
                        area.child(self.sidebar(window, cx))
                            .child(self.column_divider(
                                ColumnSplit::Sidebar,
                                self.sidebar_width(window),
                                window.viewport_size().width - DIVIDER_WIDTH,
                                cx,
                            ))
                    })
                    .child(self.main_panel(compact, window, cx)),
            )
            .children(variable_popup)
    }
}

impl Workspace {
    pub(super) fn compact_layout(window: &Window) -> bool {
        window.viewport_size().width < COMPACT_WINDOW_WIDTH
    }

    fn workspace_bar(&self, cx: &Context<Self>) -> Div {
        row()
            .size_full()
            .min_w_0()
            .pr_2()
            .gap_2()
            .child(
                row()
                    .gap_2()
                    .flex_shrink_0()
                    .child(
                        row()
                            .justify_center()
                            .size(px(24.))
                            .rounded_md()
                            .bg(rgb(SELECTED))
                            .child(
                                svg()
                                    .path("prestman-mark.svg")
                                    .size(px(16.))
                                    .text_color(rgb(ACCENT)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::BOLD)
                            .child("Prestman"),
                    ),
            )
            .child(self.workspace_switcher(cx))
            .children(self.update_button(cx))
            .child(div().flex_1().min_w_0())
            .child(
                Button::new("find-request")
                    .outline()
                    .small()
                    .disabled(!self.request_view_visible())
                    .flex_shrink_0()
                    .h(REQUEST_CONTROL_HEIGHT)
                    .icon(IconName::Search)
                    .label("Search requests")
                    .tooltip("Search requests")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_request_finder(window, cx)),
                    ),
            )
    }

    fn main_panel(&self, compact: bool, window: &Window, cx: &Context<Self>) -> Div {
        if let Some(panel) = self.cloud_panel(cx) {
            return panel;
        }
        if let Some(comparison) = self.cloud_comparison(cx) {
            return comparison;
        }
        if let Some(import) = &self.openapi_import {
            return self.openapi_import_panel(import, cx);
        }
        column()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .children(self.team_status(cx))
            .child(self.request_tab_bar(compact, cx))
            .when(!self.request_tabs.is_empty(), |panel| {
                panel.child(self.toolbar(compact, window, cx))
            })
            .children(
                self.storage
                    .as_ref()
                    .err()
                    .or(self.save_error.as_ref())
                    .map(|error| {
                        div()
                            .px_3()
                            .py_2()
                            .bg(rgb(ERROR_BG))
                            .text_color(rgb(RED))
                            .child(error.clone())
                    }),
            )
            .child(self.workbench_content(compact, window, cx))
    }

    fn workbench_content(&self, compact: bool, window: &Window, cx: &Context<Self>) -> Div {
        let available =
            window.viewport_size().width - self.sidebar_width(window) - DIVIDER_WIDTH * 2.;
        if self.variable_editor.is_some() {
            if compact || self.request_tabs.is_empty() {
                return column().flex_1().min_h_0().child(self.variables_editor(cx));
            }
            let variables_width = self
                .variables_width
                .clamp(MIN_VARIABLES_WIDTH, available - MIN_EDITOR_WIDTH);
            return div()
                .flex()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(self.request_editor(false, cx))
                .child(self.column_divider(
                    ColumnSplit::RequestVariables,
                    available - variables_width,
                    available,
                    cx,
                ))
                .child(
                    self.variables_editor(cx)
                        .flex_none()
                        .w(variables_width)
                        .bg(rgb(SIDEBAR)),
                );
        }
        if self.request_tabs.is_empty() {
            return empty_panel(
                icon(IconName::Library, 22., ACCENT),
                "Open a request",
                "Select a request from Collections or find it by name, method, or URL.",
            )
            .child(
                Button::new("find-empty-request")
                    .primary()
                    .label("Search requests")
                    .tooltip("Search requests")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_request_finder(window, cx)),
                    ),
            );
        }
        if compact {
            return column()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(self.compact_pane_bar(cx))
                .when_else(
                    self.compact_pane == CompactPane::Request,
                    |area| area.child(self.request_editor(true, cx)),
                    |area| area.child(self.response_view(cx)),
                );
        }
        let request_width = (available * self.request_pane_fraction)
            .clamp(MIN_EDITOR_WIDTH, available - MIN_EDITOR_WIDTH);
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(self.request_editor(false, cx).flex_none().w(request_width))
            .child(self.column_divider(ColumnSplit::RequestResponse, request_width, available, cx))
            .child(self.response_view(cx))
    }

    fn compact_pane_bar(&self, cx: &Context<Self>) -> Div {
        row()
            .h(PANE_BAR_HEIGHT)
            .px_2()
            .gap_1()
            .flex_shrink_0()
            .bg(rgb(SIDEBAR))
            .border_b_1()
            .border_color(rgb(BORDER))
            .children(
                [
                    (CompactPane::Request, "compact-request", "Request"),
                    (CompactPane::Response, "compact-response", "Response"),
                ]
                .map(|(pane, id, label)| {
                    pane_button(id, label, self.compact_pane == pane).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.compact_pane = pane;
                            this.sync_visualizer(window, cx);
                            cx.notify();
                        },
                    ))
                }),
            )
            .when(
                self.compact_pane == CompactPane::Request,
                |bar| match &self.active().response {
                    RequestResponse::Finished { response, .. } => bar.child(response_status(
                        response,
                        self.active().draft.grpc.is_some(),
                    )),
                    RequestResponse::Sending { .. } => {
                        bar.child(status_badge("Sending…", MUTED, RAISED))
                    }
                    RequestResponse::Failed(_) => {
                        bar.child(status_badge("Request failed", RED, ERROR_BG))
                    }
                    RequestResponse::Idle => bar,
                },
            )
    }

    fn request_tab_bar(&self, compact: bool, cx: &Context<Self>) -> impl IntoElement {
        row()
            .h(REQUEST_TAB_BAR_HEIGHT)
            .flex_shrink_0()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(BACKGROUND))
            .px_2()
            .gap_1()
            .when(compact, |bar| {
                bar.child(
                    Button::new("toggle-collections")
                        .ghost()
                        .small()
                        .compact()
                        .flex_shrink_0()
                        .icon(IconName::PanelLeft)
                        .accessibility_label("Toggle sidebar")
                        .toggled(self.compact_sidebar_open)
                        .tooltip("Toggle sidebar")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.compact_sidebar_open = !this.compact_sidebar_open;
                            if this.compact_sidebar_open {
                                this.focus_sidebar(window, cx);
                            }
                            cx.notify();
                        })),
                )
            })
            .child(
                row()
                    .id("open-request-tabs")
                    .role(Role::TabList)
                    .aria_label("Open requests")
                    .h_full()
                    .min_w_0()
                    .gap_1()
                    .overflow_x_scroll()
                    .track_scroll(&self.request_tabs_scroll)
                    .on_drag_move(cx.listener(
                        |this, event: &DragMoveEvent<RequestTabDrag>, window, _| {
                            if !event.bounds.contains(&event.event.position) {
                                return;
                            }
                            let x = event.event.position.x;
                            let edge = px(24.);
                            let delta = if x < event.bounds.left() + edge {
                                px(12.)
                            } else if x > event.bounds.right() - edge {
                                px(-12.)
                            } else {
                                return;
                            };
                            let scroll = &this.request_tabs_scroll;
                            let mut offset = scroll.offset();
                            offset.x = (offset.x + delta).clamp(-scroll.max_offset().x, px(0.));
                            scroll.set_offset(offset);
                            window.refresh();
                        },
                    ))
                    .children(self.request_tabs.iter().filter_map(|&id| {
                        self.request_index(id)
                            .map(|index| self.request_tab(index, cx))
                    })),
            )
            .child(
                Button::new("add-request-tab")
                    .ghost()
                    .compact()
                    .small()
                    .mx_1()
                    .flex_shrink_0()
                    .icon(IconName::Plus)
                    .accessibility_label("New request tab")
                    .tooltip("New request tab")
                    .on_click(cx.listener(|this, _, window, cx| this.new_request(window, cx))),
            )
    }

    fn request_tab(&self, index: usize, cx: &Context<Self>) -> impl IntoElement {
        let entry = &self.requests[index];
        let id = entry.id;
        let active = index == self.selected;
        let method = entry.draft.method_badge();
        let title = SharedString::from(entry.draft.title());
        let dirty = entry.is_dirty() && !self.viewer_draft_saved(entry);
        div()
            .id(("request-tab", id))
            .relative()
            .flex()
            .flex_col()
            .w(REQUEST_TAB_WIDTH)
            .h(REQUEST_TAB_HEIGHT)
            .flex_shrink_0()
            .text_size(px(12.))
            .rounded_md()
            .border_1()
            .border_color(rgb(if active { ACCENT } else { BORDER }))
            .bg(rgb(if active { SURFACE } else { SIDEBAR }))
            .hover(|style| style.bg(rgb(SURFACE)))
            .child(
                row()
                    .flex_1()
                    .min_h_0()
                    .pr_1()
                    .child(
                        row()
                            .id("select-tab")
                            .role(Role::Tab)
                            .aria_label(format!("{method} {title}"))
                            .aria_selected(active)
                            .when(dirty, |tab| tab.aria_description("Unsaved changes"))
                            .focusable()
                            .tab_stop(true)
                            .rounded_md()
                            .focus_visible(|style| style.bg(rgb(SELECTED)))
                            .cursor_grab()
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .px_1p5()
                            .gap_1()
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(method_color(method)))
                                    .child(method.to_owned()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(rgb(if active { TEXT } else { MUTED }))
                                    .child(title.clone()),
                            )
                            .when(dirty, |tab| {
                                tab.child(
                                    div()
                                        .size(px(5.))
                                        .flex_shrink_0()
                                        .rounded_full()
                                        .bg(rgb(AMBER)),
                                )
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(index) = this.request_index(id) {
                                    this.select_request(index, window, cx);
                                }
                            }))
                            .on_drag(
                                RequestTabDrag(RequestDrag {
                                    id,
                                    title: title.clone(),
                                }),
                                |drag, _, _, cx| cx.new(|_| drag.0.clone()),
                            ),
                    )
                    .child(
                        Button::new("close-tab")
                            .ghost()
                            .xsmall()
                            .flex_shrink_0()
                            .icon(IconName::Close)
                            .accessibility_label(format!("Close {title}"))
                            .tooltip("Close tab")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.close_request_tab(id, window, cx);
                            })),
                    ),
            )
            .child(self.request_tab_drop_target(id, false, cx))
            .child(self.request_tab_drop_target(id, true, cx))
    }

    fn request_tab_drop_target(&self, id: usize, after: bool, cx: &Context<Self>) -> Div {
        div()
            .absolute()
            .top_0()
            .h_full()
            .w_1_2()
            .when_else(after, |edge| edge.right_0(), |edge| edge.left_0())
            .drag_over::<RequestTabDrag>(move |style, drag, _, _| {
                if drag.0.id == id {
                    return style;
                }
                let style = if after {
                    style.border_r_2()
                } else {
                    style.border_l_2()
                };
                style.border_color(rgb(ACCENT))
            })
            .on_drop(cx.listener(move |this, drag: &RequestTabDrag, _, cx| {
                this.move_request_tab(drag.0.id, id, after, cx);
            }))
    }

    fn sidebar_width(&self, window: &Window) -> Pixels {
        Self::clamp_sidebar_width(self.sidebar_width, window.viewport_size().width - DIVIDER_WIDTH)
    }

    fn clamp_sidebar_width(width: Pixels, available: Pixels) -> Pixels {
        let max = (available - MIN_WORKBENCH_WIDTH).clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
        width.clamp(MIN_SIDEBAR_WIDTH, max)
    }

    fn resize_columns(
        &mut self,
        split: ColumnSplit,
        left_width: Pixels,
        available: Pixels,
        cx: &mut Context<Self>,
    ) {
        let changed = match split {
            ColumnSplit::Sidebar => {
                let width = Self::clamp_sidebar_width(left_width, available);
                std::mem::replace(&mut self.sidebar_width, width) != width
            }
            ColumnSplit::RequestResponse => {
                let width = left_width.clamp(MIN_EDITOR_WIDTH, available - MIN_EDITOR_WIDTH);
                let fraction = width / available;
                std::mem::replace(&mut self.request_pane_fraction, fraction) != fraction
            }
            ColumnSplit::RequestVariables => {
                let width = (available - left_width)
                    .clamp(MIN_VARIABLES_WIDTH, available - MIN_EDITOR_WIDTH);
                std::mem::replace(&mut self.variables_width, width) != width
            }
        };
        if changed {
            cx.notify();
        }
    }

    fn column_divider(
        &self,
        split: ColumnSplit,
        left_width: Pixels,
        available: Pixels,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let workspace = cx.entity().downgrade();
        let (id, label) = match split {
            ColumnSplit::Sidebar => ("sidebar-divider", "Resize sidebar"),
            ColumnSplit::RequestResponse => {
                ("request-response-divider", "Resize request and response")
            }
            ColumnSplit::RequestVariables => {
                ("request-variables-divider", "Resize request and variables")
            }
        };
        div()
            .id(id)
            .role(Role::Splitter)
            .aria_label(label)
            .flex()
            .justify_center()
            .h_full()
            .w(DIVIDER_WIDTH)
            .flex_shrink_0()
            .cursor_col_resize()
            .bg(rgb(SIDEBAR))
            .hover(|style| style.bg(rgb(SELECTED)))
            .child(div().h_full().w(px(1.)).bg(rgb(BORDER)))
            .on_drag(
                ColumnResize {
                    split,
                    origin: Cell::new(px(0.)),
                },
                move |drag, offset, window, cx| {
                    let width = left_width + offset.x - DIVIDER_WIDTH / 2.;
                    let origin = window.mouse_position().x - width;
                    drag.origin.set(origin);
                    let _ = workspace.update(cx, |this, cx| {
                        this.resize_columns(split, width, available, cx);
                    });
                    cx.new(|_| Empty)
                },
            )
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<ColumnResize>, _, cx| {
                    let drag = event.drag(cx);
                    if drag.split == split {
                        let width = event.event.position.x - drag.origin.get();
                        this.resize_columns(split, width, available, cx);
                    }
                }),
            )
    }

    fn sidebar(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let workspace = cx.entity().downgrade();
        let settings_workspace = workspace.clone();
        let add_workspace = workspace.clone();
        let history = self.history.visible;
        let sidebar = column()
            .id("sidebar")
            .w(self.sidebar_width(window))
            .flex_shrink_0()
            .min_h_0()
            .overflow_hidden()
            .bg(rgb(SIDEBAR))
            .child(
                row()
                    .h(px(36.))
                    .px_2()
                    .gap_1()
                    .flex_shrink_0()
                    .children(
                        [
                            ("sidebar-collections", "Collections", IconName::Library, false),
                            ("sidebar-history", "History", IconName::Clock, true),
                        ]
                        .map(|(id, label, icon, show)| {
                            pane_icon_button(id, label, icon, history == show)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.show_history(show, window, cx);
                                }))
                        }),
                    )
                    .child(div().flex_1())
                    .when(!history, |header| {
                        header.child(
                            Button::new("add-sidebar-item")
                                .ghost()
                                .compact()
                                .small()
                                .icon(IconName::Plus)
                                .accessibility_label("New request or collection")
                                .disabled(self.cloud_is_open())
                                .dropdown_caret(false)
                                .tooltip("New request or collection")
                                .dropdown_menu(move |menu, _, cx| {
                                    request_menu::build_add(menu, add_workspace.clone(), cx)
                                }),
                        )
                    }),
            )
            .when(history, |sidebar| sidebar.child(self.history_sidebar(cx)))
            .when(!history, |sidebar| {
                sidebar.child(
                    column()
                        .id("requests")
                        .flex_1()
                        .min_h_0()
                        .mx_2()
                        .px_1()
                        .py_1()
                        .gap(px(SIDEBAR_ROW_GAP))
                        .role(Role::Tree)
                        .aria_label("Collections and requests")
                        .key_context(SIDEBAR_CONTEXT)
                        .track_focus(&self.sidebar_focus)
                        .on_action(cx.listener(|this, action: &SidebarNavigate, window, cx| {
                            this.navigate_sidebar(*action, window, cx);
                        }))
                        .overflow_y_scroll()
                        .track_scroll(&self.sidebar_scroll)
                        .children(self.sidebar_rows().into_iter().map(|item| {
                            match item {
                                SidebarRow::Collection(index) => {
                                    self.collection_row(index, window, cx).into_any_element()
                                }
                                SidebarRow::Edit => self
                                    .collection_form(
                                        self.collection_edit.expect("sidebar form is open"),
                                        cx,
                                    )
                                    .into_any_element(),
                                SidebarRow::Ungrouped => self.ungrouped_row(cx).into_any_element(),
                                SidebarRow::Request(index) => {
                                    self.request_row(index, window, cx).into_any_element()
                                }
                            }
                        })),
                )
            })
            .child(
                column()
                    .px_2()
                    .py_1()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(
                        Button::new("settings")
                            .ghost()
                            .small()
                            .accessibility_label("Settings")
                            .child(
                                row()
                                    .w_full()
                                    .gap_2()
                                    .child(icon(IconName::Settings2, 16., MUTED))
                                    .child("Settings"),
                            )
                            .dropdown_caret(false)
                            .tooltip("Settings")
                            .dropdown_menu(move |menu, _, cx| {
                                request_menu::build_settings(menu, settings_workspace.clone(), cx)
                            }),
                    ),
            );
        if history {
            sidebar.into_any_element()
        } else {
            sidebar
                .context_menu(move |menu, window, cx| {
                    request_menu::build_sidebar(menu, workspace.clone(), window, cx)
                })
                .into_any_element()
        }
    }

    fn openapi_import_panel(
        &self,
        import: &collection_files::OpenApiImport,
        cx: &Context<Self>,
    ) -> Div {
        column().flex_1().min_w_0().min_h_0().p_4().gap_4().child(
            column()
                .id("openapi-import-panel")
                .w_full()
                .max_w(px(680.))
                .gap_3()
                .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                    cx.stop_propagation();
                    this.close_api_import(window, cx);
                }))
                .child(
                    row()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Import Swagger / OpenAPI"),
                        )
                        .child(
                            Button::new("cancel-openapi-import")
                                .ghost()
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_api_import(window, cx)
                                })),
                        ),
                )
                .child(
                    row()
                        .gap_2()
                        .child(
                            div().flex_1().min_w_0().child(
                                Input::new(&import.url)
                                    .aria_label("Specification URL")
                                    .disabled(import.busy),
                            ),
                        )
                        .child(
                            Button::new("import-openapi-url")
                                .primary()
                                .label("Import URL")
                                .disabled(
                                    import.busy || import.url.read(cx).value().trim().is_empty(),
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.import_api_url(window, cx)
                                })),
                        ),
                )
                .child(
                    row()
                        .gap_2()
                        .child(
                            Button::new("import-openapi-file")
                                .outline()
                                .label("Choose File")
                                .disabled(import.busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.import_api_file(window, cx)
                                })),
                        )
                        .child(div().text_xs().text_color(rgb(MUTED)).child("JSON or YAML")),
                )
                .when(self.in_team(), |panel| {
                    panel.child(div().text_color(rgb(MUTED)).child(
                        "Set baseUrl and other request variables in Local workspace, then link them in Settings.",
                    ))
                })
                .when(import.busy, |panel| {
                    panel.child(row().gap_2().child(Spinner::new()).child("Importing…"))
                })
                .children(
                    import
                        .error
                        .as_ref()
                        .map(|error| div().text_color(rgb(RED)).child(error.clone())),
                ),
        )
    }

    fn collection_form(&self, edit: CollectionEdit, cx: &Context<Self>) -> Div {
        let depth = match edit {
            CollectionEdit::New => 0,
            CollectionEdit::NewFolder(parent) => {
                crate::storage::lineage(&self.collections, parent).len()
            }
            CollectionEdit::Rename(id) => crate::storage::lineage(&self.collections, id)
                .len()
                .saturating_sub(1),
        };
        let folder = match edit {
            CollectionEdit::NewFolder(_) => true,
            CollectionEdit::Rename(id) => self
                .collections
                .iter()
                .any(|group| group.id == id && group.parent_id.is_some()),
            _ => false,
        };
        let save_label = match edit {
            CollectionEdit::New => "Create collection",
            CollectionEdit::NewFolder(_) => "Create folder",
            CollectionEdit::Rename(_) => {
                if folder {
                    "Rename folder"
                } else {
                    "Rename collection"
                }
            }
        };
        row()
            .ml(px(SIDEBAR_COLLECTION_INDENT * depth as f32))
            .px(px(SIDEBAR_ROW_PADDING))
            .h(px(30.))
            .gap(px(SIDEBAR_ITEM_GAP))
            .flex_shrink_0()
            .capture_action(
                cx.listener(|this, _: &Escape, window, cx| this.close_collection_edit(window, cx)),
            )
            .child(div().w(px(12.)).flex_shrink_0())
            .child(icon(
                if folder {
                    IconName::Folder
                } else {
                    IconName::Library
                },
                16.,
                ACCENT,
            ))
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.collection_name)
                        .small()
                        .aria_label(if folder {
                            "Folder name"
                        } else {
                            "Collection name"
                        }),
                ),
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

    fn collection_row(
        &self,
        index: usize,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let collection = &self.collections[index];
        let id = collection.id;
        let expanded = !self.collapsed_collections.contains(&id);
        let cursor = self.sidebar_focus.is_focused(window)
            && self.sidebar_cursor == Some(SidebarTarget::Collection(id));
        let depth = crate::storage::lineage(&self.collections, id)
            .len()
            .saturating_sub(1);
        let workspace = cx.entity().downgrade();
        row()
            .id(("collection", id))
            .h(px(SIDEBAR_ROW_HEIGHT))
            .px(px(SIDEBAR_ROW_PADDING))
            .gap(px(SIDEBAR_ITEM_GAP))
            .ml(px(SIDEBAR_COLLECTION_INDENT * depth as f32))
            .min_w_0()
            .flex_shrink_0()
            .rounded_md()
            .cursor_pointer()
            .hover(|style| style.bg(rgb(if cursor { SELECTED } else { SURFACE })))
            .when(cursor, |row| row.bg(rgb(SELECTED)).aria_active_descendant())
            .role(Role::TreeItem)
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
                if collection.parent_id.is_none() {
                    IconName::Library
                } else if expanded {
                    IconName::FolderOpen
                } else {
                    IconName::Folder
                },
                16.,
                ACCENT,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(collection.name.clone()),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.click_count() > 1 {
                    return;
                }
                this.sidebar_cursor = Some(SidebarTarget::Collection(id));
                this.toggle_sidebar_group(id, cx);
                this.sidebar_focus.focus(window, cx);
            }))
            .drag_over::<RequestDrag>(|style, _, _, _| style.bg(rgb(SELECTED)))
            .on_drop(cx.listener(move |this, drag: &RequestDrag, _, cx| {
                this.move_request(drag.id, Some(id), cx);
            }))
            .drag_over::<FolderDrag>(|style, _, _, _| style.bg(rgb(SELECTED)))
            .on_drop(
                cx.listener(move |this, drag: &FolderDrag, _, cx| {
                    this.move_folder(drag.id, id, cx)
                }),
            )
            .when(collection.parent_id.is_some(), |row| {
                row.on_drag(
                    FolderDrag {
                        id,
                        title: collection.name.clone().into(),
                    },
                    |drag, _, _, cx| cx.new(move |_| drag.clone()),
                )
            })
            .context_menu(move |menu, window, cx| {
                request_menu::build_collection(menu, workspace.clone(), id, window, cx)
            })
    }

    fn ungrouped_row(&self, cx: &Context<Self>) -> impl IntoElement {
        row()
            .id("ungrouped-requests")
            .h(px(10.))
            .px_2()
            .flex_shrink_0()
            .rounded_md()
            .aria_label("Requests outside collections")
            .child(div().w_full().h(px(1.)).bg(rgb(BORDER)))
            .drag_over::<RequestDrag>(|style, _, _, _| style.bg(rgb(SELECTED)))
            .on_drop(cx.listener(|this, drag: &RequestDrag, _, cx| {
                this.move_request(drag.id, None, cx);
            }))
    }

    fn request_row(&self, index: usize, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let entry = &self.requests[index];
        let active = index == self.selected && !self.request_tabs.is_empty();
        let id = entry.id;
        let cursor = self.sidebar_focus.is_focused(window)
            && self.sidebar_cursor == Some(SidebarTarget::Request(id));
        let workspace = cx.entity().downgrade();
        let title = SharedString::from(entry.draft.title());
        let renaming = self
            .request_rename
            .as_ref()
            .is_some_and(|rename| rename.id == id);
        row()
            .id(("request", id))
            .min_w_0()
            .h(px(SIDEBAR_ROW_HEIGHT))
            .flex_shrink_0()
            .cursor_pointer()
            .rounded_md()
            .role(Role::TreeItem)
            .aria_selected(active)
            .when(cursor, |row| row.aria_active_descendant())
            .border_l_2()
            .border_color(if active {
                rgb(ACCENT).into()
            } else if cursor {
                rgb(MUTED).into()
            } else {
                transparent_black()
            })
            .bg(rgb(if active || cursor { SELECTED } else { SIDEBAR }))
            .hover(|style| style.bg(rgb(if active || cursor { SELECTED } else { SURFACE })))
            .px(px(SIDEBAR_ROW_PADDING))
            .gap(px(SIDEBAR_ITEM_GAP))
            .when(entry.collection_id.is_some(), |row| {
                let depth =
                    crate::storage::lineage(&self.collections, entry.collection_id.unwrap()).len();
                row.ml(px(SIDEBAR_COLLECTION_INDENT * depth as f32))
            })
            .child(
                div()
                    .w(px(SIDEBAR_METHOD_WIDTH))
                    .flex_shrink_0()
                    .truncate()
                    .text_size(px(10.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(method_color(entry.draft.method_badge())))
                    .child(entry.draft.method_badge().to_owned()),
            )
            .child(if renaming {
                div()
                    .flex_1()
                    .min_w_0()
                    .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                        if let Some(rename) = this.request_rename.take() {
                            if let Some(index) = this.request_index(rename.id) {
                                this.requests[index].draft.name = rename.original;
                            }
                        }
                        this.sidebar_focus.focus(window, cx);
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child(
                        Input::new(&self.name)
                            .small()
                            .aria_label("Request name")
                            .appearance(false)
                            .px_0(),
                    )
            } else {
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(rgb(if active || cursor { TEXT } else { MUTED }))
                    .child(title.clone())
            })
            .when(entry.is_dirty() && !self.viewer_draft_saved(entry), |row| {
                row.child(
                    div()
                        .size(px(5.))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(rgb(AMBER)),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if this
                    .request_rename
                    .as_ref()
                    .is_some_and(|rename| rename.id == id)
                {
                    return;
                }
                if let Some(index) = this.request_index(id) {
                    this.sidebar_cursor = Some(SidebarTarget::Request(id));
                    this.sidebar_focus.focus(window, cx);
                    this.select_request(index, window, cx);
                }
            }))
            .when(!renaming && !self.collections.is_empty(), |row| {
                row.on_drag(RequestDrag { id, title }, |drag, _, _, cx| {
                    cx.new(|_| drag.clone())
                })
            })
            .context_menu(move |menu, window, cx| {
                request_menu::build(menu, workspace.clone(), id, window, cx)
            })
    }

    fn request_identity(&self, compact: bool) -> Div {
        let entry = self.active();
        let clean = self.viewer_draft_saved(entry) || !entry.is_dirty();
        row()
            .min_w_0()
            .gap_2()
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(if compact { 16. } else { 18. }))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(entry.draft.title().to_owned()),
            )
            .when(clean && self.in_team() && self.team_can_edit(), |row| {
                row.child(status_badge("Saved", MUTED, RAISED).flex_shrink_0())
            })
    }

    fn toolbar(&self, compact: bool, window: &Window, cx: &Context<Self>) -> Div {
        let entry = self.active();
        let toolbar = column()
            .flex_shrink_0()
            .px_2()
            .py_1p5()
            .gap_1p5()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SIDEBAR))
            .child(self.request_identity(compact));
        if entry.draft.grpc.is_some() {
            return toolbar.child(self.grpc_toolbar(cx));
        }
        let sending = entry.response.is_sending();
        let graphql = entry.draft.graphql.is_some();
        toolbar
            .child(
                row()
                    .gap_1p5()
                    .child(
                        row()
                            .id("request-address-bar")
                            .flex_1()
                            .min_w_0()
                            .h(REQUEST_CONTROL_HEIGHT)
                            .rounded_md()
                            .bg(rgb(EDITOR))
                            .border_1()
                            .border_color(rgb(if self.url.focus_handle(cx).is_focused(window) {
                                ACCENT
                            } else {
                                BORDER
                            }))
                            .when(graphql, |bar| {
                                bar.child(
                                    row()
                                        .w(px(72.))
                                        .h_full()
                                        .flex_shrink_0()
                                        .border_r_1()
                                        .border_color(rgb(BORDER))
                                        .bg(rgb(SURFACE))
                                        .px_2()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(VIOLET))
                                        .child("GraphQL"),
                                )
                            })
                            .when(!graphql, |bar| {
                                let method = entry.draft.method.as_str();
                                let color = Hsla::from(rgb(method_color(method)));
                                bar.child(
                                    div()
                                        .ml(px(3.))
                                        .w(px(if method.len() > 4 { 86. } else { 64. }))
                                        .h(px(22.))
                                        .flex_shrink_0()
                                        .child(
                                            Select::new(&self.method)
                                                .xsmall()
                                                .w_full()
                                                .h_full()
                                                .px_1()
                                                .py_0()
                                                .rounded_sm()
                                                .border_color(transparent_black())
                                                .bg(color.opacity(0.12))
                                                .text_size(px(11.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(color)
                                                .menu_width(px(108.))
                                                .accessibility_label("HTTP method"),
                                        ),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .capture_action(cx.listener(
                                        |this, action: &Enter, window, cx| {
                                            if action.secondary {
                                                this.send_request(window, cx);
                                            }
                                            cx.stop_propagation();
                                        },
                                    ))
                                    .child(
                                        self.variable_field(
                                            &self.url,
                                            Textarea::new(&self.url)
                                                .small()
                                                .font_family(
                                                    Theme::global(cx).mono_font_family.clone(),
                                                )
                                                .appearance(false)
                                                .bordered(false)
                                                .h(REQUEST_CONTROL_HEIGHT - px(2.))
                                                .aria_label("Request URL")
                                                .on_paste({
                                                    let url = self.url.clone();
                                                    let workspace = cx.entity().downgrade();
                                                    move |item, window, cx| {
                                                        if let Some(text) =
                                                            item.text().filter(|text| {
                                                                !graphql
                                                                    && crate::curl_import::is_curl(
                                                                        text,
                                                                    )
                                                            })
                                                        {
                                                            let Some(view) = workspace.upgrade()
                                                            else {
                                                                return true;
                                                            };
                                                            let id = view.read(cx).active().id;
                                                            let workspace = workspace.clone();
                                                            // The paste hook runs inside the URL state's update.
                                                            // Reload its fields only after that update has finished.
                                                            window.defer(cx, move |window, cx| {
                                                                _ = workspace.update(
                                                                    cx,
                                                                    |view, cx| {
                                                                        view.import_curl(
                                                                            id, &text, window, cx,
                                                                        )
                                                                    },
                                                                );
                                                            });
                                                            return true;
                                                        }
                                                        if let Some(text) =
                                                            item.text().filter(|text| {
                                                                text.contains(['\r', '\n'])
                                                            })
                                                        {
                                                            url.update(cx, |state, cx| {
                                                                state.replace(
                                                                    text.replace(['\r', '\n'], ""),
                                                                    window,
                                                                    cx,
                                                                )
                                                            });
                                                            true
                                                        } else {
                                                            false
                                                        }
                                                    }
                                                }),
                                            cx,
                                        ),
                                    ),
                            ),
                    )
                    .child(self.inspect_url_variables_button(compact, cx))
                    .child(
                        Button::new("send-request")
                            .primary()
                            .small()
                            .flex_shrink_0()
                            .h(REQUEST_CONTROL_HEIGHT)
                            .label(if sending { "Sending…" } else { "Send" })
                            .disabled(sending || self.variable_editor.is_some())
                            .tooltip("Send request")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.send_request(window, cx)),
                            ),
                    ),
            )
    }

    fn grpc_toolbar(&self, cx: &Context<Self>) -> Div {
        let entry = self.active();
        let state = &entry.grpc_state;
        let run = state.run.as_ref();
        let can_send = run.is_some_and(|run| run.can_send && run.sender.is_some());
        let has_methods = state.current_schema(&entry.draft).is_some();
        let options = entry.draft.grpc.as_ref().expect("gRPC request");
        let tls_enabled = crate::grpc::uses_tls(&entry.draft.url, options);
        let tls_tooltip = if tls_enabled {
            "Disable TLS"
        } else {
            "Enable TLS"
        };
        column()
            .flex_shrink_0()
            .gap_1p5()
            .child(
                row()
                    .gap_1p5()
                    .child(
                        row()
                            .flex_1()
                            .min_w_0()
                            .h(REQUEST_CONTROL_HEIGHT)
                            .rounded_md()
                            .bg(rgb(SURFACE))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .child(
                                row()
                                    .w(REQUEST_CONTROL_HEIGHT)
                                    .h_full()
                                    .flex_shrink_0()
                                    .justify_center()
                                    .border_r_1()
                                    .border_color(rgb(BORDER))
                                    .child(
                                        Button::new("grpc-tls")
                                            .ghost()
                                            .small()
                                            .compact()
                                            .icon(if tls_enabled {
                                                IconName::Lock
                                            } else {
                                                IconName::LockOpen
                                            })
                                            .accessibility_label("TLS")
                                            .toggled(tls_enabled)
                                            .tooltip(tls_tooltip)
                                            .disabled(state.loading || run.is_some())
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.set_grpc_tls(!tls_enabled, window, cx);
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .capture_action(cx.listener(
                                        |this, action: &Enter, window, cx| {
                                            if action.secondary {
                                                this.send_request(window, cx);
                                            } else {
                                                this.discover_grpc_methods_if_needed(
                                                    false, window, cx,
                                                );
                                            }
                                            cx.stop_propagation();
                                        },
                                    ))
                                    .child(
                                        self.variable_field(
                                            &self.url,
                                            Textarea::new(&self.url)
                                                .small()
                                                .appearance(false)
                                                .bordered(false)
                                                .h(REQUEST_CONTROL_HEIGHT - px(2.))
                                                .aria_label("gRPC server address"),
                                            cx,
                                        ),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .w(relative(0.30))
                            .min_w(px(130.))
                            .max_w(px(240.))
                            .child(
                                Select::new(&self.method)
                                    .small()
                                    .w_full()
                                    .h(REQUEST_CONTROL_HEIGHT)
                                    .placeholder("Select a method")
                                    .accessibility_label("gRPC method")
                                    .disabled(run.is_some() || !has_methods),
                            ),
                    )
                    .child(
                        Button::new("grpc-invoke")
                            .primary()
                            .small()
                            .h(REQUEST_CONTROL_HEIGHT)
                            .label(if run.is_some() { "Send" } else { "Invoke" })
                            .disabled(
                                self.variable_editor.is_some()
                                    || if run.is_some() {
                                        !can_send
                                    } else {
                                        entry.draft.method.is_empty()
                                            || entry.draft.url.trim().is_empty()
                                    },
                            )
                            .on_click(
                                cx.listener(|this, _, window, cx| this.send_request(window, cx)),
                            ),
                    )
                    .when(can_send, |bar| {
                        bar.child(
                            Button::new("grpc-end")
                                .outline()
                                .small()
                                .h(REQUEST_CONTROL_HEIGHT)
                                .label("End Streaming")
                                .on_click(cx.listener(|this, _, _, cx| this.end_grpc_stream(cx))),
                        )
                    })
                    .when(run.is_some(), |bar| {
                        bar.child(
                            Button::new("grpc-cancel")
                                .outline()
                                .small()
                                .h(REQUEST_CONTROL_HEIGHT)
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_grpc_call(cx))),
                        )
                    }),
            )
            .children(
                if state.loading {
                    Some("Loading methods…")
                } else {
                    self.grpc_kind()
                }
                .map(|status| div().text_xs().text_color(rgb(MUTED)).child(status)),
            )
            .children(
                state
                    .error
                    .as_ref()
                    .map(|error| div().text_xs().text_color(rgb(RED)).child(error.clone())),
            )
    }

    fn grpc_service_editor(&self, cx: &Context<Self>) -> Stateful<Div> {
        let entry = self.active();
        let options = entry.draft.grpc.as_ref().expect("gRPC request");
        let state = &entry.grpc_state;
        let disabled = state.loading || state.run.is_some();
        let source = options
            .proto
            .as_ref()
            .and_then(|proto| proto.path.file_name())
            .map_or_else(
                || "Server reflection".into(),
                |name| name.to_string_lossy().into_owned(),
            );
        column()
            .id("grpc-service-definition")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .gap_3()
            .child(div().text_color(rgb(MUTED)).child(source))
            .child(
                row()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("grpc-load-methods")
                            .outline()
                            .label(if state.loading {
                                "Loading…"
                            } else if state.current_schema(&entry.draft).is_some() {
                                "Reload methods"
                            } else {
                                "Load methods"
                            })
                            .disabled(
                                disabled
                                    || (entry.draft.url.trim().is_empty()
                                        && options.proto.is_none()),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.load_grpc_methods(window, cx)
                            })),
                    )
                    .child(
                        Button::new("grpc-import-proto")
                            .outline()
                            .label("Import .proto")
                            .disabled(disabled)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.choose_grpc_proto(window, cx)
                            })),
                    )
                    .when(options.proto.is_some(), |bar| {
                        bar.child(
                            Button::new("grpc-reflection")
                                .outline()
                                .label("Use reflection")
                                .disabled(disabled)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.use_grpc_reflection(window, cx)
                                })),
                        )
                    }),
            )
            .child(
                Checkbox::new("grpc-verify-tls")
                    .label("Verify server certificate")
                    .checked(options.verify_tls)
                    .disabled(disabled || !crate::grpc::uses_tls(&entry.draft.url, options))
                    .on_click(cx.listener(|this, checked: &bool, window, cx| {
                        this.requests[this.selected]
                            .draft
                            .grpc
                            .as_mut()
                            .expect("gRPC request")
                            .verify_tls = *checked;
                        this.sync_grpc_methods(window, cx);
                        cx.notify();
                    })),
            )
            .child(
                Checkbox::new("grpc-include-defaults")
                    .label("Include default response fields")
                    .checked(options.include_default_fields)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.requests[this.selected]
                            .draft
                            .grpc
                            .as_mut()
                            .expect("gRPC request")
                            .include_default_fields = *checked;
                        cx.notify();
                    })),
            )
            .children(
                [
                    ("Server name override", &self.grpc_fields.server_name),
                    ("Deadline (ms)", &self.grpc_fields.timeout),
                    (
                        "Maximum response message (bytes, 0 = unlimited)",
                        &self.grpc_fields.max_response_bytes,
                    ),
                ]
                .into_iter()
                .map(|(label, field)| {
                    column()
                        .gap_1()
                        .flex_shrink_0()
                        .child(div().text_xs().text_color(rgb(MUTED)).child(label))
                        .child(self.variable_field(field, Input::new(field).aria_label(label), cx))
                }),
            )
    }

    fn request_editor(&self, compact: bool, cx: &Context<Self>) -> Div {
        let grpc = self.active().draft.grpc.is_some();
        let graphql = self.active().draft.graphql.is_some();
        let tabs = [
            Some((
                "request-body",
                if grpc {
                    "Message"
                } else if graphql {
                    "Query"
                } else {
                    "Body"
                },
                RequestPane::Body,
            )),
            graphql.then_some((
                "graphql-variables",
                "Variables",
                RequestPane::GraphqlVariables,
            )),
            graphql.then_some(("graphql-schema", "Schema", RequestPane::GraphqlSchema)),
            Some(if grpc {
                (
                    "request-service",
                    "Service definition",
                    RequestPane::ServiceDefinition,
                )
            } else {
                (
                    "request-query",
                    "Params",
                    RequestPane::Pairs(PairKind::Query),
                )
            }),
            Some((
                "request-headers",
                if grpc { "Metadata" } else { "Headers" },
                RequestPane::Pairs(PairKind::Headers),
            )),
            Some(("request-auth", "Authorization", RequestPane::Authorization)),
            Some(("request-scripts", "Scripts", RequestPane::Scripts)),
        ];
        column()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .when(!compact, |panel| {
                panel.child(
                    row()
                        .h(PANE_BAR_HEIGHT)
                        .px_2()
                        .flex_shrink_0()
                        .child(section_label("Request")),
                )
            })
            .child(
                row()
                    .id("request-tabs")
                    .overflow_x_scroll()
                    .px_2()
                    .h(PANE_BAR_HEIGHT)
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .children(tabs.into_iter().flatten().map(|(id, label, pane)| {
                        pane_button(id, label, self.request_pane == pane).on_click(cx.listener(
                            move |this, _, window, cx| {
                                if pane == RequestPane::Pairs(PairKind::Query) {
                                    this.sync_path_variables(window, cx);
                                }
                                this.request_pane = pane;
                                cx.notify();
                            },
                        ))
                    })),
            )
            .child(match self.request_pane {
                RequestPane::Body if graphql => self.graphql_query_editor(cx),
                RequestPane::Body => self.body_editor(cx),
                RequestPane::GraphqlVariables => {
                    column().flex_1().min_h_0().p_2().child(editor_surface(
                        self.variable_field(
                            &self.graphql_fields.variables,
                            Editor::new(&self.graphql_fields.variables)
                                .aria_label("GraphQL variables")
                                .size_full()
                                .bordered(false),
                            cx,
                        ),
                    ))
                }
                RequestPane::GraphqlSchema => self.graphql_schema_editor(cx),
                RequestPane::Pairs(PairKind::Query) => self.query_editor(cx),
                RequestPane::Pairs(kind) => column()
                    .flex_1()
                    .min_h_0()
                    .p_2()
                    .child(self.pairs_editor(kind, true, cx)),
                RequestPane::Authorization => self.authorization_editor(cx),
                RequestPane::Scripts => self.scripts_editor(cx),
                RequestPane::ServiceDefinition => column()
                    .flex_1()
                    .min_h_0()
                    .child(self.grpc_service_editor(cx)),
            })
    }

    fn variables_editor(&self, cx: &Context<Self>) -> Stateful<Div> {
        let editor = self
            .variable_editor
            .as_ref()
            .expect("variable editor is open");
        let (title, description) = match editor.scope {
            VariableScope::Global => (
                "Global variables".to_owned(),
                "Available in every request. Collection variables override globals with the same name.",
            ),
            VariableScope::Environment => (
                "Environment variables".to_owned(),
                "Available in every request and in pm.environment. These values override collection and global variables.",
            ),
            VariableScope::Collection(id) => (
                self.collections
                    .iter()
                    .find(|collection| collection.id == id)
                    .map(|collection| {
                        format!(
                            "{} / Variables",
                            crate::storage::lineage(&self.collections, collection.id)
                                .iter()
                                .map(|group| group.name.as_str())
                                .collect::<Vec<_>>()
                                .join(" / "),
                        )
                    })
                    .unwrap_or_default(),
                "Available to requests in this collection or folder and its subfolders. Values override the parent scope; Environment has higher priority.",
            ),
        };
        column()
            .id("variable-editor")
            .track_focus(&self.variable_editor_focus)
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
                row()
                    .h(px(48.))
                    .px_3()
                    .gap_2()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(icon(IconName::Settings2, 18., ACCENT))
                    .child(
                        div().flex_1().min_w_0().truncate()
                            .font_weight(FontWeight::SEMIBOLD).child(title),
                    )
                    .child(
                        Button::new("cancel-variables")
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| this.close_variables(window, cx))),
                    )
                    .child(
                        Button::new("save-variables")
                            .primary()
                            .label("Save")
                            .disabled(self.storage.is_err())
                            .on_click(cx.listener(|this, _, window, cx| this.save_variables(window, cx))),
                    ),
            )
            .child(
                column()
                    .p_3()
                    .gap_2()
                    .flex_1()
                    .min_h_0()
                    .child(div().text_color(rgb(MUTED)).child(description))
                    .child(
                        div().text_color(rgb(MUTED)).child(
                            "Use {{name}} in the URL, query parameters, headers, body or authorization. Uncheck a variable to disable it.",
                        ),
                    )
                    .children(self.storage.as_ref().err().or(editor.error.as_ref()).map(|error| {
                        div().text_color(rgb(RED)).child(error.clone())
                    }))
                    .child(
                        row().gap_2()
                            .child(div().w(px(28.)).flex_shrink_0())
                            .child(div().flex_1().min_w_0().child(section_label("Name")))
                            .child(div().flex_1().min_w_0().child(section_label("Value")))
                            .child(div().w(px(28.)).flex_shrink_0()),
                    )
                    .child(
                        column()
                            .id("variable-rows")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .gap_2()
                            .pb_1()
                            .children(editor.fields.iter().map(|fields| self.variable_row(fields, cx))),
                    ),
            )
    }

    fn variable_row(&self, fields: &variables::VariableFields, cx: &Context<Self>) -> Div {
        let row_id = fields.name.entity_id();
        row()
            .gap_2()
            .flex_shrink_0()
            .child(
                div().w(px(28.)).flex_shrink_0().child(
                    Checkbox::new(("variable-enabled", row_id))
                        .checked(fields.enabled)
                        .accessibility_label("Enable variable")
                        .on_click(cx.listener(move |this, enabled: &bool, _, cx| {
                            this.set_variable_enabled(row_id, *enabled, cx);
                        })),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&fields.name).small().aria_label("Variable name")),
            )
            .child(
                div().flex_1().min_w_0().child(
                    self.variable_field(
                        &fields.value,
                        Input::new(&fields.value)
                            .small()
                            .aria_label("Variable value"),
                        cx,
                    ),
                ),
            )
            .child(
                Button::new(("remove-variable", row_id))
                    .ghost()
                    .compact()
                    .w(px(28.))
                    .icon(IconName::Close)
                    .accessibility_label("Remove variable")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.remove_variable(row_id, window, cx);
                    })),
            )
    }

    fn graphql_query_editor(&self, cx: &Context<Self>) -> Div {
        let body = self.request_body_editor();
        column()
            .flex_1()
            .min_h_0()
            .p_2()
            .gap_2()
            .child(
                self.variable_field(
                    &self.graphql_fields.operation,
                    Input::new(&self.graphql_fields.operation)
                        .small()
                        .max_w(px(280.))
                        .aria_label("GraphQL operation name"),
                    cx,
                ),
            )
            .child(editor_surface(
                self.variable_field(
                    body,
                    Editor::new(body)
                        .aria_label("GraphQL query")
                        .size_full()
                        .bordered(false),
                    cx,
                ),
            ))
    }

    fn graphql_schema_editor(&self, cx: &Context<Self>) -> Div {
        let entry = self.active();
        let state = &entry.graphql_state;
        let schema = state.result(&entry.draft);
        column()
            .flex_1()
            .min_h_0()
            .p_2()
            .gap_2()
            .child(
                row().gap_2().child(
                    Button::new("load-graphql-schema")
                        .outline()
                        .label(if state.loading {
                            "Loading…"
                        } else if schema.is_some_and(Result::is_ok) {
                            "Reload schema"
                        } else {
                            "Load schema"
                        })
                        .disabled(state.loading || entry.draft.url.trim().is_empty())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.load_graphql_schema(window, cx)),
                        ),
                ),
            )
            .when_some(schema, |panel, result| match result {
                Ok(_) => panel.child(editor_surface(
                    Editor::new(&self.graphql_fields.schema)
                        .aria_label("GraphQL schema")
                        .size_full()
                        .bordered(false)
                        .readonly(true),
                )),
                Err(error) => panel.child(div().text_color(rgb(RED)).child(error.clone())),
            })
    }

    fn body_editor(&self, cx: &Context<Self>) -> Div {
        let body = self.request_body_editor();
        column()
            .flex_1()
            .min_h_0()
            .p_2()
            .gap_2()
            .child(editor_surface(
                self.variable_field(
                    body,
                    Editor::new(body)
                        .aria_label(if self.active().draft.grpc.is_some() {
                            "gRPC message"
                        } else {
                            "Request body"
                        })
                        .size_full()
                        .bordered(false),
                    cx,
                ),
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
                    .when(self.grpc_kind().is_some(), |bar| {
                        bar.child(
                            Button::new("grpc-example")
                                .outline()
                                .compact()
                                .label("Use Example Message")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.use_grpc_example(window, cx);
                                })),
                        )
                    })
                    .child(
                        Button::new("format-request-json")
                            .outline()
                            .compact()
                            .icon(IconName::ListFilter)
                            .accessibility_label("Format JSON")
                            .tooltip("Format JSON")
                            .disabled(self.active().draft.body.trim().is_empty())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.format_request_body(window, cx);
                            })),
                    ),
            )
    }

    fn authorization_editor(&self, cx: &Context<Self>) -> Div {
        let field = |label, state: &Entity<InputState>, secret| {
            column().gap_1().child(section_label(label)).child(
                self.variable_field(
                    state,
                    Input::new(state)
                        .small()
                        .max_w(px(360.))
                        .aria_label(label)
                        .when(secret, |input| input.mask_toggle()),
                    cx,
                ),
            )
        };
        let form = column().flex_1().min_h_0().p_2().gap_3().child(
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

    fn query_editor(&self, cx: &Context<Self>) -> Div {
        let request = &self.active().draft;
        let has_path_variables = request.url[path_range(&request.url)]
            .split('/')
            .any(|segment| {
                path_parameter(segment).is_some()
                    || request.path_variables.iter().any(|variable| {
                        !variable.name.is_empty() && segment == format!("{{{}}}", variable.name)
                    })
            });
        column().flex_1().min_h_0().child(
            column()
                .id("request-parameters")
                .flex_1()
                .min_h_0()
                .p_2()
                .gap_2()
                .when(has_path_variables, |panel| {
                    panel
                        .overflow_y_scroll()
                        .track_scroll(&self.active().query_scroll)
                })
                .child(self.pairs_editor(PairKind::Query, !has_path_variables, cx))
                .when(has_path_variables, |panel| {
                    panel.child(self.pairs_editor(PairKind::Path, false, cx))
                }),
        )
    }

    fn pairs_editor(&self, kind: PairKind, scroll_rows: bool, cx: &Context<Self>) -> Div {
        let (name_label, value_label, remove_label) = match kind {
            PairKind::Query => ("Parameter name", "Parameter value", "Remove parameter"),
            PairKind::Headers => ("Header name", "Header value", "Remove header"),
            PairKind::Path => (
                "Path variable name",
                "Path variable value",
                "Remove path variable",
            ),
        };
        let fields = self.pair_fields(kind);
        let field_count = fields.len();
        let scroll = if kind == PairKind::Headers {
            &self.active().headers_scroll
        } else {
            &self.active().query_scroll
        };
        let column_gutter = || div().w(px(18.)).flex_shrink_0();
        column()
            .when_else(scroll_rows, |panel| panel.flex_1().min_h_0(), |panel| panel.flex_shrink_0())
            .gap_1p5()
            .when(kind == PairKind::Query, |panel| {
                panel.child(section_label("Query parameters").flex_shrink_0())
            })
            .when(kind == PairKind::Path, |panel| {
                panel.child(section_label("Path variables")).child(
                    div().text_xs().text_color(rgb(MUTED))
                        .child("Use :name in the URL path. Values belong to this request and can contain {{variables}}."),
                )
            })
            .child(
                column()
                    .w_full().max_w(PAIR_TABLE_MAX_WIDTH)
                    .when_else(scroll_rows, |table| {
                        table.flex_1().min_h_0().max_h(PAIR_HEADER_HEIGHT + PAIR_ROW_HEIGHT * field_count as f32 + px(2.))
                    }, |table| table.flex_shrink_0())
                    .rounded_lg().border_1().border_color(rgb(BORDER)).overflow_hidden().bg(rgb(EDITOR))
                    .when(!fields.is_empty(), |panel| {
                        panel.child(
                            row().h(PAIR_HEADER_HEIGHT).px_2().gap_1p5().flex_shrink_0()
                                .bg(rgb(SURFACE)).border_b_1().border_color(rgb(BORDER))
                                .text_xs().text_color(rgb(MUTED)).font_weight(FontWeight::SEMIBOLD)
                                .child(column_gutter())
                                .child(div().flex_1().min_w_0().px_2().child("Name"))
                                .child(div().flex_1().min_w_0().px_2().child("Value"))
                                .child(div().w(px(24.)).flex_shrink_0()),
                        )
                    })
                    .child(
                        column()
                            .id(match kind { PairKind::Query => "query-rows", PairKind::Headers => "header-rows", PairKind::Path => "path-rows" })
                            .when_else(scroll_rows, |rows| rows.flex_1().min_h_0().overflow_y_scroll().track_scroll(scroll), |rows| rows.flex_shrink_0())
                            .children(fields.iter().enumerate().map(|(index, fields)| {
                                let row_id = fields.name.entity_id();
                                row()
                                    .h(PAIR_ROW_HEIGHT).px_2().gap_1p5().flex_shrink_0()
                                    .when(index + 1 < field_count, |row| row.border_b_1().border_color(rgb(BORDER)))
                                    .child(column_gutter().child(
                                        Checkbox::new(("pair-enabled", row_id))
                                            .small()
                                            .checked(fields.enabled)
                                            .accessibility_label(match kind {
                                                PairKind::Query => "Send query parameter",
                                                PairKind::Headers => "Send header",
                                                PairKind::Path => "Substitute path variable",
                                            })
                                            .on_click(cx.listener(move |this, enabled: &bool, window, cx| {
                                                this.set_pair_enabled(kind, row_id, *enabled, window, cx);
                                            })),
                                    ))
                                    .child(div().flex_1().min_w_0().child(self.variable_field(
                                        &fields.name, Input::new(&fields.name).small().aria_label(name_label), cx,
                                    )))
                                    .child(div().flex_1().min_w_0().child(self.variable_field(
                                        &fields.value, Input::new(&fields.value).small().aria_label(value_label), cx,
                                    )))
                                    .child(Button::new(("remove-pair", row_id)).ghost().small().compact().w(px(24.))
                                        .flex_shrink_0().icon(IconName::Close).accessibility_label(remove_label)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.remove_pair(kind, row_id, window, cx)
                                        })))
                            })),
                    ),
            )
    }

    fn scripts_editor(&self, cx: &Context<Self>) -> Div {
        let grpc = self.active().draft.grpc.is_some();
        let (editor, label) = match self.script_phase {
            ScriptPhase::Pre => (
                &self.pre_script,
                if grpc { "Before invoke" } else { "Pre-request" },
            ),
            ScriptPhase::OnMessage => (&self.on_message_script, "On message"),
            ScriptPhase::Post => (
                &self.post_script,
                if grpc {
                    "After response"
                } else {
                    "Post-response"
                },
            ),
        };
        column().flex_1().min_h_0().p_2().gap_2()
            .child(row().gap_1()
                .children([
                    ("pre-request-script", if grpc { "Before invoke" } else { "Pre-request" }, ScriptPhase::Pre),
                    ("grpc-message-script", "On message", ScriptPhase::OnMessage),
                    ("post-response-script", if grpc { "After response" } else { "Post-response" }, ScriptPhase::Post),
                ].into_iter()
                    .filter(|(_, _, phase)| grpc || *phase != ScriptPhase::OnMessage)
                    .map(|(id, label, phase)| pane_button(id, label, self.script_phase == phase)
                        .on_click(cx.listener(move |this, _, _, cx| { this.script_phase = phase; cx.notify(); }))))
                .child(div().flex_1())
                .child(Button::new("insert-script-example").ghost().compact().label("Example")
                    .disabled(!editor.read(cx).value().is_empty())
                    .on_click(cx.listener(|this, _, window, cx| this.insert_script_example(window, cx)))))
            .child(editor_surface(self.variable_field(editor, Editor::new(editor).aria_label(label).size_full().bordered(false), cx)))
            .child(div().text_xs().text_color(rgb(MUTED)).child(match self.script_phase {
                ScriptPhase::Pre => "JavaScript · Runs before variable substitution and sending. Results appear in Tests and Console.",
                ScriptPhase::OnMessage => "JavaScript · Runs for each incoming message. Use pm.message.data.",
                ScriptPhase::Post => "JavaScript · Runs after the response. Use pm.response, pm.test and pm.expect.",
            }))
    }

    fn script_results(&self, console: bool) -> impl IntoElement {
        let report = &self.active().script_report;
        let passed = report.tests.iter().filter(|test| test.passed).count();
        column()
            .id("script-results")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_2()
            .gap_2()
            .when(self.active().response.is_sending(), |panel| {
                panel.child(
                    div()
                        .text_color(rgb(MUTED))
                        .child("Running request and scripts…"),
                )
            })
            .children(
                report
                    .errors
                    .iter()
                    .map(|error| div().text_color(rgb(RED)).child(error.clone())),
            )
            .when(console, |panel| {
                panel
                    .children(report.logs.iter().map(|log| {
                        column()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .child(format!("{} · {}", log.phase, log.level)),
                            )
                            .child(
                                div()
                                    .font_family("monospace")
                                    .text_color(rgb(match log.level.as_str() {
                                        "error" => RED,
                                        "warn" => AMBER,
                                        _ => TEXT,
                                    }))
                                    .child(log.message.clone()),
                            )
                    }))
                    .when(report.logs.is_empty(), |panel| {
                        panel.child(
                            div()
                                .text_color(rgb(MUTED))
                                .child("No console output. Use console.log() in a script."),
                        )
                    })
            })
            .when(!console, |panel| {
                panel
                    .child(
                        div()
                            .text_color(rgb(MUTED))
                            .child(format!("{passed}/{} tests passed", report.tests.len())),
                    )
                    .children(report.tests.iter().map(|test| {
                        column()
                            .gap_1()
                            .p_2()
                            .rounded_md()
                            .bg(rgb(SURFACE))
                            .child(
                                div()
                                    .text_color(rgb(if test.passed { GREEN } else { RED }))
                                    .child(format!(
                                        "{} · {}",
                                        if test.passed { "PASS" } else { "FAIL" },
                                        test.name
                                    )),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .child(test.phase.clone()),
                            )
                            .children(
                                test.error
                                    .clone()
                                    .map(|error| div().text_color(rgb(RED)).child(error)),
                            )
                    }))
                    .when(report.tests.is_empty(), |panel| {
                        panel.child(
                            div().text_color(rgb(MUTED)).child(
                                "Add pm.test() assertions in Scripts, then send the request.",
                            ),
                        )
                    })
            })
    }

    fn response_view(&self, cx: &Context<Self>) -> impl IntoElement {
        let entry = self.active();
        let response = &entry.response;
        let body = self.response_pane == Pane::Body;
        let finished = match response {
            RequestResponse::Finished { response, .. } => Some(response),
            _ => None,
        };
        let received = finished.is_some();
        let grpc = entry.draft.grpc.is_some();
        let streaming = entry.grpc_state.streaming;
        let live = response.is_sending() && grpc;
        let tabs = [
            Some(("response-body", "Body", Pane::Body, received)),
            streaming.then_some(("grpc-messages", "Messages", Pane::Messages, true)),
            Some((
                "response-headers",
                if grpc { "Metadata" } else { "Headers" },
                Pane::Headers,
                received || live,
            )),
            grpc.then_some(("grpc-trailers", "Trailers", Pane::Trailers, received)),
            Some((
                "response-visualizer",
                "Visualizer",
                Pane::Visualizer,
                received,
            )),
        ];
        column()
            .id("response-panel")
            .role(Role::Region)
            .aria_label("Response")
            .track_focus(&self.response_focus)
            .tab_group()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
                row().h(PANE_BAR_HEIGHT).px_2().gap_1p5().flex_shrink_0()
                    .child(section_label("Response"))
                    .child(div().flex_1())
                    .children(finished.map(|response| response_metadata(response, grpc))),
            )
            .child(
                row()
                    .id("response-tabs")
                    .overflow_x_scroll()
                    .px_2()
                    .h(PANE_BAR_HEIGHT)
                    .flex_shrink_0()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .children(tabs.into_iter().flatten().map(|(id, label, pane, enabled)| {
                        pane_button(id, label, self.response_pane == pane)
                            .disabled(!enabled)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_response_pane(pane, window, cx);
                            }))
                    })),
            )
            .child(match (self.response_pane, response) {
                (Pane::Visualizer, RequestResponse::Finished { .. }) => {
                    if let Some(error) = &self.visualizer_error {
                        empty_panel(icon(IconName::CircleAlert, 20., RED), "Visualizer failed", error.clone())
                    } else if let Some(view) = self.visualizer.as_ref().filter(|_| entry.script_report.visualization.is_some()) {
                        column().flex_1().min_h_0().child(view.clone())
                    } else {
                        empty_panel(icon(IconName::Code, 20., ACCENT), "No visualization yet", "Call pm.visualizer.set(template, data) in a request script, then send it again.")
                    }
                },
                (Pane::Tests, _) => column().flex_1().min_h_0().child(self.script_results(false)),
                (Pane::Console, _) => column().flex_1().min_h_0().child(self.script_results(true)),
                (_, RequestResponse::Idle) => empty_panel(
                    icon(IconName::Inbox, 20., ACCENT),
                    "No response yet",
                    "Send a request to see the response here.",
                ),
                (Pane::Messages | Pane::Headers, RequestResponse::Sending { .. }) if live => column()
                    .flex_1().min_h_0().p_2().gap_2()
                    .child(row().gap_2().child(Spinner::new()).child("Streaming"))
                    .child(editor_surface(Editor::new(&self.response_details).size_full().bordered(false).readonly(true))),
                (_, RequestResponse::Sending { .. }) => empty_panel(
                    Spinner::new(), "Waiting for response", "The request is in progress.",
                ),
                (_, RequestResponse::Failed(error)) => empty_panel(
                    icon(IconName::CircleAlert, 20., RED), "Request failed", error.clone(),
                ).child(
                    Button::new("retry-request")
                        .outline()
                        .small()
                        .label(if grpc { "Invoke again" } else { "Send again" })
                        .on_click(cx.listener(|this, _, window, cx| this.send_request(window, cx))),
                ),
                (Pane::Body, RequestResponse::Finished { response, .. }) if response.body.is_empty() => {
                    empty_panel(icon(IconName::Inbox, 20., MUTED), "No response body", "The server returned an empty body.")
                }
                (_, RequestResponse::Finished { response, .. }) => column()
                    .flex_1().min_h_0().p_2()
                    .child(
                        editor_surface(
                            column().size_full().min_h_0()
                                .when(body, |panel| panel.child(row().h(px(26.)).px_2().flex_shrink_0()
                                    .border_b_1().border_color(rgb(BORDER)).bg(rgb(SURFACE))
                                    .text_xs().text_color(rgb(MUTED)).font_weight(FontWeight::SEMIBOLD)
                                    .child(if response.is_json { "JSON" } else { "Text" })
                                ))
                                .child(div().flex_1().min_h_0().min_w_0().child(
                                    Editor::new(self.response_editor())
                                        .size_full().bordered(false).readonly(true)
                                        .aria_label(match self.response_pane {
                                            Pane::Body => "Response body",
                                            Pane::Messages => "Stream messages",
                                            Pane::Trailers => "Response trailers",
                                            Pane::Headers if grpc => "Response metadata",
                                            _ => "Response headers",
                                        })
                                        .when(!body, |editor| editor.text_color(rgb(0xb8cbe9))),
                                )),
                        )
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.open_response_link(event, cx);
                        })),
                    ),
            })
            .child(
                row()
                    .h(PANE_BAR_HEIGHT)
                    .px_2()
                    .gap_1()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .children(
                        [
                            ("response-tests", "Tests", IconName::FlaskConical, Pane::Tests),
                            ("response-console", "Console", IconName::Terminal, Pane::Console),
                        ]
                        .map(|(id, label, icon, pane)| {
                            pane_icon_button(id, label, icon, self.response_pane == pane)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.select_response_pane(pane, window, cx);
                                }))
                        }),
                    ),
            )
    }
}

pub(super) fn status_badge(label: impl Into<SharedString>, color: u32, background: u32) -> Div {
    row()
        .gap_1()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(background))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(color))
        .child(
            div()
                .size(px(4.))
                .rounded_full()
                .bg(rgb(color))
                .flex_shrink_0(),
        )
        .child(div().min_w_0().truncate().child(label.into()))
}

fn response_status(response: &Response, grpc: bool) -> Div {
    let (color, background) = if grpc {
        if response.status == 0 {
            (GREEN, SELECTED)
        } else {
            (RED, ERROR_BG)
        }
    } else {
        match response.status {
            200..=299 => (GREEN, SELECTED),
            400..=599 => (RED, ERROR_BG),
            _ => (AMBER, AMBER_BG),
        }
    };
    let status = format!("{} {}", response.status, response.reason);
    status_badge(status.trim_end().to_owned(), color, background)
}

fn response_metadata(response: &Response, grpc: bool) -> Div {
    row()
        .min_w_0()
        .gap_2()
        .text_xs()
        .text_color(rgb(MUTED))
        .child(response_status(response, grpc))
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
        .p_3()
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

fn editor_surface(editor: impl IntoElement) -> Div {
    div()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .rounded_lg()
        .overflow_hidden()
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(EDITOR))
        .child(editor)
}

fn pane_button(id: &'static str, label: &'static str, selected: bool) -> Button {
    Button::new(id)
        .ghost()
        .small()
        .compact()
        .label(label)
        .flex_shrink_0()
        .bg(rgb(if selected { SELECTED } else { BACKGROUND }))
        .text_color(rgb(if selected { ACCENT } else { MUTED }))
}

fn pane_icon_button(id: &'static str, label: &'static str, icon: IconName, selected: bool) -> Button {
    Button::new(id)
        .ghost()
        .small()
        .compact()
        .flex_shrink_0()
        .icon(icon)
        .accessibility_label(label)
        .tooltip(label)
        .selected(selected)
}

fn section_label(label: &'static str) -> Div {
    div()
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
}

pub(super) fn method_color(method: &str) -> u32 {
    match method {
        "GET" => GREEN,
        "POST" => AMBER,
        "PUT" => 0x8cb9f6,
        "PATCH" | "gRPC" | "GQL" => VIOLET,
        "DELETE" => RED,
        _ => MUTED,
    }
}

pub(super) fn icon(name: IconName, size: f32, color: u32) -> Icon {
    Icon::from(name).size(px(size)).text_color(rgb(color))
}

pub(super) fn row() -> Div {
    div().flex().items_center()
}

pub(super) fn column() -> Div {
    div().flex().flex_col()
}
