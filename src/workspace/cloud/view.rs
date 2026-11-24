use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Disableable, Selectable,
    button::{Button, ButtonVariants},
    input::{Enter, Escape, Input},
    spinner::Spinner,
};
use gpui_kit::prelude::FluentBuilder as _;

use super::*;
use crate::workspace::view::{
    ACCENT, BACKGROUND, BORDER, EDITOR, GREEN, MUTED, RED, SELECTED, SURFACE, column, icon, row,
};

impl Workspace {
    pub(in crate::workspace) fn team_sidebar_controls(&self, cx: &Context<Self>) -> Option<Div> {
        let mounted = self.cloud.mounted.as_ref()?;
        Some(
            row()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(mounted.team.name.clone()),
                )
                .child(
                    Button::new("cloud-local")
                        .ghost()
                        .compact()
                        .flex_shrink_0()
                        .text_xs()
                        .label("Back to local")
                        .disabled(self.cloud.panel.as_ref().is_some_and(Panel::is_busy))
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.cloud_return_local(window, cx);
                        })),
                ),
        )
    }

    pub(in crate::workspace) fn team_status(&self, cx: &Context<Self>) -> Option<Div> {
        let mounted = self.cloud.mounted.as_ref()?;
        let conflicts = mounted.cache.conflicts();
        let status = if let Some(error) = &mounted.sync_error {
            error.clone()
        } else if !conflicts.is_empty() {
            "Resolve conflicts to sync changes".into()
        } else if !mounted.cache.pending.is_empty() && !mounted.team.role.can_edit() {
            "Workspace is read-only · unpublished changes saved on this device".into()
        } else if !mounted.cache.pending.is_empty() {
            "Changes saved on this device · syncing".into()
        } else {
            return None;
        };
        Some(
            column()
                .px_3()
                .py_1()
                .border_b_1()
                .border_color(rgb(BORDER))
                .text_xs()
                .child(
                    div()
                        .text_color(rgb(if mounted.sync_error.is_some() {
                            RED
                        } else {
                            MUTED
                        }))
                        .child(status),
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
                                .label("Compare")
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
                        .px_3()
                        .py_2()
                        .child(div().flex_1().child("Compare conflicting changes"))
                        .child(
                            Button::new("close-cloud-comparison")
                                .ghost()
                                .label("Close")
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.set_cloud_comparison(None, window, cx)
                                })),
                        ),
                )
                .child(
                    row()
                        .id("cloud-compare-scroll")
                        .flex_1()
                        .min_h_0()
                        .items_start()
                        .overflow_y_scroll()
                        .gap_3()
                        .p_3()
                        .child(card("Your unpublished version", local))
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
            .max_w(px(880.))
            .min_h(px(58.))
            .gap_3()
            .child(symbol(IconName::Cloud))
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Cloud"))
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
                    .tooltip("Close Cloud (Esc)")
                    .accessibility_label("Close Cloud")
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
                        .bg(rgb(EDITOR))
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
                        .px_5()
                        .py_6()
                        .child(
                            column()
                                .w_full()
                                .max_w(px(if auth_page { 440. } else { 880. }))
                                .flex_shrink_0()
                                .gap_4()
                                .children(panel.error.as_ref().map(|message| {
                                    notice(IconName::CircleAlert, RED, 0x341f2c, message.clone())
                                }))
                                .children(panel.message.as_ref().map(|message| {
                                    notice(IconName::CircleCheck, GREEN, 0x182b28, message.clone())
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
                "Use the server URL and public API key from your administrator.",
            ))
            .child(
                column()
                    .gap_3()
                    .capture_action(cx.listener(|view, _: &Enter, window, cx| {
                        cx.stop_propagation();
                        view.cloud_save_server(window, cx);
                    }))
                    .child(field("Server URL", &editor.url, false))
                    .child(field("Public API key", &editor.key, false)),
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
                "Access your shared workspaces in Prestman Cloud.",
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
                            "Collections and requests shared with your teams."
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
                        .child(symbol(IconName::Folder))
                        .child(
                            column()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child(
                                    div()
                                        .truncate()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(team.name.clone()),
                                )
                                .when(is_current, |name| name.child(muted("Current workspace"))),
                        )
                        .child(role_badge(team.role))
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
            card().child(
                row()
                    .flex_wrap()
                    .gap_3()
                    .child(symbol(IconName::FolderOpen))
                    .child(
                        column()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(px(23.))
                                    .child(mounted.team.name.clone()),
                            )
                            .child(muted(if mounted.team.role.can_edit() {
                                "Team workspace"
                            } else {
                                "Read-only access"
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
            ),
        );
        if owner {
            content =
                content
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
                    })
                    .child(self.cloud_invite_card(panel, cx));
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
            .child(row().gap_3().child(symbol(IconName::UserPlus)).child(heading("Invite a teammate")))
            .child(
                div().capture_action(cx.listener(move |view, _: &Enter, window, cx| {
                    cx.stop_propagation();
                    view.cloud_invite(role, window, cx);
                })).child(field("Teammate’s email", &panel.invite_email, busy)),
            )
            .child(
                row().gap_3().flex_wrap().justify_between()
                    .child(column().gap_2()
                        .child(row().gap_2().child(role_option("cloud-role-editor", Role::Editor)).child(role_option("cloud-role-viewer", Role::Viewer)))
                        .child(muted(if role == Role::Editor { "Can edit shared requests." } else { "Can view and run shared requests." })))
                    .child(Button::new("cloud-create-invite").primary().label("Create invitation").disabled(busy)
                        .on_click(cx.listener(move |view, _, window, cx| view.cloud_invite(role, window, cx)))),
            )
            .children(panel.invite_code.map(|code| {
                column().gap_2().rounded_md().bg(rgb(EDITOR)).p_3()
                    .child(heading("Invitation code ready"))
                    .child(muted("Share this code with your teammate. They can enter it under Join with code."))
                    .child(row().gap_2().flex_wrap().justify_between()
                        .child(div().text_xs().font_family("monospace").child(code.to_string()))
                        .child(Button::new("cloud-copy-invite").outline().compact()
                            .icon(if panel.invite_copied { IconName::Check } else { IconName::Copy })
                            .label(if panel.invite_copied { "Copied" } else { "Copy code" })
                            .on_click(cx.listener(move |view, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(code.to_string()));
                                if let Some(panel) = &mut view.cloud.panel { panel.invite_copied = true; }
                                cx.notify();
                            }))))
            }))
    }
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
        .child(Input::new(input).aria_label(label).disabled(busy))
}

fn role_badge(role: Role) -> Div {
    let (background, color) = match role {
        Role::Owner => (SELECTED, ACCENT),
        Role::Editor => (0x182b28, GREEN),
        Role::Viewer => (EDITOR, MUTED),
    };
    div()
        .flex_shrink_0()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(background))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(color))
        .child(role.label())
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
