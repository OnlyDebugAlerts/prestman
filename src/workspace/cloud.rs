mod view;

pub(super) const CLOUD_CONTEXT: &str = "PrestmanCloud";

use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context as _, Result, ensure};
use uuid::Uuid;

use super::*;
use crate::cloud::{
    AccessDenied, Cache, Changes, CloudApi, CloudConfig, CloudServer, Content, Draft, ItemRevision,
    Member, Role, SyncResult, Team, share_request,
};

#[derive(Default)]
pub(super) struct CloudState {
    api: Option<CloudApi>,
    panel: Option<Panel>,
    teams: Vec<Team>,
    teams_loaded: bool,
    mounted: Option<Mounted>,
    local: Option<LocalView>,
    pub(super) rename_version: Option<ItemRevision>,
}

struct Panel {
    server: Option<CloudServer>,
    server_editor: Option<ServerEditor>,
    email: Entity<InputState>,
    password: Entity<InputState>,
    name: Entity<InputState>,
    code: Entity<InputState>,
    invite_email: Entity<InputState>,
    focus: FocusHandle,
    signup: bool,
    browsing_workspaces: bool,
    workspace_form: Option<WorkspaceForm>,
    invite_role: Role,
    invite_copied: bool,
    removing_member: Option<Uuid>,
    publication: Option<Publication>,
    members: Vec<Member>,
    invite_code: Option<Uuid>,
    job: Option<PanelJob>,
    message: Option<String>,
    error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkspaceForm {
    Create,
    Join,
}

impl Panel {
    fn is_busy(&self) -> bool {
        self.job.is_some()
    }

    fn can_close(&self) -> bool {
        !matches!(self.job, Some(PanelJob::Command | PanelJob::Restore))
    }
}

#[derive(Clone, Copy)]
enum PanelJob {
    Refresh,
    Command,
    Restore,
}

struct ServerEditor {
    url: Entity<InputState>,
    key: Entity<InputState>,
}

impl ServerEditor {
    fn new(server: Option<&CloudServer>, window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let url = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://cloud.example.com")
                .default_value(server.map_or("", |server| server.url.as_str()).to_owned())
        });
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Publishable or anon key")
                .default_value(server.map_or("", |server| server.key.as_str()).to_owned())
        });
        url.read(cx).focus_handle(cx).focus(window, cx);
        Self { url, key }
    }
}

struct Publication {
    name: String,
    changes: Changes,
    preview: Option<String>,
}

struct LocalView {
    requests: Vec<Entry>,
    request_tabs: Vec<usize>,
    collections: Vec<Collection>,
    globals: Vec<Variable>,
    environment: Vec<Variable>,
    collapsed: HashSet<usize>,
    selected: usize,
}

struct Mounted {
    team: Team,
    generation: Uuid,
    path: PathBuf,
    cache: Cache,
    group_ids: HashMap<usize, Uuid>,
    request_ids: HashMap<usize, Uuid>,
    sync_error: Option<String>,
    syncing: bool,
    compare: Option<Uuid>,
}

enum Outcome {
    SignedIn(CloudApi, Result<Vec<Team>>),
    SignedOut,
    ConfirmEmail,
    Teams(Vec<Team>),
    WorkspaceAdded(Team),
    Open(Team, PathBuf, Cache, Vec<Member>),
    Members(Vec<Member>),
    Invited(Uuid),
    Published,
}

impl Workspace {
    pub(super) fn cloud_is_open(&self) -> bool {
        self.cloud.panel.is_some()
    }

    pub(super) fn in_team(&self) -> bool {
        self.cloud.mounted.is_some()
    }

    pub(super) fn team_can_edit(&self) -> bool {
        self.cloud
            .mounted
            .as_ref()
            .is_none_or(|mounted| mounted.team.role.can_edit())
    }

