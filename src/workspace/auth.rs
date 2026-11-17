use super::*;
use crate::request::Authorization;

const NONE: &str = "No Auth";
const BEARER: &str = "Bearer Token";
const BASIC: &str = "Basic Auth";

pub(super) struct AuthFields {
    pub(super) kind: Entity<SelectState<Vec<&'static str>>>,
    pub(super) token: Entity<InputState>,
    pub(super) username: Entity<InputState>,
    pub(super) password: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl AuthFields {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let kind = cx.new(|cx| {
            SelectState::new(
                vec![NONE, BEARER, BASIC],
                Some(IndexPath::default()),
                window,
                cx,
            )
        });
        let token = cx.new(|cx| InputState::new(window, cx).masked(true));
        let username = cx.new(|cx| InputState::new(window, cx));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let mut subscriptions = vec![cx.subscribe(&kind, |view, _, event, cx| {
            if matches!(event, SelectEvent::Confirm(_)) {
                view.update_authorization(cx);
            }
        })];
        subscriptions.extend([&token, &username, &password].map(|input| {
            cx.subscribe(input, |view, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    view.update_authorization(cx);
                }
            })
        }));
        Self {
            kind,
            token,
            username,
            password,
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn load(&self, auth: &Authorization, window: &mut Window, cx: &mut App) {
        let (row, token, username, password) = match auth {
            Authorization::None => (0, "", "", ""),
            Authorization::Bearer { token } => (1, token.as_str(), "", ""),
            Authorization::Basic { username, password } => {
                (2, "", username.as_str(), password.as_str())
            }
        };
        self.kind.update(cx, |kind, cx| {
            kind.set_selected_index(Some(IndexPath::default().row(row)), window, cx)
        });
        for (field, value, masked) in [
            (&self.token, token, true),
            (&self.username, username, false),
            (&self.password, password, true),
        ] {
            field.update(cx, |state, cx| {
                state.set_masked(masked, window, cx);
                state.set_value(value.to_owned(), window, cx);
            });
        }
    }

    fn value(&self, cx: &App) -> Authorization {
        match self.kind.read(cx).selected_value().copied() {
            Some(BEARER) => Authorization::Bearer {
                token: self.token.read(cx).value().to_string(),
            },
            Some(BASIC) => Authorization::Basic {
                username: self.username.read(cx).value().to_string(),
                password: self.password.read(cx).value().to_string(),
            },
            _ => Authorization::None,
        }
    }
}

impl Workspace {
    fn update_authorization(&mut self, cx: &mut Context<Self>) {
        self.requests[self.selected].draft.authorization = self.auth.value(cx);
        cx.notify();
    }
}
