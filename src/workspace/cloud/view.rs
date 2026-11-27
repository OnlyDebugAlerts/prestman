use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Disableable, Selectable, Side, Sizable,
    button::{Button, ButtonVariants},
    input::{Enter, Escape, Input},
    menu::{DropdownMenu, PopupMenuItem},
    spinner::Spinner,
};
use gpui_kit::prelude::FluentBuilder as _;

use super::*;
use crate::workspace::view::{
    ACCENT, AMBER, AMBER_BG, BACKGROUND, BORDER, EDITOR, ERROR_BG, GREEN, MUTED, RED, SELECTED,
    SURFACE, VIOLET, VIOLET_BG, column, icon, row, status_badge,
};

const CLOUD_PAGE_WIDTH: Pixels = px(1088.);

impl Workspace {
    pub(in crate::workspace) fn workspace_switcher(&self, cx: &Context<Self>) -> impl IntoElement {
        let workspace = cx.weak_entity();
        let current = self.cloud.mounted.as_ref().map(|mounted| &mounted.team);
        let current_id = current.map(|team| team.id);
        let current_name = current.map_or("Local workspace", |team| team.name.as_str());
        let signed_in = self.cloud.api.is_some();
        let mut teams = self.cloud.teams.clone();
        if let Some(current) = current
            && let Some(team) = teams.iter_mut().find(|team| team.id == current.id)
        {
            team.role = current.role;
        }
        teams.sort_by_cached_key(|team| team.name.to_lowercase());
        let storage_unavailable = self.storage.is_err();
        Button::new("workspace-switcher")
            .outline()
            .small()
            .compact()
            .icon(IconName::PanelTop)
            .dropdown_caret(true)
            .accessibility_label(format!("Switch workspace, current: {current_name}"))
            .tooltip("Switch workspace")
            .disabled(self.cloud.panel.as_ref().is_some_and(Panel::is_busy))
            .child(
                div()
                    .max_w(px(140.))
                    .truncate()
                    .child(current_name.to_owned()),
            )
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu
                    .min_w(px(272.))
                    .max_w(px(370.))
                    .max_h(px(384.))
                    .scrollable(true)
                    .check_side(Side::Right)
                    .label("On this device")
                    .item(
                        workspace_menu_item(
                            "Local workspace",
                            IconName::Monitor,
                            &workspace,
                            Workspace::cloud_return_local,
                        )
                        .checked(current_id.is_none()),
                    );
                if signed_in && !teams.is_empty() {
                    menu = menu.separator().label("Team");
                    for team in &teams {
                        let selected = current_id == Some(team.id);
                        let label = format!("{}  ·  {}", team.name, team.role.label());
                        let team = team.clone();
                        menu = menu.item(
                            workspace_menu_item(
                                label,
                                IconName::Users,
                                &workspace,
                                move |view, window, cx| {
                                    view.cloud_switch_team(team.clone(), window, cx)
                                },
                            )
                            .checked(selected)
                            .disabled(storage_unavailable && !selected),
                        );
                    }
                }
                menu.separator().item(workspace_menu_item(
                    if signed_in {
                        "Manage workspaces"
                    } else {
                        "Connect to Cloud"
                    },
                    IconName::Cloud,
                    &workspace,
                    |view, window, cx| view.show_cloud(CloudPage::Directory, window, cx),
                ))
            })
    }

    pub(in crate::workspace) fn team_status(&self, cx: &Context<Self>) -> Option<Div> {
        let mounted = self.cloud.mounted.as_ref()?;
        let generation = mounted.generation;
        let conflicts = mounted.cache.conflicts();
        let pending = mounted.cache.pending.len();
        let queued_detail = format!(
            "{pending} {} saved on this device.",
            if pending == 1 { "change" } else { "changes" },
        );
        let (title, detail, foreground, background) = if mounted.unreachable {
            (
                "Offline",
                if pending > 0 {
                    format!("{queued_detail} Sync resumes when the server is reachable.")
                } else {
                    "Sync resumes when the server is reachable.".into()
                },
                AMBER,
                AMBER_BG,
            )
        } else if !conflicts.is_empty() {
            (
                "Resolve conflicts to sync changes",
                "Compare the changes below before replacing or copying a version.".into(),
                RED,
                ERROR_BG,
            )
        } else if let Some(error) = &mounted.sync_error {
            ("Sync unavailable", error.clone(), RED, ERROR_BG)
        } else if !mounted.team.role.can_edit() {
            (
                "Read-only access",
                "Run shared requests and save your changes as local drafts.".into(),
                VIOLET,
                VIOLET_BG,
            )
        } else if pending > 0 {
            ("Syncing", queued_detail, AMBER, AMBER_BG)
        } else {
            ("Synced", String::new(), GREEN, SELECTED)
        };
        Some(
            column()
                .mx_3()
                .mt_2()
                .mb_1()
                .p_2()
                .gap_1()
                .min_w_0()
                .rounded_md()
                .border_1()
                .border_color(rgb(BORDER))
                .text_xs()
                .bg(rgb(SURFACE))
                .child(
                    row()
                        .gap_2()
                        .min_w_0()
                        .child(role_badge(mounted.team.role))
                        .child(status_badge(title, foreground, background))
                        .when(!detail.is_empty(), |row| {
                            row.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_color(rgb(MUTED))
                                    .child(detail),
                            )
                        })
                        .when(
                            mounted.sync_error.is_some() && conflicts.is_empty(),
                            |row| {
                                row.child(
                                    Button::new("retry-workspace-sync")
                                        .ghost()
                                        .compact()
                                        .label("Retry sync")
                                        .disabled(mounted.syncing)
                                        .on_click(cx.listener(move |view, _, window, cx| {
                                            view.sync_team(generation, window, cx);
                                        })),
                                )
                            },
                        ),
                )
                .children(conflicts.into_iter().map(|id| {
                    let content = mounted
                        .cache
                        .pending
                        .get(&id)
                        .and_then(|change| change.content.as_ref());
                    let label = content
                        .or_else(|| mounted.cache.remote.get(&id).map(|item| &item.content))
                        .map_or("Workspace item", Content::name)
                        .to_owned();
                    let can_copy = mounted.team.role.can_edit()
                        && matches!(content, Some(Content::Request { .. }));
                    row()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .child(format!("Conflicting change · {label}")),
                        )
                        .child(
                            Button::new(SharedString::from(format!("cloud-compare-{id}")))
                                .ghost()
                                .compact()
                                .label("Compare changes")
                                .on_click(cx.listener(move |view, _, window, cx| {
                                    view.set_cloud_comparison(Some(id), window, cx);
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("cloud-use-remote-{id}")))
                                .ghost()
                                .compact()
                                .label("Use cloud version")
                                .on_click(cx.listener(move |view, _, window, cx| {
                                    view.resolve_cloud_conflict(id, false, window, cx);
                                })),
                        )
                        .when(can_copy, |row| {
                            row.child(
                                Button::new(SharedString::from(format!("cloud-save-copy-{id}")))
                                    .ghost()
                                    .compact()
                                    .label("Save as copy")
                                    .on_click(cx.listener(move |view, _, window, cx| {
                                        view.resolve_cloud_conflict(id, true, window, cx);
                                    })),
                            )
                        })
                })),
        )
    }

    pub(in crate::workspace) fn cloud_comparison(&self, cx: &Context<Self>) -> Option<Div> {
        let mounted = self.cloud.mounted.as_ref()?;
        let id = mounted.compare?;
        let remote = mounted.cache.remote.get(&id).map(|item| &item.content);
        let local = mounted
            .cache
            .pending
            .get(&id)
            .and_then(|change| change.content.as_ref());
        let name = local.or(remote).map_or("Workspace item", Content::name);
        let can_copy =
            mounted.team.role.can_edit() && matches!(local, Some(Content::Request { .. }));
        let card = |label, content: Option<&Content>| {
            let text = content
                .and_then(|content| serde_json::to_string_pretty(content).ok())
                .unwrap_or_else(|| "Item deleted".into());
            column()
                .flex_1()
                .min_w_0()
                .gap_2()
                .child(div().font_weight(FontWeight::SEMIBOLD).child(label))
                .child(
                    div()
                        .p_3()
                        .bg(rgb(EDITOR))
                        .font_family("monospace")
                        .text_xs()
                        .child(text),
                )
        };
        Some(
            column()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .child(
                    row()
                        .gap_2()
                        .px_4()
                        .py_3()
                        .flex_wrap()
                        .border_b_1()
                        .border_color(rgb(BORDER))
                        .child(
                            column()
                                .gap_1()
                                .flex_1()
                                .min_w_0()
                                .child(heading("Conflicting change"))
                                .child(muted(name.to_owned())),
                        )
                        .child(
                            Button::new("close-cloud-comparison")
                                .ghost()
                                .label("Cancel")
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.set_cloud_comparison(None, window, cx)
                                })),
                        )
                        .child(
                            Button::new("compare-use-cloud-version")
                                .outline()
                                .label("Use cloud version")
                                .on_click(cx.listener(move |view, _, window, cx| {
                                    view.resolve_cloud_conflict(id, false, window, cx);
                                })),
                        )
                        .when(can_copy, |row| {
                            row.child(
                                Button::new("compare-save-as-copy")
                                    .primary()
                                    .label("Save as copy")
                                    .on_click(cx.listener(move |view, _, window, cx| {
                                        view.resolve_cloud_conflict(id, true, window, cx);
                                    })),
                            )
                        }),
                )
                .children(self.save_error.as_ref().map(|error| {
                    div().mx_4().mt_2().child(notice(
                        IconName::CircleAlert,
                        RED,
                        ERROR_BG,
                        error.clone(),
                    ))
                }))
                .child(
                    row()
                        .id("cloud-compare-scroll")
                        .flex_1()
                        .min_h_0()
                        .items_start()
                        .overflow_y_scroll()
                        .gap_3()
                        .p_3()
                        .child(card("On this device", local))
                        .child(card("Cloud version", remote)),
                ),
        )
    }

    pub(in crate::workspace) fn cloud_panel(&self, cx: &Context<Self>) -> Option<Div> {
        let panel = self.cloud.panel.as_ref()?;
        let active = self.cloud.mounted.as_ref();
        let team_page = active.filter(|_| !panel.browsing_workspaces);
        let auth_page = self.cloud.api.is_none();
        let content = if let Some(editor) = &panel.server_editor {
            self.cloud_server_page(panel, editor, cx)
        } else if matches!(panel.job, Some(PanelJob::Restore)) {
            card()
                .items_center()
                .py_10()
                .child(Spinner::new())
                .child(heading("Restoring Cloud session"))
        } else if auth_page {
            self.cloud_auth_page(panel, cx)
        } else if let Some(mounted) = team_page {
            self.cloud_team_page(panel, mounted, cx)
        } else {
            self.cloud_workspaces_page(panel, cx)
        };

        let header = row()
            .w_full()
            .max_w(CLOUD_PAGE_WIDTH)
            .min_h(px(58.))
            .gap_3()
            .child(symbol(if team_page.is_some() {
                IconName::Users
            } else {
                IconName::Layers
            }))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(if auth_page {
                "Cloud connection"
            } else if team_page.is_some() {
                "Team access"
            } else {
                "Workspace directory"
            }))
            .when(team_page.is_some(), |header| {
                header.child(
                    Button::new("cloud-all-workspaces")
                        .ghost()
                        .compact()
                        .icon(IconName::ArrowLeft)
                        .label("All workspaces")
                        .disabled(panel.is_busy())
                        .on_click(
                            cx.listener(|view, _, window, cx| view.cloud_browse(true, window, cx)),
                        ),
                )
            })
            .child(div().flex_1())
            .when(
                panel.is_busy() && !matches!(panel.job, Some(PanelJob::Restore)),
                |header| {
                    header.child(
                        row()
                            .gap_2()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(Spinner::new())
                            .child("Working…"),
                    )
                },
            )
            .when(!auth_page, |header| {
                header.child(
                    Button::new("cloud-sign-out")
                        .ghost()
                        .compact()
                        .label("Sign out")
                        .disabled(panel.is_busy())
                        .on_click(
                            cx.listener(|view, _, window, cx| view.cloud_sign_out(window, cx)),
                        ),
                )
            })
            .child(
                Button::new("close-cloud")
                    .ghost()
                    .compact()
                    .icon(IconName::Close)
                    .tooltip("Back to requests (Esc)")
                    .accessibility_label("Back to requests")
                    .disabled(!panel.can_close())
                    .on_click(cx.listener(|view, _, window, cx| view.close_cloud(window, cx))),
            );

        Some(
            column()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .bg(rgb(BACKGROUND))
                .key_context(CLOUD_CONTEXT)
                .track_focus(&panel.focus)
                .capture_action(cx.listener(|view, _: &Escape, window, cx| {
                    cx.stop_propagation();
                    if view
                        .cloud
                        .panel
                        .as_ref()
                        .is_some_and(|panel| panel.workspace_form.is_some() && !panel.is_busy())
                    {
                        view.cloud_workspace_form(None, window, cx);
                    } else {
                        view.close_cloud(window, cx);
                    }
                }))
                .child(
                    div()
                        .flex()
                        .justify_center()
                        .flex_shrink_0()
                        .px_5()
                        .bg(rgb(SURFACE))
                        .border_b_1()
                        .border_color(rgb(BORDER))
                        .child(header),
                )
                .child(
                    column()
                        .id("cloud-panel")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .items_center()
                        .px_4()
                        .py_4()
                        .child(
                            column()
                                .w_full()
                                .max_w(if auth_page {
                                    px(420.)
                                } else {
                                    CLOUD_PAGE_WIDTH
                                })
                                .flex_shrink_0()
                                .gap_4()
                                .children(panel.error.as_ref().map(|message| {
                                    notice(IconName::CircleAlert, RED, ERROR_BG, message.clone())
                                }))
                                .children(panel.message.as_ref().map(|message| {
                                    notice(IconName::CircleCheck, GREEN, SELECTED, message.clone())
                                }))
                                .child(content),
                        ),
                ),
        )
    }

    fn cloud_server_page(&self, panel: &Panel, editor: &ServerEditor, cx: &Context<Self>) -> Div {
        card()
            .child(page_intro(
                "Connect to Cloud",
                "Connect to your team's server to open shared collections and requests.",
            ))
            .child(
                column()
                    .gap_3()
                    .capture_action(cx.listener(|view, _: &Enter, window, cx| {
                        cx.stop_propagation();
                        view.cloud_save_server(window, cx);
                    }))
                    .child(field("Server URL", &editor.url, false))
                    .when_else(
                        editor.show_key,
                        |form| form.child(field("Public API key", &editor.key, false)),
                        |form| {
                            form.child(
                                row().child(
                                    Button::new("cloud-show-server-key")
                                        .ghost()
                                        .compact()
                                        .label("Add public API key")
                                        .tooltip("For managed Supabase and older Prestman servers")
                                        .on_click(cx.listener(|view, _, window, cx| {
                                            if let Some(editor) = view
                                                .cloud
                                                .panel
                                                .as_mut()
                                                .and_then(|panel| panel.server_editor.as_mut())
                                            {
                                                editor.show_key = true;
                                                editor
                                                    .key
                                                    .read(cx)
                                                    .focus_handle(cx)
                                                    .focus(window, cx);
                                                cx.notify();
                                            }
                                        })),
                                ),
                            )
                        },
                    ),
            )
            .child(
                row()
                    .gap_2()
                    .justify_end()
                    .when(panel.server.is_some(), |row| {
                        row.child(
                            Button::new("cloud-cancel-server-edit")
                                .ghost()
                                .label("Cancel")
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.cloud_cancel_server_edit(window, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new("cloud-save-server")
                            .primary()
                            .label("Save server")
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.cloud_save_server(window, cx)
                            })),
                    ),
            )
    }

    fn cloud_auth_page(&self, panel: &Panel, cx: &Context<Self>) -> Div {
        let busy = panel.is_busy();
        let signup = panel.signup;
        let action = if signup { "Create account" } else { "Sign in" };
        card()
            .child(page_intro(
                action,
                "Access the workspaces you belong to on this server.",
            ))
            .child(
                column()
                    .gap_3()
                    .capture_action(cx.listener(move |view, _: &Enter, window, cx| {
                        cx.stop_propagation();
                        view.cloud_sign_in(signup, window, cx);
                    }))
                    .child(field("Email", &panel.email, busy))
                    .child(field("Password", &panel.password, busy)),
            )
            .child(
                Button::new("cloud-auth-submit")
                    .primary()
                    .label(action)
                    .w_full()
                    .disabled(busy)
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.cloud_sign_in(signup, window, cx)
                    })),
            )
            .child(
                row()
                    .justify_center()
                    .gap_1()
                    .text_sm()
                    .child(div().text_color(rgb(MUTED)).child(if signup {
                        "Already have an account?"
                    } else {
                        "New to this server?"
                    }))
                    .child(
                        Button::new("cloud-auth-toggle")
                            .ghost()
                            .compact()
                            .disabled(busy)
                            .label(if signup { "Sign in" } else { "Create account" })
                            .on_click(cx.listener(|view, _, window, cx| {
                                let Some(panel) = &mut view.cloud.panel else {
                                    return;
                                };
                                panel.signup = !panel.signup;
                                panel.error = None;
                                panel.message = None;
                                panel.email.read(cx).focus_handle(cx).focus(window, cx);
                                cx.notify();
                            })),
                    ),
            )
            .children(panel.server.as_ref().map(|server| {
                row()
                    .gap_2()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .pt_3()
                    .child(icon(IconName::Server, 15., MUTED))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(server.url.clone()),
                    )
                    .child(
                        Button::new("cloud-change-server")
                            .ghost()
                            .compact()
                            .label("Change server")
                            .disabled(busy)
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.cloud_edit_server(window, cx)
                            })),
                    )
            }))
    }

    fn cloud_workspaces_page(&self, panel: &Panel, cx: &Context<Self>) -> Div {
        let busy = panel.is_busy();
        let publishing = panel.publication.is_some();
        let current = self.cloud.mounted.as_ref().map(|mounted| mounted.team.id);
        let mut teams: Vec<_> = self.cloud.teams.iter().collect();
        teams.sort_by(|left, right| {
            (Some(right.id) == current)
                .cmp(&(Some(left.id) == current))
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.id.cmp(&right.id))
        });
        let count = teams.len();
        column()
            .gap_4()
            .children(
                panel
                    .publication
                    .as_ref()
                    .map(|publication| self.cloud_publication(publication, busy, cx)),
            )
            .child(
                row()
                    .flex_wrap()
                    .gap_3()
                    .justify_between()
                    .child(page_intro(
                        if publishing {
                            "Choose a workspace"
                        } else {
                            "Workspaces"
                        },
                        if publishing {
                            "Select a workspace where you can edit requests."
                        } else {
                            "Each workspace keeps its own collections and open requests."
                        },
                    ))
                    .child(
                        row()
                            .gap_2()
                            .child(
                                Button::new("cloud-refresh")
                                    .ghost()
                                    .icon(IconName::RefreshCw)
                                    .compact()
                                    .tooltip("Refresh workspaces")
                                    .accessibility_label("Refresh workspaces")
                                    .disabled(busy)
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        view.cloud_refresh(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("cloud-show-join")
                                    .outline()
                                    .label("Join with code")
                                    .disabled(busy)
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        view.cloud_workspace_form(
                                            Some(WorkspaceForm::Join),
                                            window,
                                            cx,
                                        )
                                    })),
                            )
                            .child(
                                Button::new("cloud-show-create")
                                    .primary()
                                    .icon(IconName::Plus)
                                    .label("New workspace")
                                    .disabled(busy)
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        view.cloud_workspace_form(
                                            Some(WorkspaceForm::Create),
                                            window,
                                            cx,
                                        )
                                    })),
                            ),
                    ),
            )
            .children(
                panel
                    .workspace_form
                    .map(|form| self.cloud_workspace_form_view(panel, form, cx)),
            )
            .when(!publishing, |page| {
                page.child(section_heading("Local"))
                    .child(
                        list_card().child(
                            list_row(0)
                                .child(symbol(IconName::Monitor))
                                .child(workspace_identity(
                                    "Local workspace",
                                    "Collections and drafts on this device",
                                ))
                                .when(current.is_none(), |row| {
                                    row.child(status_badge("Current", GREEN, SELECTED))
                                })
                                .child(
                                    Button::new("cloud-open-local")
                                        .outline()
                                        .compact()
                                        .label("Open")
                                        .accessibility_label("Open local workspace")
                                        .disabled(busy)
                                        .on_click(cx.listener(|view, _, window, cx| {
                                            view.cloud_return_local(window, cx)
                                        })),
                                ),
                        ),
                    )
                    .child(section_heading("Team"))
            })
            .child(if count == 0 {
                let (title, help) = if matches!(panel.job, Some(PanelJob::Refresh)) {
                    ("Loading workspaces", "")
                } else if !self.cloud.teams_loaded {
                    (
                        "Workspaces unavailable",
                        "Refresh to try loading your workspaces again.",
                    )
                } else {
                    (
                        "No workspaces yet",
                        "Create a workspace for your team, or join one with an invitation code.",
                    )
                };
                card()
                    .items_center()
                    .py_8()
                    .text_center()
                    .child(symbol(IconName::Layers))
                    .child(heading(title))
                    .when(!help.is_empty(), |card| card.child(muted(help)))
            } else {
                list_card().children(teams.into_iter().enumerate().map(|(index, team)| {
                    let team = team.clone();
                    let is_current = Some(team.id) == current;
                    let open_label = if publishing {
                        "Publish here"
                    } else if is_current {
                        "Manage"
                    } else {
                        "Open"
                    };
                    list_row(index)
                        .child(symbol(IconName::Users))
                        .child(workspace_identity(
                            team.name.clone(),
                            if team.role.can_edit() {
                                "Shared collections and requests"
                            } else {
                                "Browse and run shared requests"
                            },
                        ))
                        .child(role_badge(
                            self.cloud
                                .mounted
                                .as_ref()
                                .filter(|mounted| mounted.team.id == team.id)
                                .map_or(team.role, |mounted| mounted.team.role),
                        ))
                        .children(
                            self.cloud
                                .mounted
                                .as_ref()
                                .filter(|mounted| mounted.team.id == team.id && !publishing)
                                .map(directory_status),
                        )
                        .child(
                            Button::new(SharedString::from(format!("cloud-team-{}", team.id)))
                                .outline()
                                .compact()
                                .label(open_label)
                                .accessibility_label(format!("{open_label} {}", team.name))
                                .disabled(
                                    busy || (publishing && !team.role.can_edit())
                                        || (!publishing && !is_current && self.storage.is_err()),
                                )
                                .on_click(cx.listener(move |view, _, window, cx| {
                                    if publishing {
                                        view.cloud_publish(team.clone(), window, cx);
                                    } else if is_current {
                                        view.cloud_browse(false, window, cx);
                                    } else {
                                        view.cloud_open_team(team.clone(), window, cx);
                                    }
                                })),
                        )
                }))
            })
    }

    fn cloud_workspace_form_view(
        &self,
        panel: &Panel,
        form: WorkspaceForm,
        cx: &Context<Self>,
    ) -> Div {
        let (title, label, input, submit) = match form {
            WorkspaceForm::Create => (
                "New workspace",
                "Workspace name",
                &panel.name,
                "Create workspace",
            ),
            WorkspaceForm::Join => (
                "Join a workspace",
                "Invitation code",
                &panel.code,
                "Join workspace",
            ),
        };
        let on_submit =
            move |view: &mut Workspace, window: &mut Window, cx: &mut Context<Self>| match form {
                WorkspaceForm::Create => view.cloud_create(window, cx),
                WorkspaceForm::Join => view.cloud_accept(window, cx),
            };
        let busy = panel.is_busy();
        card()
            .child(heading(title))
            .child(
                div()
                    .capture_action(cx.listener(move |view, _: &Enter, window, cx| {
                        cx.stop_propagation();
                        on_submit(view, window, cx);
                    }))
                    .child(field(label, input, busy)),
            )
            .child(
                row()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("cloud-cancel-workspace-form")
                            .ghost()
                            .label("Cancel")
                            .disabled(busy)
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.cloud_workspace_form(None, window, cx)
                            })),
                    )
                    .child(
                        Button::new("cloud-submit-workspace-form")
                            .primary()
                            .label(submit)
                            .disabled(busy)
                            .on_click(
                                cx.listener(move |view, _, window, cx| on_submit(view, window, cx)),
                            ),
                    ),
            )
    }

    fn cloud_publication(&self, publication: &Publication, busy: bool, cx: &Context<Self>) -> Div {
        card()
            .child(
                row().gap_3()
                    .child(symbol(IconName::CloudUpload))
                    .child(div().flex_1().min_w_0().truncate().child(heading(format!("Publish {}", publication.name))))
                    .child(Button::new("cloud-preview-publication").outline().compact().disabled(busy)
                        .label(if publication.preview.is_some() { "Hide preview" } else { "Preview shared data" })
                        .on_click(cx.listener(|view, _, _, cx| {
                            if let Some(publication) = view.cloud.panel.as_mut().and_then(|panel| panel.publication.as_mut()) {
                                if publication.preview.is_some() {
                                    publication.preview = None;
                                } else {
                                    let contents: BTreeMap<_, _> = publication.changes.iter()
                                        .map(|(id, change)| (id, &change.content)).collect();
                                    publication.preview = serde_json::to_string_pretty(&contents).ok();
                                }
                            }
                            cx.notify();
                        }))),
            )
            .child(muted(
                "The team can see URLs, bodies, scripts and headers. Collection variable values are cleared, and supported credential fields use local variable references. Review the preview for sensitive values before publishing.",
            ))
            .children(publication.preview.as_ref().map(|preview| {
                div().id("cloud-publication-preview").max_h(px(300.)).overflow_y_scroll()
                    .rounded_md().p_3().bg(rgb(EDITOR)).font_family("monospace").text_xs().child(preview.clone())
            }))
    }

    fn cloud_team_page(&self, panel: &Panel, mounted: &Mounted, cx: &Context<Self>) -> Div {
        let busy = panel.is_busy();
        let owner = mounted.team.role == Role::Owner;
        let open_team = mounted.team.clone();
        let mut content = column().gap_4().child(
            row()
                .flex_wrap()
                .gap_3()
                .child(symbol(IconName::Users))
                .child(
                    column()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(
                            div()
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(26.))
                                .child(mounted.team.name.clone()),
                        )
                        .child(muted(match mounted.team.role {
                            Role::Owner => "Manage who can access this workspace.",
                            Role::Editor => "Edit and run shared requests in this workspace.",
                            Role::Viewer => {
                                "Run shared requests and save your changes as local drafts."
                            }
                        })),
                )
                .child(role_badge(mounted.team.role))
                .child(
                    Button::new("cloud-resume-workspace")
                        .primary()
                        .label("Open workspace")
                        .disabled(busy)
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.cloud_open_team(open_team.clone(), window, cx)
                        })),
                ),
        );
        if owner {
            let members =
                column()
                    .flex_1()
                    .min_w(px(448.))
                    .gap_3()
                    .child(
                        row()
                            .gap_2()
                            .justify_between()
                            .child(heading("Members"))
                            .child(
                                Button::new("cloud-refresh-members")
                                    .ghost()
                                    .compact()
                                    .icon(IconName::RefreshCw)
                                    .tooltip("Refresh members")
                                    .accessibility_label("Refresh members")
                                    .disabled(busy)
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        view.cloud_refresh(window, cx)
                                    })),
                            ),
                    )
                    .child(if panel.members.is_empty() {
                        card().child(muted(if busy {
                            "Loading members…"
                        } else {
                            "No members to display."
                        }))
                    } else {
                        list_card().children(
                            panel.members.iter().enumerate().map(|(index, member)| {
                                self.cloud_member_row(panel, member, index, cx)
                            }),
                        )
                    });
            content = content.child(
                row()
                    .gap_4()
                    .flex_wrap()
                    .items_start()
                    .child(members)
                    .child(self.cloud_invite_card(panel, cx).flex_1().min_w(px(300.))),
            );
        }
        content
    }

    fn cloud_member_row(
        &self,
        panel: &Panel,
        member: &Member,
        index: usize,
        cx: &Context<Self>,
    ) -> Div {
        let id = member.id;
        let busy = panel.is_busy();
        let role = member.role;
        let removing = panel.removing_member == Some(id);
        let change_to = if role == Role::Editor {
            Role::Viewer
        } else {
            Role::Editor
        };
        list_row(index)
            .flex_wrap()
            .child(symbol(IconName::User))
            .child(
                div()
                    .flex_1()
                    .min_w(px(125.))
                    .truncate()
                    .child(member.email.clone()),
            )
            .child(role_badge(role))
            .when(role != Role::Owner, |member_row| {
                member_row.child(row().gap_2().children(if removing {
                    [
                        Button::new(SharedString::from(format!("cloud-member-cancel-{id}")))
                            .ghost()
                            .label("Cancel")
                            .compact()
                            .disabled(busy)
                            .on_click(cx.listener(|view, _, _, cx| {
                                if let Some(panel) = &mut view.cloud.panel {
                                    panel.removing_member = None;
                                }
                                cx.notify();
                            })),
                        Button::new(SharedString::from(format!("cloud-member-confirm-{id}")))
                            .danger()
                            .label("Remove member")
                            .compact()
                            .disabled(busy)
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.cloud_member_role(id, None, window, cx)
                            })),
                    ]
                } else {
                    [
                        Button::new(SharedString::from(format!("cloud-member-role-{id}")))
                            .ghost()
                            .label(format!("Make {}", change_to.label()))
                            .compact()
                            .disabled(busy)
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.cloud_member_role(id, Some(change_to), window, cx)
                            })),
                        Button::new(SharedString::from(format!("cloud-member-remove-{id}")))
                            .ghost()
                            .label("Remove")
                            .compact()
                            .disabled(busy)
                            .on_click(cx.listener(move |view, _, _, cx| {
                                if let Some(panel) = &mut view.cloud.panel {
                                    panel.removing_member = Some(id);
                                }
                                cx.notify();
                            })),
                    ]
                }))
            })
    }

    fn cloud_invite_card(&self, panel: &Panel, cx: &Context<Self>) -> Div {
        let busy = panel.is_busy();
        let role = panel.invite_role;
        let role_option = |id, option: Role| {
            let button = Button::new(id)
                .outline()
                .compact()
                .selected(option == role)
                .label(option.label())
                .disabled(busy);
            button.on_click(cx.listener(move |view, _, _, cx| {
                if let Some(panel) = &mut view.cloud.panel {
                    panel.invite_role = option;
                }
                cx.notify();
            }))
        };
        card()
            .child(
                row()
                    .gap_3()
                    .child(symbol(IconName::UserPlus))
                    .child(heading("Invite a teammate")),
            )
            .child(muted("Your teammate must join with this email address."))
            .child(
                div()
                    .capture_action(cx.listener(move |view, _: &Enter, window, cx| {
                        cx.stop_propagation();
                        view.cloud_invite(role, window, cx);
                    }))
                    .child(field("Teammate’s email", &panel.invite_email, busy)),
            )
            .child(
                row()
                    .gap_3()
                    .flex_wrap()
                    .justify_between()
                    .child(
                        column()
                            .gap_2()
                            .child(
                                row()
                                    .gap_2()
                                    .child(role_option("cloud-role-editor", Role::Editor))
                                    .child(role_option("cloud-role-viewer", Role::Viewer)),
                            )
                            .child(muted(if role == Role::Editor {
                                "Can edit shared requests."
                            } else {
                                "Can view and run shared requests."
                            })),
                    )
                    .child(
                        Button::new("cloud-create-invite")
                            .primary()
                            .label("Create invitation")
                            .disabled(busy)
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.cloud_invite(role, window, cx)
                            })),
                    ),
            )
            .children(panel.invite_code.map(|code| {
                column()
                    .gap_2()
                    .rounded_md()
                    .bg(rgb(EDITOR))
                    .p_3()
                    .child(heading("Invitation code ready"))
                    .child(muted(
                        "Share this code with your teammate. It is valid for 7 days and one use.",
                    ))
                    .child(
                        row()
                            .gap_2()
                            .flex_wrap()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .font_family("monospace")
                                    .child(code.to_string()),
                            )
                            .child(
                                Button::new("cloud-copy-invite")
                                    .outline()
                                    .compact()
                                    .icon(if panel.invite_copied {
                                        IconName::Check
                                    } else {
                                        IconName::Copy
                                    })
                                    .label(if panel.invite_copied {
                                        "Copied"
                                    } else {
                                        "Copy code"
                                    })
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            code.to_string(),
                                        ));
                                        if let Some(panel) = &mut view.cloud.panel {
                                            panel.invite_copied = true;
                                        }
                                        cx.notify();
                                    })),
                            ),
                    )
            }))
    }
}