    pub(super) fn open_cloud(
        &mut self,
        publish_root: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .cloud
            .panel
            .as_ref()
            .is_some_and(|panel| !panel.can_close())
        {
            return;
        }
        if self.cloud.panel.is_some() {
            self.close_cloud(window, cx);
            if publish_root.is_none() {
                return;
            }
        }
        let input = |placeholder, cx: &mut Context<Self>, window: &mut Window| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let password = input("Password", cx, window);
        password.update(cx, |state, cx| state.set_masked(true, window, cx));
        let (server, error) = if self.cloud.api.is_none() {
            match self.load_cloud_server() {
                Ok(server) => (server, None),
                Err(error) => (None, Some(format!("{error:#}"))),
            }
        } else {
            (None, None)
        };
        let publication = publish_root.and_then(|root| self.prepare_publication(root));
        let server_editor = (self.cloud.api.is_none() && server.is_none())
            .then(|| ServerEditor::new(None, window, cx));
        self.cloud.panel = Some(Panel {
            server,
            server_editor,
            email: input("you@example.com", cx, window),
            password,
            name: input("e.g. Platform API", cx, window),
            code: input("Paste your invitation code", cx, window),
            invite_email: input("teammate@example.com", cx, window),
            focus: cx.focus_handle(),
            signup: false,
            browsing_workspaces: self.cloud.mounted.is_none() || publication.is_some(),
            workspace_form: None,
            invite_role: Role::Editor,
            invite_copied: false,
            removing_member: None,
            publication,
            members: Vec::new(),
            invite_code: None,
            job: None,
            message: None,
            error,
        });
        self.variable_hover = None;
        self.sync_visualizer(window, cx);
        if self.cloud.api.is_some() {
            if let Some(panel) = &self.cloud.panel {
                panel.focus.focus(window, cx);
            }
            self.cloud_refresh(window, cx);
        } else {
            self.restore_cloud_session(window, cx);
            if let Some(panel) = &self.cloud.panel {
                if matches!(panel.job, Some(PanelJob::Restore)) {
                    panel.focus.focus(window, cx);
                } else if panel.server.is_some() {
                    panel.email.read(cx).focus_handle(cx).focus(window, cx);
                }
            }
        }
        cx.notify();
    }

