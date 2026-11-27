#[cfg(test)]
mod tests;

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{StatusCode, Url, blocking::Client, redirect::Policy};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use uuid::Uuid;

use super::tree::{Changes, SyncResult};
use crate::storage::CloudSessionStore;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Editor,
    Viewer,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Self::Owner => "Owner",
            Self::Editor => "Editor",
            Self::Viewer => "Viewer",
        }
    }

    pub fn can_edit(self) -> bool {
        self != Self::Viewer
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Team {
    pub id: Uuid,
    pub name: String,
    pub role: Role,
}

#[derive(Clone, Deserialize)]
pub struct Member {
    pub id: Uuid,
    pub email: String,
    pub role: Role,
}

#[derive(Clone)]
pub struct CloudConfig {
    server: CloudServer,
    http: Client,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct CloudServer {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl CloudServer {
    pub fn from_environment() -> Result<Option<Self>> {
        let runtime_url = std::env::var("PRESTMAN_CLOUD_URL").ok();
        let runtime_key = std::env::var("PRESTMAN_CLOUD_KEY").ok();
        let compiled_url = option_env!("PRESTMAN_CLOUD_URL");
        let url = runtime_url.as_deref().or(compiled_url);
        // A runtime URL override can point to another server. Keep its key separate.
        let key = runtime_key
            .as_deref()
            .or(option_env!("PRESTMAN_CLOUD_KEY").filter(|_| url == compiled_url))
            .unwrap_or_default();
        match url {
            Some(url) => Self::parse(url, key).map(Some),
            None if key.trim().is_empty() => Ok(None),
            None => bail!("Set PRESTMAN_CLOUD_URL, or enter a server below."),
        }
    }

    pub fn parse(address: &str, key: &str) -> Result<Self> {
        let url = Url::parse(address.trim()).context("Enter a valid cloud server URL.")?;
        ensure!(
            url.host().is_some()
                && (url.scheme() == "https" || (url.scheme() == "http" && is_loopback(&url)))
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none(),
            "Enter the server origin without a path. HTTPS is required outside localhost."
        );
        let key = key.trim();
        ensure!(
            key.is_empty() || is_publishable_key(key),
            "Enter the server's public API key (publishable or anon)."
        );
        Ok(Self {
            url: url.origin().ascii_serialization(),
            key: (!key.is_empty()).then(|| key.to_owned()),
        })
    }
}

fn is_loopback(url: &Url) -> bool {
    matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
}

impl CloudConfig {
    pub fn new(server: CloudServer) -> Result<Self> {
        let loopback = is_loopback(&Url::parse(&server.url)?);
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(12))
            .redirect(Policy::none());
        let http = if loopback { http.no_proxy() } else { http };
        Ok(Self {
            server,
            http: http.build()?,
        })
    }

    fn post(
        &self,
        path: &str,
        token: Option<&str>,
        body: &Value,
    ) -> Result<reqwest::blocking::Response> {
        let mut request = self
            .http
            .post(format!("{}{path}", self.server.url))
            .header("content-type", "application/json")
            .body(serde_json::to_vec(body)?);
        if let Some(key) = &self.server.key {
            request = request.header("apikey", key);
        }
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        request.send().context("Could not reach Prestman Cloud.")
    }

    pub fn authenticate(
        &self,
        email: &str,
        password: &str,
        signup: bool,
        store: Option<CloudSessionStore>,
    ) -> Result<Option<CloudApi>> {
        let path = if signup {
            "/auth/v1/signup"
        } else {
            "/auth/v1/token?grant_type=password"
        };
        let body = read(self.post(path, None, &json!({"email": email, "password": password}))?)?;
        if signup && body.get("access_token").is_none_or(Value::is_null) {
            return Ok(None);
        }
        let response: AuthResponse =
            serde_json::from_value(body).context("Invalid sign-in response.")?;
        self.start_session(response, store).map(Some)
    }

    pub fn restore(&self, store: CloudSessionStore, refresh_token: &str) -> Result<CloudApi> {
        let response = match self.refresh(refresh_token) {
            Err(error) if error.is::<SessionExpired>() => {
                store.forget()?;
                return Err(error);
            }
            result => result?,
        };
        self.start_session(response, Some(store))
    }

    fn refresh(&self, refresh_token: &str) -> Result<AuthResponse> {
        deserialize(self.post(
            "/auth/v1/token?grant_type=refresh_token",
            None,
            &json!({ "refresh_token": refresh_token }),
        )?)
    }

    fn start_session(
        &self,
        response: AuthResponse,
        store: Option<CloudSessionStore>,
    ) -> Result<CloudApi> {
        let account = response.user.id;
        let mut session = Session::new(response, store);
        session.save(&self.server.url)?;
        Ok(CloudApi {
            account,
            config: self.clone(),
            session: Arc::new(Mutex::new(Some(session))),
        })
    }
}

fn is_publishable_key(key: &str) -> bool {
    if key.starts_with("sb_publishable_") {
        return true;
    }
    // Self-hosted Supabase can use a legacy JWT as its public API key.
    let claims = key
        .split('.')
        .nth(1)
        .and_then(|segment| URL_SAFE_NO_PAD.decode(segment).ok());
    claims
        .and_then(|claims| serde_json::from_slice::<Value>(&claims).ok())
        .is_some_and(|claims| claims.get("role").and_then(Value::as_str) == Some("anon"))
}

#[derive(Deserialize)]
struct AuthResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
    user: AuthUser,
}

#[derive(Deserialize)]
struct AuthUser {
    id: Uuid,
}

struct Session {
    access_token: String,
    refresh_token: String,
    expires_at: Instant,
    store: Option<CloudSessionStore>,
    needs_save: bool,
}

impl Session {
    fn new(value: AuthResponse, store: Option<CloudSessionStore>) -> Self {
        Self {
            access_token: value.access_token,
            refresh_token: value.refresh_token,
            expires_at: Instant::now()
                + Duration::from_secs(value.expires_in.saturating_sub(30).min(86400)),
            needs_save: store.is_some(),
            store,
        }
    }

    fn save(&mut self, origin: &str) -> Result<()> {
        if self.needs_save
            && let Some(store) = &self.store
        {
            store.save(origin, &self.refresh_token)?;
            self.needs_save = false;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct CloudApi {
    pub account: Uuid,
    config: CloudConfig,
    session: Arc<Mutex<Option<Session>>>,
}

impl CloudApi {
    pub fn sign_out(&self) -> Result<()> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| anyhow::anyhow!("Cloud session is unavailable."))?;
        if let Some(store) = session.as_ref().and_then(|session| session.store.as_ref()) {
            store.forget()?;
        }
        *session = None;
        Ok(())
    }

    fn rpc<T: DeserializeOwned>(&self, name: &str, body: Value) -> Result<T> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| anyhow::anyhow!("Cloud session is unavailable."))?;
        let session = session.as_mut().context("You are signed out of Cloud.")?;
        session.save(&self.config.server.url)?;
        if Instant::now() >= session.expires_at {
            self.refresh(session)?;
        }
        let path = format!("/rest/v1/rpc/prestman_{name}");
        let mut response = self
            .config
            .post(&path, Some(&session.access_token), &body)?;
        if response.status() == StatusCode::UNAUTHORIZED {
            self.refresh(session)?;
            response = self
                .config
                .post(&path, Some(&session.access_token), &body)?;
        }
        deserialize(response)
    }

    fn refresh(&self, session: &mut Session) -> Result<()> {
        let response = self.config.refresh(&session.refresh_token)?;
        self.apply_refresh(session, response)
    }

    fn apply_refresh(&self, session: &mut Session, response: AuthResponse) -> Result<()> {
        ensure!(
            response.user.id == self.account,
            "Cloud session account changed."
        );
        *session = Session::new(response, session.store.take());
        session.save(&self.config.server.url)
    }

    pub fn workspaces(&self) -> Result<Vec<Team>> {
        self.rpc("workspaces", json!({}))
    }

    pub fn create(&self, name: &str) -> Result<Team> {
        self.rpc("create_workspace", json!({"p_name": name}))
    }

    pub fn invite(&self, team: Uuid, email: &str, role: Role) -> Result<Uuid> {
        self.rpc(
            "invite",
            json!({"p_workspace": team, "p_email": email, "p_role": role}),
        )
    }

    pub fn accept(&self, code: Uuid) -> Result<Team> {
        self.rpc("accept_invite", json!({"p_code": code}))
    }

    pub fn members(&self, team: Uuid) -> Result<Vec<Member>> {
        self.rpc("members", json!({"p_workspace": team}))
    }

    pub fn set_role(&self, team: Uuid, member: Uuid, role: Option<Role>) -> Result<Vec<Member>> {
        self.rpc(
            "set_member_role",
            json!({"p_workspace": team, "p_member": member, "p_role": role}),
        )
    }

    pub fn sync(&self, team: Uuid, changes: &Changes, revision: Option<u64>) -> Result<SyncResult> {
        self.rpc(
            "sync",
            json!({"p_workspace": team, "p_changes": changes, "p_revision": revision}),
        )
    }

    pub fn cache_key(&self) -> String {
        let origin = Uuid::new_v5(&Uuid::NAMESPACE_URL, self.config.server.url.as_bytes());
        format!("{origin}-{}", self.account)
    }
}

