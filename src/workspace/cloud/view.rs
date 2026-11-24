use gpui_kit::component::{
    Disableable,
    button::{Button, ButtonVariants},
    input::Input,
};
use gpui_kit::prelude::FluentBuilder as _;

use super::*;
use crate::workspace::view::{BORDER, EDITOR, GREEN, MUTED, RED, column, row};

impl Workspace {
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
            format!("{} · {}", mounted.team.name, mounted.team.role.label())
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
        use gpui_kit::component::input::Escape;

        let panel = self.cloud.panel.as_ref()?;
        let active = self.cloud.mounted.as_ref();
        let header = row()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(active.map_or("Cloud workspaces".into(), |mounted| {
                        mounted.team.name.clone()
                    })),
            )
            .child(
                Button::new("close-cloud")
                    .ghost()
                    .label("Close")
                    .disabled(!panel.can_close())
                    .on_click(cx.listener(|view, _, window, cx| view.close_cloud(window, cx))),
            );
        let content = column()
            .id("cloud-content")
            .w_full()
            .max_w(px(720.))
            .gap_3()
            .child(header);
        let content = if let Some(editor) = &panel.server_editor {
            content
                .child(div().text_color(rgb(MUTED)).child(
                    "Enter the server URL and public API key provided by your administrator.",
                ))
                .child(
                    column()
                        .gap_1()
                        .child("Server URL")
                        .child(Input::new(&editor.url).aria_label("Server URL")),
                )
                .child(
                    column()
                        .gap_1()
                        .child("Public API key")
                        .child(Input::new(&editor.key).aria_label("Public API key")),
                )
                .child(
                    row()
                        .gap_2()
                        .child(
                            Button::new("cloud-save-server")
                                .primary()
                                .label("Save server")
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.cloud_save_server(window, cx)
                                })),
                        )
                        .when(panel.server.is_some(), |row| {
                            row.child(
                                Button::new("cloud-cancel-server-edit")
                                    .ghost()
                                    .label("Cancel")
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        view.cloud_cancel_server_edit(window, cx)
                                    })),
                            )
                        }),
                )
        } else if let Some(server) = panel.server.as_ref().filter(|_| self.cloud.api.is_none()) {
            content
                .child(
                    row()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .text_color(rgb(MUTED))
                                .child(server.url.clone()),
                        )
                        .child(
                            Button::new("cloud-change-server")
                                .ghost()
                                .label("Change server")
                                .disabled(panel.is_busy())
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.cloud_edit_server(window, cx)
                                })),
                        ),
                )
                .child(
                    Input::new(&panel.email)
                        .aria_label("Email")
                        .disabled(panel.is_busy()),
                )
                .child(
                    Input::new(&panel.password)
                        .aria_label("Password")
                        .disabled(panel.is_busy()),
                )
                .child(
                    row()
                        .gap_2()
                        .child(
                            Button::new("cloud-sign-in")
                                .primary()
                                .label("Sign in")
                                .disabled(panel.is_busy())
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.cloud_sign_in(false, window, cx)
                                })),
                        )
                        .child(
                            Button::new("cloud-sign-up")
                                .outline()
                                .label("Create account")
                                .disabled(panel.is_busy())
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.cloud_sign_in(true, window, cx)
                                })),
                        ),
                )
        } else if let Some(mounted) = active {
            let content = content.child(
                row()
                    .gap_2()
                    .child(
                        Button::new("cloud-local")
                            .outline()
                            .label("Back to local")
                            .disabled(panel.is_busy())
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.cloud_return_local(window, cx)
                            })),
                    )
                    .child(
                        Button::new("cloud-sign-out-active")
                            .ghost()
                            .label("Sign out")
                            .disabled(panel.is_busy())
                            .on_click(
                                cx.listener(|view, _, window, cx| view.cloud_sign_out(window, cx)),
                            ),
                    ),
            );
            if mounted.team.role == Role::Owner {
                content
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Members"))
                    .children(panel.members.iter().map(|member| {
                        let id = member.id;
                        let role = member.role;
                        row()
                            .gap_2()
                            .child(div().flex_1().truncate().child(member.email.clone()))
                            .child(div().text_xs().text_color(rgb(MUTED)).child(role.label()))
                            .when(role != Role::Owner, |row| {
                                let change_to = if role == Role::Editor {
                                    Role::Viewer
                                } else {
                                    Role::Editor
                                };
                                row.child(
                                    Button::new(SharedString::from(format!(
                                        "cloud-member-role-{id}"
                                    )))
                                    .ghost()
                                    .compact()
                                    .label(format!("Make {}", change_to.label()))
                                    .disabled(panel.is_busy())
                                    .on_click(cx.listener(move |view, _, window, cx| {
                                        view.cloud_member_role(id, Some(change_to), window, cx)
                                    })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "cloud-member-remove-{id}"
                                    )))
                                    .ghost()
                                    .compact()
                                    .label("Remove")
                                    .disabled(panel.is_busy())
                                    .on_click(cx.listener(move |view, _, window, cx| {
                                        view.cloud_member_role(id, None, window, cx)
                                    })),
                                )
                            })
                    }))
                    .child(
                        row()
                            .gap_2()
                            .child(
                                div().flex_1().child(
                                    Input::new(&panel.invite_email)
                                        .aria_label("Teammate's email")
                                        .disabled(panel.is_busy()),
                                ),
                            )
                            .child(
                                Button::new("cloud-invite-editor")
                                    .primary()
                                    .label("Invite editor")
                                    .disabled(panel.is_busy())
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        view.cloud_invite(Role::Editor, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("cloud-invite-viewer")
                                    .outline()
                                    .label("Invite viewer")
                                    .disabled(panel.is_busy())
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        view.cloud_invite(Role::Viewer, window, cx)
                                    })),
                            ),
                    )
                    .children(panel.invite_code.map(|code| {
                        row().gap_2().child(code.to_string()).child(
                            Button::new("cloud-copy-invite")
                                .ghost()
                                .label("Copy code")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        code.to_string(),
                                    ))
                                })),
                        )
                    }))
            } else {
                content
            }
        } else {
            let publishing = panel.publication.is_some();
            let content = if let Some(publication) = &panel.publication {
                content
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("Publish {}", publication.name)))
                    .child(div().text_color(rgb(MUTED)).child(
                        "The team can see URLs, bodies, scripts and headers in published requests. Collection variable values are cleared. Supported credential fields use local variable references. Review any sensitive values in free text before publishing."
                    ))
                    .child(Button::new("cloud-preview-publication").ghost()
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
                        })))
                    .children(publication.preview.as_ref().map(|preview| {
                        div().id("cloud-publication-preview").max_h(px(350.)).overflow_y_scroll()
                            .p_3().bg(rgb(EDITOR)).font_family("monospace").text_xs().child(preview.clone())
                    }))
            } else {
                content
            };
            content
                .children(self.cloud.teams.iter().cloned().map(|team| {
                    let label = team.name.clone();
                    let role = team.role;
                    let key = SharedString::from(format!("cloud-team-{}", team.id));
                    row()
                        .gap_2()
                        .py_1()
                        .child(div().flex_1().truncate().child(label))
                        .child(div().text_xs().text_color(rgb(MUTED)).child(role.label()))
                        .child(
                            Button::new(key)
                                .outline()
                                .label(if publishing { "Publish here" } else { "Open" })
                                .disabled(
                                    panel.is_busy()
                                        || (publishing && !role.can_edit())
                                        || self.storage.is_err(),
                                )
                                .on_click(cx.listener(move |view, _, window, cx| {
                                    if publishing {
                                        view.cloud_publish(team.clone(), window, cx);
                                    } else {
                                        view.cloud_open_team(team.clone(), window, cx);
                                    }
                                })),
                        )
                }))
                .child(
                    row()
                        .gap_2()
                        .child(
                            div().flex_1().child(
                                Input::new(&panel.name)
                                    .aria_label("Workspace name")
                                    .disabled(panel.is_busy()),
                            ),
                        )
                        .child(
                            Button::new("cloud-create")
                                .primary()
                                .label("Create workspace")
                                .disabled(panel.is_busy())
                                .on_click(
                                    cx.listener(|view, _, window, cx| {
                                        view.cloud_create(window, cx)
                                    }),
                                ),
                        ),
                )
                .child(
                    row()
                        .gap_2()
                        .child(
                            div().flex_1().child(
                                Input::new(&panel.code)
                                    .aria_label("Invitation code")
                                    .disabled(panel.is_busy()),
                            ),
                        )
                        .child(
                            Button::new("cloud-accept")
                                .outline()
                                .label("Accept invitation")
                                .disabled(panel.is_busy())
                                .on_click(
                                    cx.listener(|view, _, window, cx| {
                                        view.cloud_accept(window, cx)
                                    }),
                                ),
                        ),
                )
                .child(
                    Button::new("cloud-sign-out")
                        .ghost()
                        .label("Sign out")
                        .disabled(panel.is_busy())
                        .on_click(
                            cx.listener(|view, _, window, cx| view.cloud_sign_out(window, cx)),
                        ),
                )
        };
        Some(
            column().flex_1().min_w_0().min_h_0().child(
                column()
                    .id("cloud-panel")
                    .size_full()
                    .overflow_y_scroll()
                    .p_4()
                    .child(
                        content
                            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                                cx.stop_propagation();
                                this.close_cloud(window, cx);
                            }))
                            .when(panel.is_busy(), |content| {
                                content.child(div().text_xs().child("Working…"))
                            })
                            .children(
                                panel.message.as_ref().map(|message| {
                                    div().text_color(rgb(GREEN)).child(message.clone())
                                }),
                            )
                            .children(
                                panel
                                    .error
                                    .as_ref()
                                    .map(|error| div().text_color(rgb(RED)).child(error.clone())),
                            ),
                    ),
            ),
        )
    }
}