    fn cloud_refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Some(panel)) = (self.cloud.api.clone(), self.cloud.panel.as_ref()) else {
            return;
        };
        if panel.browsing_workspaces {
            self.cloud_job(PanelJob::Refresh, window, cx, move || {
                Ok(Outcome::Teams(api.workspaces()?))
            });
        } else if let Some(mounted) = &self.cloud.mounted {
            if mounted.team.role == Role::Owner {
                let id = mounted.team.id;
                self.cloud_job(PanelJob::Refresh, window, cx, move || {
                    Ok(Outcome::Members(api.members(id)?))
                });
            }
        }
    }

    fn cloud_browse(&mut self, browse: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.cloud.panel.as_mut().filter(|panel| !panel.is_busy()) else {
            return;
        };
        panel.browsing_workspaces = browse;
        panel.workspace_form = None;
        panel.removing_member = None;
        panel.error = None;
        panel.message = None;
        panel.focus.focus(window, cx);
        self.cloud_refresh(window, cx);
        cx.notify();
    }

    pub(super) fn dismiss_cloud_panel(&mut self) -> bool {
        if self
            .cloud
            .panel
            .as_ref()
            .is_some_and(|panel| !panel.can_close())
        {
            return false;
        }
        self.cloud.panel = None;
        true
    }

    fn close_cloud(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dismiss_cloud_panel() {
            self.sync_visualizer(window, cx);
            if self.request_view_visible() {
                self.focus_url(window, cx);
            }
            cx.notify();
        }
    }

    fn cloud_job(
        &mut self,
        kind: PanelJob,
        window: &mut Window,
        cx: &mut Context<Self>,
        job: impl FnOnce() -> Result<Outcome> + Send + 'static,
    ) {
        let Some(panel) = self.cloud.panel.as_mut().filter(|panel| !panel.is_busy()) else {
            return;
        };
        panel.job = Some(kind);
        panel.error = None;
        panel.message = None;
        let panel_id = panel.email.entity_id();
        cx.spawn_in(window, async move |view, cx| {
            let result = cx.background_spawn(async move { job() }).await;
            _ = view.update_in(cx, |view, window, cx| {
                let Some(panel) = view
                    .cloud
                    .panel
                    .as_mut()
                    .filter(|panel| panel.email.entity_id() == panel_id)
                else {
                    return;
                };
                panel.job = None;
                match result {
                    Err(error) => {
                        if matches!(kind, PanelJob::Restore) {
                            panel.email.read(cx).focus_handle(cx).focus(window, cx);
                        }
                        panel.error = Some(format!("{error:#}"));
                    }
                    Ok(Outcome::ConfirmEmail) => {
                        panel.signup = false;
                        panel.email.read(cx).focus_handle(cx).focus(window, cx);
                        panel.message =
                            Some("Check your email to confirm your account, then sign in.".into());
                    }
                    Ok(Outcome::SignedIn(api, teams)) => {
                        view.cloud.api = Some(api);
                        panel.focus.focus(window, cx);
                        match teams {
                            Ok(teams) => {
                                view.cloud.teams_loaded = true;
                                view.cloud.teams = teams;
                            }
                            Err(error) => panel.error = Some(format!("{error:#}")),
                        }
                    }
                    Ok(Outcome::SignedOut) => {
                        view.cloud.api = None;
                        view.cloud.teams.clear();
                        view.cloud.teams_loaded = false;
                        view.cloud.panel = None;
                        view.open_cloud(None, window, cx);
                    }
                    Ok(Outcome::Teams(teams)) => {
                        view.cloud.teams_loaded = true;
                        view.cloud.teams = teams;
                    }
                    Ok(Outcome::WorkspaceAdded(team)) => {
                        if let Some(form) = panel.workspace_form.take() {
                            let (input, message) = match form {
                                WorkspaceForm::Create => (&panel.name, "Workspace created"),
                                WorkspaceForm::Join => (&panel.code, "Joined workspace"),
                            };
                            input.update(cx, |input, cx| input.set_value("", window, cx));
                            panel.focus.focus(window, cx);
                            panel.message = Some(format!("{message} · {}", team.name));
                        }
                        view.cloud.teams.retain(|item| item.id != team.id);
                        view.cloud.teams.push(team);
                    }
                    Ok(Outcome::Open(team, path, cache, members)) => {
                        panel.members = members;
                        if let Err(error) = view.mount_team(team, path, cache, window, cx) {
                            view.cloud.panel.as_mut().unwrap().error = Some(format!("{error:#}"));
                        }
                    }
                    Ok(Outcome::Members(members)) => {
                        panel.members = members;
                        panel.removing_member = None;
                    }
                    Ok(Outcome::Invited(code)) => {
                        panel.invite_code = Some(code);
                        panel.invite_copied = false;
                    }
                    Ok(Outcome::Published) => {
                        panel.publication = None;
                        panel.message =
                            Some("Collection published. Open the workspace to collaborate.".into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn load_cloud_server(&self) -> Result<Option<CloudServer>> {
        if let Ok(storage) = &self.storage
            && let Some(server) = storage.load_cloud_server()?
        {
            return Ok(Some(server));
        }
        CloudServer::from_environment()
    }

    fn restore_cloud_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(panel), Ok(storage)) = (&mut self.cloud.panel, &self.storage) else {
            return;
        };
        let Some(server) = panel.server.clone() else {
            return;
        };
        let store = storage.cloud_session_store();
        match store.load(&server.url) {
            Ok(Some(token)) => self.cloud_job(PanelJob::Restore, window, cx, move || {
                let api = CloudConfig::new(server)?.restore(store, &token)?;
                let teams = api.workspaces();
                Ok(Outcome::SignedIn(api, teams))
            }),
            Ok(None) => {}
            Err(error) => panel.error = Some(format!("{error:#}")),
        }
    }

    fn cloud_edit_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.cloud.panel.as_mut() else {
            return;
        };
        panel
            .password
            .update(cx, |input, cx| input.set_value("", window, cx));
        panel.server_editor = Some(ServerEditor::new(panel.server.as_ref(), window, cx));
        panel.error = None;
        panel.message = None;
        cx.notify();
    }

    fn cloud_cancel_server_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(panel) = self
            .cloud
            .panel
            .as_mut()
            .filter(|panel| panel.server.is_some())
        {
            panel.server_editor = None;
            panel.error = None;
            panel.email.read(cx).focus_handle(cx).focus(window, cx);
            cx.notify();
        }
    }

    fn cloud_save_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.cloud.panel else {
            return;
        };
        let Some(editor) = &panel.server_editor else {
            return;
        };
        let result = CloudServer::parse(&editor.url.read(cx).value(), &editor.key.read(cx).value())
            .and_then(|server| {
                self.storage
                    .as_ref()
                    .map_err(|error| anyhow::anyhow!("{error}"))?
                    .save_cloud_server(&server)?;
                Ok(server)
            });
        match result {
            Ok(server) => {
                panel.server = Some(server);
                panel.server_editor = None;
                panel.error = None;
                panel.email.read(cx).focus_handle(cx).focus(window, cx);
            }
            Err(error) => panel.error = Some(format!("{error:#}")),
        }
        cx.notify();
    }

    fn cloud_sign_in(&mut self, signup: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self
            .cloud
            .panel
            .as_ref()
            .filter(|panel| panel.server_editor.is_none() && !panel.is_busy())
        else {
            return;
        };
        let Some(server) = panel.server.clone() else {
            return;
        };
        let email = panel.email.read(cx).value().trim().to_owned();
        let password = panel.password.read(cx).value().to_string();
        panel
            .password
            .update(cx, |input, cx| input.set_value("", window, cx));
        let store = self.storage.as_ref().ok().map(Storage::cloud_session_store);
        self.cloud_job(PanelJob::Command, window, cx, move || {
            ensure!(
                !email.is_empty() && !password.is_empty(),
                "Enter your email and password."
            );
            let api = CloudConfig::new(server)?.authenticate(&email, &password, signup, store)?;
            match api {
                Some(api) => {
                    let teams = api.workspaces();
                    Ok(Outcome::SignedIn(api, teams))
                }
                None => Ok(Outcome::ConfirmEmail),
            }
        });
    }

    fn cloud_workspace_form(
        &mut self,
        form: Option<WorkspaceForm>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = self.cloud.panel.as_mut().filter(|panel| !panel.is_busy()) else {
            return;
        };
        panel.workspace_form = form;
        panel.error = None;
        panel.message = None;
        if let Some(form) = form {
            let input = match form {
                WorkspaceForm::Create => &panel.name,
                WorkspaceForm::Join => &panel.code,
            };
            input.read(cx).focus_handle(cx).focus(window, cx);
        } else {
            panel.focus.focus(window, cx);
        }
        cx.notify();
    }

    fn cloud_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Some(panel)) = (self.cloud.api.clone(), self.cloud.panel.as_ref()) else {
            return;
        };
        let name = panel.name.read(cx).value().trim().to_owned();
        self.cloud_job(PanelJob::Command, window, cx, move || {
            ensure!(!name.is_empty(), "Enter a workspace name.");
            Ok(Outcome::WorkspaceAdded(api.create(&name)?))
        });
    }

    fn cloud_accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Some(panel)) = (self.cloud.api.clone(), self.cloud.panel.as_ref()) else {
            return;
        };
        let code = panel.code.read(cx).value().trim().to_owned();
        self.cloud_job(PanelJob::Command, window, cx, move || {
            Ok(Outcome::WorkspaceAdded(api.accept(
                Uuid::parse_str(&code).context("Enter a valid invitation code.")?,
            )?))
        });
    }

    fn cloud_invite(&mut self, role: Role, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Some(panel), Some(mounted)) = (
            self.cloud.api.clone(),
            self.cloud.panel.as_ref(),
            self.cloud.mounted.as_ref(),
        ) else {
            return;
        };
        let email = panel.invite_email.read(cx).value().trim().to_owned();
        let id = mounted.team.id;
        self.cloud_job(PanelJob::Command, window, cx, move || {
            ensure!(!email.is_empty(), "Enter your teammate's email address.");
            Ok(Outcome::Invited(api.invite(id, &email, role)?))
        });
    }

    fn cloud_member_role(
        &mut self,
        member: Uuid,
        role: Option<Role>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(api), Some(mounted)) = (self.cloud.api.clone(), self.cloud.mounted.as_ref())
        else {
            return;
        };
        let id = mounted.team.id;
        self.cloud_job(PanelJob::Command, window, cx, move || {
            Ok(Outcome::Members(api.set_role(id, member, role)?))
        });
    }

    fn cloud_open_team(&mut self, mut team: Team, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Ok(storage)) = (self.cloud.api.clone(), &self.storage) else {
            return;
        };
        if self
            .cloud
            .mounted
            .as_ref()
            .is_some_and(|mounted| mounted.team.id == team.id)
        {
            self.close_cloud(window, cx);
            return;
        }
        let path = storage.cloud_cache_path(&api.cache_key(), team.id);
        self.cloud_job(PanelJob::Command, window, cx, move || {
            let mut cache = Cache::open(&path)?;
            let result = api.sync(team.id, &Changes::new(), cache.remote_revision)?;
            team.role = result.role;
            cache.acknowledge(result, &Changes::new());
            let members = if team.role == Role::Owner {
                api.members(team.id)?
            } else {
                Vec::new()
            };
            Ok(Outcome::Open(team, path, cache, members))
        });
    }

    fn prepare_publication(&self, root: usize) -> Option<Publication> {
        let name = self
            .collections
            .iter()
            .find(|group| group.id == root && group.parent_id.is_none())?
            .name
            .clone();
        let descendants = crate::storage::descendants(&self.collections, root);
        let ids: HashMap<_, _> = descendants
            .into_iter()
            .map(|id| (id, Uuid::new_v4()))
            .collect();
        let group = |collection: &Collection| Content::Group {
            parent: collection
                .parent_id
                .and_then(|parent| ids.get(&parent).copied()),
            name: collection.name.clone(),
            variables: collection
                .variables
                .iter()
                .cloned()
                .map(|mut row| {
                    row.value.clear();
                    row
                })
                .collect(),
        };
        let groups = self
            .collections
            .iter()
            .filter_map(|collection| Some((*ids.get(&collection.id)?, group(collection))));
        let requests = self.requests.iter().filter_map(|entry| {
            let parent = ids.get(&entry.collection_id?)?;
            Some((
                Uuid::new_v4(),
                Content::Request {
                    parent: Some(*parent),
                    request: Box::new(share_request(entry.draft.clone())),
                },
            ))
        });
        let changes = groups
            .chain(requests)
            .map(|(id, content)| {
                (
                    id,
                    crate::cloud::Change {
                        expected: None,
                        content: Some(content),
                    },
                )
            })
            .collect();
        Some(Publication {
            name,
            changes,
            preview: None,
        })
    }

    fn cloud_publish(&mut self, team: Team, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Some(publication)) = (
            self.cloud.api.clone(),
            self.cloud
                .panel
                .as_ref()
                .and_then(|panel| panel.publication.as_ref()),
        ) else {
            return;
        };
        let changes = publication.changes.clone();
        self.cloud_job(PanelJob::Command, window, cx, move || {
            let result = api.sync(team.id, &changes, None)?;
            ensure!(
                result.conflicts.is_empty(),
                "Could not publish the collection. Try again."
            );
            Ok(Outcome::Published)
        });
    }

    fn mount_team(
        &mut self,
        team: Team,
        path: PathBuf,
        cache: Cache,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        cache.save(&path)?;
        if self.in_team() {
            self.leave_team(window, cx)?;
        }
        self.save_on_exit(cx)?;
        self.exit_saved = false;
        self.cloud.local = Some(LocalView {
            requests: std::mem::take(&mut self.requests),
            request_tabs: std::mem::take(&mut self.request_tabs),
            collections: std::mem::take(&mut self.collections),
            globals: std::mem::take(&mut self.global_variables),
            environment: std::mem::take(&mut self.environment_variables),
            collapsed: std::mem::take(&mut self.collapsed_collections),
            selected: self.selected,
        });
        self.global_variables = cache.globals.clone();
        self.environment_variables = cache.environment.clone();
        let generation = Uuid::new_v4();
        self.cloud.mounted = Some(Mounted {
            team,
            generation,
            path,
            cache,
            group_ids: HashMap::new(),
            request_ids: HashMap::new(),
            sync_error: None,
            syncing: false,
            compare: None,
        });
        self.reset_team_view();
        self.refresh_team_projection(true, window, cx);
        self.close_cloud(window, cx);
        self.start_team_sync(generation, window, cx);
        Ok(())
    }

    fn leave_team(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<()> {
        if !self.in_team() {
            return Ok(());
        }
        self.save_team_cache(false, None)?;
        let local = self
            .cloud
            .local
            .take()
            .context("Could not restore your local workspace.")?;
        self.cloud.mounted = None;
        self.requests = local.requests;
        self.request_tabs = local.request_tabs;
        self.collections = local.collections;
        self.global_variables = local.globals;
        self.environment_variables = local.environment;
        self.collapsed_collections = local.collapsed;
        self.selected = local.selected;
        self.reset_team_view();
        self.restore_selection(window, cx);
        Ok(())
    }

    fn reset_team_view(&mut self) {
        self.openapi_import = None;
        self.variable_editor = None;
        self.collection_edit = None;
        self.request_rename = None;
        self.request_pane = RequestPane::Body;
        self.response_pane = Pane::Body;
        self.visualizer_document = None;
    }

    pub(super) fn team_group_version(&self, id: usize) -> Option<ItemRevision> {
        let mounted = self.cloud.mounted.as_ref()?;
        let &key = mounted.group_ids.get(&id)?;
        Some((key, mounted.cache.revision(key)))
    }

    pub(super) fn save_team_cache(
        &mut self,
        stage: bool,
        base: Option<ItemRevision>,
    ) -> Result<()> {
        let mounted = self
            .cloud
            .mounted
            .as_mut()
            .context("Open a team workspace first.")?;
        let mut next = mounted.cache.clone();
        for group in &self.collections {
            mounted
                .group_ids
                .entry(group.id)
                .or_insert_with(Uuid::new_v4);
        }
        for entry in &self.requests {
            if entry.saved.is_some()
                || entry.draft != Request::default()
                || entry.collection_id.is_some()
                || self.request_tabs.contains(&entry.id)
            {
                mounted
                    .request_ids
                    .entry(entry.id)
                    .or_insert_with(Uuid::new_v4);
            }
        }
        if stage {
            ensure!(
                next.structural_conflicts().is_empty(),
                "Resolve conflicting collection changes before publishing new edits."
            );
            let groups = self.collections.iter().map(|group| {
                (
                    mounted.group_ids[&group.id],
                    Content::Group {
                        parent: group
                            .parent_id
                            .and_then(|id| mounted.group_ids.get(&id).copied()),
                        name: group.name.clone(),
                        variables: group.variables.clone(),
                    },
                )
            });
            let requests = self.requests.iter().filter_map(|entry| {
                let saved = entry.saved.as_ref()?;
                Some((
                    mounted.request_ids[&entry.id],
                    Content::Request {
                        parent: entry
                            .collection_id
                            .and_then(|id| mounted.group_ids.get(&id).copied()),
                        request: Box::new(share_request(saved.clone())),
                    },
                ))
            });
            let desired = groups.chain(requests).collect();
            ensure!(
                mounted.team.role.can_edit() || next.working() == desired,
                "This workspace is read-only."
            );
            next.stage(desired, base);
        }
        next.globals = self.global_variables.clone();
        next.environment = self.environment_variables.clone();
        next.sidebar = SidebarState {
            collapsed_collections: self
                .collections
                .iter()
                .filter(|group| self.collapsed_collections.contains(&group.id))
                .map(|group| mounted.group_ids[&group.id])
                .collect(),
            selected_request: self
                .requests
                .get(self.selected)
                .and_then(|entry| mounted.request_ids.get(&entry.id))
                .copied(),
            request_tabs: Some(
                self.request_tabs
                    .iter()
                    .filter_map(|id| mounted.request_ids.get(id).copied())
                    .collect(),
            ),
        };
        next.drafts = self
            .requests
            .iter()
            .filter_map(|entry| {
                let &id = mounted.request_ids.get(&entry.id)?;
                (entry.is_dirty() || share_request(entry.draft.clone()) != entry.draft).then(|| {
                    (
                        id,
                        Draft {
                            expected: next
                                .drafts
                                .get(&id)
                                .map_or_else(|| next.revision(id), |draft| draft.expected),
                            parent: entry
                                .collection_id
                                .and_then(|id| mounted.group_ids.get(&id).copied()),
                            request: entry.draft.clone(),
                        },
                    )
                })
            })
            .collect();
        next.save(&mounted.path)?;
        mounted.cache = next;
        Ok(())
    }

    fn refresh_team_projection(
        &mut self,
        restore_sidebar: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mounted) = self.cloud.mounted.as_mut() else {
            return;
        };
        let working = mounted.cache.working();
        let group_ids = &mut mounted.group_ids;
        let mut group_by_key: HashMap<_, _> =
            group_ids.iter().map(|(&id, &key)| (key, id)).collect();
        let mut groups = Vec::new();
        for (&key, content) in &working {
            if let Content::Group {
                parent,
                name,
                variables,
            } = content
            {
                let id = *group_by_key.entry(key).or_insert_with(|| {
                    let id = self.next_collection_id;
                    self.next_collection_id += 1;
                    group_ids.insert(id, key);
                    id
                });
                groups.push((id, *parent, name.clone(), variables.clone()));
            }
        }
        self.collections = groups
            .into_iter()
            .map(|(id, parent, name, variables)| Collection {
                id,
                parent_id: parent.and_then(|parent| group_by_key.get(&parent).copied()),
                name,
                variables,
            })
            .collect();
        self.collections
            .sort_by(|left, right| left.name.cmp(&right.name));
        if restore_sidebar {
            self.collapsed_collections = mounted
                .cache
                .sidebar
                .collapsed_collections
                .iter()
                .filter_map(|key| group_by_key.get(key).copied())
                .collect();
        }

        let selected = self
            .requests
            .get(self.selected)
            .map(|entry| (entry.id, entry.draft.clone()));
        let mut previous: HashMap<_, _> = self
            .requests
            .drain(..)
            .map(|entry| (entry.id, entry))
            .collect();
        let request_ids = &mut mounted.request_ids;
        let mut request_by_key: HashMap<_, _> =
            request_ids.iter().map(|(&id, &key)| (key, id)).collect();
        let mut contents: BTreeMap<_, _> = working
            .iter()
            .filter_map(|(&key, content)| {
                let Content::Request { parent, request } = content else {
                    return None;
                };
                Some((
                    key,
                    (
                        *parent,
                        Some(request.as_ref().clone()),
                        request.as_ref().clone(),
                    ),
                ))
            })
            .collect();
        for (&key, draft) in &mounted.cache.drafts {
            let saved = contents.remove(&key).and_then(|(_, saved, _)| saved);
            contents.insert(key, (draft.parent, saved, draft.request.clone()));
        }
        self.requests = contents
            .into_iter()
            .map(|(key, (parent, saved, draft))| {
                let id = *request_by_key.entry(key).or_insert_with(|| {
                    let id = self.next_request_id;
                    self.next_request_id += 1;
                    request_ids.insert(id, key);
                    id
                });
                let parent = parent
                    .and_then(|key| group_by_key.get(&key).copied())
                    .filter(|id| self.collections.iter().any(|group| group.id == *id));
                if let Some(mut entry) = previous.remove(&id) {
                    entry.collection_id = parent;
                    entry.replace_draft(draft);
                    entry.saved = saved;
                    entry
                } else {
                    Entry::new(id, parent, draft, saved)
                }
            })
            .collect();
        self.requests
            .sort_by(|left, right| left.draft.name.cmp(&right.draft.name));
        if self.requests.is_empty() {
            if let Some(entry) = previous
                .into_values()
                .find(|entry| entry.saved.is_none() && entry.draft == Request::default())
            {
                self.requests.push(entry);
            } else {
                self.requests.push(Entry::new(
                    self.next_request_id,
                    None,
                    Request::default(),
                    None,
                ));
                self.next_request_id += 1;
            }
        }
        if restore_sidebar {
            let indexes: HashMap<_, _> = self
                .requests
                .iter()
                .enumerate()
                .map(|(index, entry)| (entry.id, index))
                .collect();
            let (tabs, selected) = mounted.cache.sidebar.restore_request_tabs(|key| {
                request_by_key
                    .get(key)
                    .and_then(|id| indexes.get(id).copied())
            });
            self.request_tabs = tabs
                .into_iter()
                .map(|index| self.requests[index].id)
                .collect();
            self.selected = selected;
        } else {
            self.selected = selected
                .as_ref()
                .and_then(|(id, _)| self.request_index(*id))
                .unwrap_or_default();
            self.reconcile_request_tabs();
        }
        if restore_sidebar
            || selected
                .as_ref()
                .is_none_or(|(id, draft)| self.active().id != *id || &self.active().draft != draft)
        {
            self.request_rename = None;
            self.restore_selection(window, cx);
        }
    }

    fn start_team_sync(&mut self, generation: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |view, cx| {
            loop {
                let _ = view.update_in(cx, |view, window, cx| {
                    view.sync_team(generation, window, cx);
                });
                cx.background_executor().timer(Duration::from_secs(4)).await;
                let Ok(true) = view.update_in(cx, |view, _, _| {
                    view.cloud
                        .mounted
                        .as_ref()
                        .is_some_and(|mounted| mounted.generation == generation)
                }) else {
                    break;
                };
            }
        })
        .detach();
    }

    fn sync_team(&mut self, generation: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Some(mounted)) = (self.cloud.api.clone(), self.cloud.mounted.as_mut())
        else {
            return;
        };
        if mounted.syncing || mounted.generation != generation {
            return;
        }
        let team = mounted.team.id;
        let revision = mounted.cache.remote_revision;
        let sent = if mounted.team.role.can_edit() && mounted.cache.conflicts().is_empty() {
            mounted.cache.pending.clone()
        } else {
            Changes::new()
        };
        mounted.syncing = true;
        cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_spawn(async move {
                    let result = api.sync(team, &sent, revision);
                    let pulled = if result.is_err() && !sent.is_empty() {
                        Some(api.sync(team, &Changes::new(), revision))
                    } else {
                        None
                    };
                    (result, pulled, sent)
                })
                .await;
            _ = view.update_in(cx, |view, window, cx| {
                view.finish_team_sync(generation, result, window, cx)
            });
        })
        .detach();
        cx.notify();
    }

    fn finish_team_sync(
        &mut self,
        generation: Uuid,
        (result, pulled, sent): (Result<SyncResult>, Option<Result<SyncResult>>, Changes),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mounted) = self
            .cloud
            .mounted
            .as_mut()
            .filter(|mounted| mounted.generation == generation)
        else {
            return;
        };
        mounted.syncing = false;
        if let Err(error) = &result {
            mounted.sync_error = Some(format!("{error:#}"));
            let access_revoked = if sent.is_empty() {
                error.is::<AccessDenied>()
            } else {
                matches!(&pulled, Some(Err(error)) if error.is::<AccessDenied>())
            };
            if access_revoked {
                let id = mounted.team.id;
                match self.leave_team(window, cx) {
                    Ok(()) => {
                        self.cloud.teams.retain(|team| team.id != id);
                        self.cloud.panel = None;
                        self.save_error =
                            Some("Your access to this team workspace was removed.".into());
                    }
                    Err(cache_error) => {
                        let mounted = self.cloud.mounted.as_mut().unwrap();
                        mounted.team.role = Role::Viewer;
                        mounted.sync_error = Some(format!("{error:#} {cache_error:#}"));
                    }
                }
                cx.notify();
                return;
            }
        }
        let write_failed = result.is_err();
        let (result, sent) = match (result, pulled) {
            (Ok(result), _) => (result, sent),
            (Err(_), Some(Ok(result))) => (result, Changes::new()),
            (Err(_), _) => {
                cx.notify();
                return;
            }
        };
        let changed = result.tree.is_some() || !sent.is_empty();
        mounted.team.role = result.role;
        if !changed {
            if !write_failed || !mounted.team.role.can_edit() {
                mounted.sync_error = None;
            }
            cx.notify();
            return;
        }
        if let Err(error) = self.save_team_cache(false, None) {
            self.cloud.mounted.as_mut().unwrap().sync_error = Some(format!("{error:#}"));
            cx.notify();
            return;
        }
        let mounted = self.cloud.mounted.as_mut().unwrap();
        let mut next = mounted.cache.clone();
        next.acknowledge(result, &sent);
        match next.save(&mounted.path) {
            Ok(()) => {
                mounted.cache = next;
                if !write_failed || !mounted.team.role.can_edit() {
                    mounted.sync_error = None;
                }
                self.refresh_team_projection(false, window, cx);
            }
            Err(error) => mounted.sync_error = Some(format!("{error:#}")),
        }
        cx.notify();
    }

    pub(super) fn team_collection_values(&self, id: Option<usize>, values: &mut Vec<Variable>) {
        let Some(overrides) = self.cloud.mounted.as_ref().and_then(|mounted| {
            mounted
                .cache
                .collection_values
                .get(mounted.group_ids.get(&id?)?)
        }) else {
            return;
        };
        for (name, value) in overrides {
            scripting::Mutation {
                scope: String::new(),
                name: name.clone(),
                value: value.clone(),
            }
            .apply(values);
        }
    }

    pub(super) fn set_team_collection_value(
        &mut self,
        id: Option<usize>,
        mutation: &scripting::Mutation,
    ) {
        if let Some(mounted) = &mut self.cloud.mounted
            && let Some(&group) = id.and_then(|id| mounted.group_ids.get(&id))
        {
            mounted
                .cache
                .collection_values
                .entry(group)
                .or_default()
                .insert(mutation.name.clone(), mutation.value.clone());
        }
    }
}