fn workspace_menu_item(
    label: impl Into<SharedString>,
    icon: IconName,
    workspace: &WeakEntity<Workspace>,
    action: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + Clone + 'static,
) -> PopupMenuItem {
    let workspace = workspace.clone();
    PopupMenuItem::new(label)
        .icon(icon)
        .on_click(move |_, window, cx| {
            let workspace = workspace.clone();
            let action = action.clone();
            window.defer(cx, move |window, cx| {
                _ = workspace.update(cx, |view, cx| action(view, window, cx));
            });
        })
}

fn directory_status(mounted: &Mounted) -> Div {
    if mounted.unreachable {
        status_badge("Offline", AMBER, AMBER_BG)
    } else if !mounted.cache.conflicts().is_empty() {
        status_badge("Conflict", RED, ERROR_BG)
    } else if mounted.sync_error.is_some() {
        status_badge("Sync unavailable", RED, ERROR_BG)
    } else if mounted.team.role.can_edit() && !mounted.cache.pending.is_empty() {
        status_badge("Syncing", AMBER, AMBER_BG)
    } else {
        status_badge("Current", GREEN, SELECTED)
    }
}

fn section_heading(label: &'static str) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(MUTED))
        .child(label)
}

fn workspace_identity(name: impl Into<SharedString>, detail: &'static str) -> Div {
    column()
        .flex_1()
        .min_w_0()
        .gap_1()
        .child(
            div()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .child(name.into()),
        )
        .child(
            div()
                .truncate()
                .text_xs()
                .text_color(rgb(MUTED))
                .child(detail),
        )
}