#[derive(Debug)]
pub struct AccessDenied(String);

impl std::fmt::Display for AccessDenied {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for AccessDenied {}

#[derive(Debug)]
struct SessionExpired;

impl std::fmt::Display for SessionExpired {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Your Cloud session has expired. Sign in again.")
    }
}

impl std::error::Error for SessionExpired {}

fn deserialize<T: DeserializeOwned>(response: reqwest::blocking::Response) -> Result<T> {
    serde_json::from_value(read(response)?).context("Invalid response from Prestman Cloud.")
}

fn read(response: reqwest::blocking::Response) -> Result<Value> {
    use std::io::Read as _;
    let status = response.status();
    let mut bytes = Vec::new();
    response
        .take(10 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 10 * 1024 * 1024,
        "Prestman Cloud response is too large."
    );
    let data = serde_json::from_slice::<Value>(&bytes);
    if !status.is_success() {
        let error_code = data
            .as_ref()
            .ok()
            .and_then(|body| body.get("error_code")?.as_str());
        if status.is_client_error()
            && matches!(
                error_code,
                Some(
                    "refresh_token_not_found"
                        | "refresh_token_already_used"
                        | "session_not_found"
                        | "session_expired"
                )
            )
        {
            return Err(SessionExpired.into());
        }
        let message = data.as_ref().ok().and_then(|body| {
            ["message", "msg", "error_description"]
                .into_iter()
                .find_map(|key| body.get(key)?.as_str())
        });
        let message = message
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Prestman Cloud returned {status}."));
        if status == StatusCode::FORBIDDEN {
            return Err(AccessDenied(message).into());
        }
        bail!("{message}");
    }
    Ok(data?)
}