impl Workspace {
    fn cloud_return_local(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.leave_team(window, cx) {
            Ok(()) => self.close_cloud(window, cx),
            Err(error) => {
                if let Some(panel) = &mut self.cloud.panel {
                    panel.error = Some(format!("{error:#}"));
                } else if let Some(mounted) = &mut self.cloud.mounted {
                    mounted.sync_error = Some(format!("{error:#}"));
                }
                cx.notify();
            }
        }
    }

    fn cloud_sign_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.cloud.api.clone() else {
            return;
        };
        if let Err(error) = self.leave_team(window, cx) {
            if let Some(panel) = &mut self.cloud.panel {
                panel.error = Some(format!("{error:#}"));
            }
        } else {
            self.cloud_job(PanelJob::Command, window, cx, move || {
                api.sign_out()?;
                Ok(Outcome::SignedOut)
            });
        }
        cx.notify();
    }

    fn resolve_cloud_conflict(
        &mut self,
        id: Uuid,
        copy: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.save_team_cache(false, None) {
            self.save_error = Some(format!("{error:#}"));
            cx.notify();
            return;
        }
        let Some(mounted) = &mut self.cloud.mounted else {
            return;
        };
        if !mounted.cache.conflicts().contains(&id) {
            return;
        }
        mounted.compare = None;
        let mut next = mounted.cache.clone();
        let change = next.pending.remove(&id);
        let draft = next.drafts.remove(&id);
        if copy {
            let (parent, mut request) = match (draft, change.and_then(|change| change.content)) {
                (Some(draft), _) => (draft.parent, draft.request),
                (None, Some(Content::Request { parent, request })) => (parent, *request),
                _ => return,
            };
            request.name = format!("{} (copy)", request.title());
            let parent =
                parent.filter(|id| matches!(next.working().get(id), Some(Content::Group { .. })));
            let key = Uuid::new_v4();
            next.pending.insert(
                key,
                crate::cloud::Change {
                    expected: None,
                    content: Some(Content::Request {
                        parent,
                        request: Box::new(share_request(request.clone())),
                    }),
                },
            );
            if share_request(request.clone()) != request {
                next.drafts.insert(
                    key,
                    Draft {
                        parent,
                        request,
                        expected: None,
                    },
                );
            }
        }
        match next.save(&mounted.path) {
            Ok(()) => {
                mounted.cache = next;
                self.refresh_team_projection(false, window, cx);
            }
            Err(error) => self.save_error = Some(format!("{error:#}")),
        }
        cx.notify();
    }

    pub(super) fn comparing_cloud(&self) -> bool {
        self.cloud
            .mounted
            .as_ref()
            .is_some_and(|mounted| mounted.compare.is_some())
    }

    fn set_cloud_comparison(
        &mut self,
        id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(mounted) = self.cloud.mounted.as_mut() {
            mounted.compare = id;
            self.sync_visualizer(window, cx);
            cx.notify();
        }
    }
}