fn card() -> Div {
    column()
        .gap_4()
        .p_4()
        .rounded_lg()
        .bg(rgb(SURFACE))
        .border_1()
        .border_color(rgb(BORDER))
}

fn list_card() -> Div {
    column()
        .rounded_lg()
        .bg(rgb(SURFACE))
        .border_1()
        .border_color(rgb(BORDER))
}

fn list_row(index: usize) -> Div {
    row()
        .gap_3()
        .px_4()
        .py_3()
        .min_w_0()
        .when(index > 0, |row| row.border_t_1().border_color(rgb(BORDER)))
}

fn symbol(name: IconName) -> Div {
    row()
        .size(px(34.))
        .flex_shrink_0()
        .justify_center()
        .rounded_md()
        .bg(rgb(SELECTED))
        .child(icon(name, 17., ACCENT))
}

fn heading(label: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(15.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label.into())
}

fn muted(label: impl Into<SharedString>) -> Div {
    div().text_sm().text_color(rgb(MUTED)).child(label.into())
}

fn page_intro(title: &'static str, description: &'static str) -> Div {
    column()
        .gap_1()
        .child(
            div()
                .text_size(px(25.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(muted(description))
}

fn field(label: &'static str, input: &Entity<InputState>, busy: bool) -> Div {
    column()
        .gap_1()
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label))
        .child(Input::new(input).small().aria_label(label).disabled(busy))
}

fn role_badge(role: Role) -> Div {
    let (background, color) = match role {
        Role::Owner => (VIOLET_BG, VIOLET),
        Role::Editor => (SELECTED, GREEN),
        Role::Viewer => (EDITOR, MUTED),
    };
    status_badge(role.label(), color, background)
}

fn notice(symbol: IconName, color: u32, background: u32, message: String) -> Div {
    div()
        .flex()
        .items_start()
        .gap_2()
        .rounded_md()
        .px_3()
        .py_2()
        .bg(rgb(background))
        .text_sm()
        .text_color(rgb(color))
        .child(icon(symbol, 16., color))
        .child(div().min_w_0().child(message))
}
