use std::time::Duration;

use gpui_kit::component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
};
use gpui_kit::prelude::FluentBuilder as _;

use super::*;
use crate::updater::{self, AvailableUpdate};

#[derive(Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Checking,
    Installing,
    Ready,
}

#[derive(Default)]
pub(super) struct UpdateState {
    phase: Phase,
    available: Option<AvailableUpdate>,
    message: Option<String>,
    error: Option<String>,
    task: Option<Task<()>>,
    timer: Option<Task<()>>,
}

impl UpdateState {
    pub(super) fn installing(&self) -> bool {
        self.phase == Phase::Installing
    }

    pub(super) fn can_check(&self) -> bool {
        self.phase == Phase::Idle
    }
}

impl Workspace {
    pub(super) fn start_update_checks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.updates.timer = Some(cx.spawn_in(window, async move |view, cx| {
            // Let the first frame and saved workspace load before starting network work.
            cx.background_executor().timer(Duration::from_secs(5)).await;
            loop {
                if view
                    .update_in(cx, |view, window, cx| {
                        view.check_for_updates(false, window, cx);
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(6 * 60 * 60))
                    .await;
            }
        }));
    }

    pub(super) fn check_for_updates(
        &mut self,
        manual: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.updates.can_check() {
            return;
        }
        self.updates.phase = Phase::Checking;
        self.updates.error = None;
        self.updates.message = manual.then(|| "Checking for updates…".to_owned());
        self.updates.task = Some(cx.spawn_in(window, async move |view, cx| {
            let result = cx.background_spawn(async { updater::check() }).await;
            _ = view.update_in(cx, |view, _, cx| {
                view.updates.phase = Phase::Idle;
                view.updates.message = None;
                match result {
                    Ok(available) => {
                        if manual && available.is_none() {
                            view.updates.message =
                                Some(format!("Prestman {} is up to date", updater::VERSION));
                        }
                        view.updates.available = available;
                    }
                    Err(error) if manual => {
                        view.updates.error = Some(format!("Could not check for updates: {error:#}"))
                    }
                    Err(error) => eprintln!("Could not check for updates: {error:#}"),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn install_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.updates.phase != Phase::Idle {
            return;
        }
        let Some(update) = self.updates.available.clone() else {
            return;
        };
        self.updates.phase = Phase::Installing;
        self.updates.error = None;
        self.updates.message = Some(format!(
            "Downloading and verifying Prestman {}…",
            update.version
        ));
        self.updates.task = Some(cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_spawn(async move { updater::install(update) })
                .await;
            _ = view.update_in(cx, |view, _, cx| {
                view.updates.message = None;
                match result {
                    Ok(()) => view.updates.phase = Phase::Ready,
                    Err(error) => {
                        view.updates.phase = Phase::Idle;
                        view.updates.error =
                            Some(format!("Could not install the update: {error:#}"));
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn restart_after_update(&mut self, cx: &mut Context<Self>) {
        if self.updates.phase != Phase::Ready {
            return;
        }
        if !self.save_before_close(cx) {
            self.updates.error = self.save_error.take();
            cx.notify();
            return;
        }
        // Save first: restart replaces/exits the process without GPUI's quit hooks.
        let Err(error) = self_update::restart::restart();
        self.exit_saved = false;
        self.updates.error = Some(format!(
            "Could not restart Prestman: {error:#}. Close and reopen it to finish updating."
        ));
        cx.notify();
    }

    pub(super) fn update_button(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let update = self.updates.available.as_ref()?;
        let (label, tooltip) = match self.updates.phase {
            Phase::Ready => (
                "Restart",
                format!(
                    "Restart to use Prestman {}. Your drafts will be saved.",
                    update.version
                ),
            ),
            Phase::Installing => (
                "Updating…",
                format!("Installing Prestman {}", update.version),
            ),
            _ => (
                "Update",
                format!("Download and install Prestman {}", update.version),
            ),
        };
        Some(
            Button::new("app-update")
                .small()
                .primary()
                .label(label)
                .tooltip(tooltip)
                .disabled(matches!(
                    self.updates.phase,
                    Phase::Checking | Phase::Installing
                ))
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.updates.phase == Phase::Ready {
                        this.restart_after_update(cx);
                    } else {
                        this.install_update(window, cx);
                    }
                })),
        )
    }

    pub(super) fn update_status(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let error = self.updates.error.is_some();
        let message = self
            .updates
            .error
            .as_ref()
            .or(self.updates.message.as_ref())?;
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .bg(Theme::global(cx).secondary)
                .child(div().flex_1().min_w_0().child(message.clone()))
                .when(error, |row| {
                    row.child(
                        Button::new("update-releases")
                            .small()
                            .ghost()
                            .label("Releases")
                            .on_click(|_, _, cx| cx.open_url(updater::RELEASES_URL)),
                    )
                })
                .when(!self.updates.installing(), |row| {
                    row.child(
                        Button::new("dismiss-update-status")
                            .small()
                            .ghost()
                            .label("Dismiss")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.updates.error = None;
                                this.updates.message = None;
                                cx.notify();
                            })),
                    )
                }),
        )
    }
}
