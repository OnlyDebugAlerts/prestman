use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, BufReader, BufWriter, Write},
    path::Path,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    request::{Authorization, Pair, Request, merge_query_rows, replace_query},
    storage::SidebarState,
    variables::Variable,
};

use super::Role;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Content {
    Group {
        parent: Option<Uuid>,
        name: String,
        // Older servers require this field. Ignore values from older clients.
        #[serde(default, skip_deserializing)]
        variables: [(); 0],
    },
    Request {
        parent: Option<Uuid>,
        request: Box<Request>,
    },
}

impl Content {
    pub fn parent(&self) -> Option<Uuid> {
        match self {
            Self::Group { parent, .. } | Self::Request { parent, .. } => *parent,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Group { name, .. } => name,
            Self::Request { request, .. } => request.title(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub revision: u64,
    pub content: Content,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub expected: Option<u64>,
    pub content: Option<Content>,
}

pub type Tree = BTreeMap<Uuid, Item>;
pub type Changes = BTreeMap<Uuid, Change>;
pub type ItemRevision = (Uuid, Option<u64>);

#[derive(Deserialize)]
pub struct SyncResult {
    pub tree: Option<Tree>,
    pub revision: u64,
    pub conflicts: Vec<Uuid>,
    pub role: Role,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Draft {
    pub expected: Option<u64>,
    pub parent: Option<Uuid>,
    pub request: Request,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Cache {
    pub remote: Tree,
    pub remote_revision: Option<u64>,
    pub pending: Changes,
    pub drafts: BTreeMap<Uuid, Draft>,
    // Preserve existing on-device values for recovery from older caches.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    globals: Vec<Variable>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    environment: Vec<Variable>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    collection_values: BTreeMap<Uuid, BTreeMap<String, Option<String>>>,
    #[serde(default)]
    pub use_local_workspace_variables: bool,
    #[serde(default)]
    pub sidebar: SidebarState<Uuid>,
}

impl Cache {
    pub fn open(path: &Path) -> Result<Self> {
        match File::open(path) {
            Ok(file) => {
                let mut cache: Self = serde_json::from_reader(BufReader::new(file))
                    .context("Could not read the local cloud workspace cache.")?;
                cache.pending.retain(|id, change| {
                    !matches!(change.content, Some(Content::Group { .. }))
                        || cache.remote.get(id).map(|item| &item.content) != change.content.as_ref()
                });
                Ok(cache)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).context("Could not open the local cloud workspace cache."),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let directory = path.parent().context("Invalid cloud cache path.")?;
        fs::create_dir_all(directory)?;
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        {
            let mut writer = BufWriter::new(file.as_file_mut());
            serde_json::to_writer(&mut writer, self)?;
            writer.flush()?;
        }
        file.as_file().sync_all()?;
        file.persist(path)
            .context("Could not save the local cloud workspace.")?;
        Ok(())
    }

    pub fn working(&self) -> BTreeMap<Uuid, Content> {
        let mut working: BTreeMap<_, _> = self
            .remote
            .iter()
            .map(|(&id, item)| (id, item.content.clone()))
            .collect();
        for (&id, change) in &self.pending {
            if let Some(content) = &change.content {
                working.insert(id, content.clone());
            } else {
                working.remove(&id);
            }
        }
        working
    }

    pub fn revision(&self, id: Uuid) -> Option<u64> {
        self.remote.get(&id).map(|item| item.revision)
    }

    pub fn stage(&mut self, desired: BTreeMap<Uuid, Content>, base: Option<ItemRevision>) {
        let previous = self.working();
        for id in previous
            .keys()
            .chain(desired.keys().filter(|id| !previous.contains_key(id)))
        {
            let content = desired.get(id);
            if previous.get(id) == content {
                continue;
            }
            let expected = self
                .pending
                .get(id)
                .map(|change| change.expected)
                .or_else(|| self.drafts.get(id).map(|draft| draft.expected))
                .or_else(|| {
                    base.filter(|(key, _)| key == id)
                        .map(|(_, revision)| revision)
                })
                .unwrap_or_else(|| self.revision(*id));
            self.pending.insert(
                *id,
                Change {
                    expected,
                    content: content.cloned(),
                },
            );
        }
    }

    pub fn conflicts(&self) -> Vec<Uuid> {
        let mut conflicts: BTreeSet<_> = self
            .pending
            .iter()
            .filter_map(|(&id, change)| {
                let remote = self.remote.get(&id);
                (remote.map(|item| item.revision) != change.expected
                    && remote.map(|item| &item.content) != change.content.as_ref())
                .then_some(id)
            })
            .collect();
        conflicts.extend(self.structural_conflicts());
        conflicts.into_iter().collect()
    }

    pub fn structural_conflicts(&self) -> BTreeSet<Uuid> {
        let mut conflicts = BTreeSet::new();
        if self.pending.is_empty() {
            return conflicts;
        }
        let working = self.working();
        for (&id, content) in &working {
            let mut visited = BTreeSet::from([id]);
            let mut parent = content.parent();
            while let Some(group_id) = parent {
                let fresh = visited.insert(group_id);
                let group = working.get(&group_id);
                if !fresh || visited.len() > 65 || !matches!(group, Some(Content::Group { .. })) {
                    conflicts.extend(
                        visited
                            .into_iter()
                            .filter(|id| self.pending.contains_key(id)),
                    );
                    break;
                }
                parent = group.and_then(Content::parent);
            }
        }
        conflicts
    }

    pub fn acknowledge(&mut self, result: SyncResult, sent: &Changes) {
        let applied = result.conflicts.is_empty();
        if let Some(tree) = result.tree {
            self.remote = tree;
        }
        self.remote_revision = Some(result.revision);
        if !applied {
            return;
        }
        for (id, change) in sent {
            if self.pending.get(id) == Some(change) {
                self.pending.remove(id);
            } else if let Some(pending) = self.pending.get_mut(id) {
                pending.expected = self.remote.get(id).map(|item| item.revision);
            }
            if let Some(draft) = self.drafts.get_mut(id) {
                draft.expected = self.remote.get(id).map(|item| item.revision);
            }
        }
    }
}

pub fn share_request(mut request: Request) -> Request {
    let protect = |value: &mut String, name: &str| {
        if !value.is_empty() && !variable_reference(value) {
            *value = format!("{{{{{name}}}}}");
        }
    };
    match &mut request.authorization {
        Authorization::None => {}
        Authorization::Bearer { token } => protect(token, "api_token"),
        Authorization::Basic { username, password } => {
            protect(username, "api_username");
            protect(password, "api_password");
        }
    }

    for row in &mut request.headers {
        redact(row, &protect);
    }
    let mut query = merge_query_rows(&request.url, &request.query);
    let previous_query = query.clone();
    for row in &mut query {
        redact(row, &protect);
    }
    if query != previous_query {
        request.url = replace_query(&request.url, &query);
    }
    request.query = query;
    let authority = request.url.find("://").map_or(0, |index| index + 3);
    let host = &request.url[authority..];
    let host_end = host.find(['/', '?', '#']).unwrap_or(host.len());
    if let Some(end) = host[..host_end].rfind('@') {
        request
            .url
            .replace_range(authority..authority + end + 1, "");
    }
    if let Some(proto) = request
        .grpc
        .as_mut()
        .and_then(|options| options.proto.as_mut())
    {
        proto.path = proto.path.file_name().unwrap_or_default().into();
    }
    request
}

fn variable_reference(value: &str) -> bool {
    value
        .strip_prefix("{{")
        .and_then(|value| value.strip_suffix("}}"))
        .is_some_and(|name| !name.is_empty() && !name.contains(['{', '}']))
}

fn redact(row: &mut Pair, protect: &impl Fn(&mut String, &str)) {
    let name = row.name.trim().to_ascii_lowercase().replace('-', "_");
    if matches!(
        name.as_str(),
        "authorization"
            | "proxy_authorization"
            | "cookie"
            | "set_cookie"
            | "key"
            | "api_key"
            | "apikey"
            | "x_api_key"
            | "x_auth_token"
            | "token"
            | "access_token"
            | "refresh_token"
            | "password"
            | "client_secret"
            | "signature"
            | "x_amz_credential"
            | "x_amz_signature"
            | "x_amz_security_token"
    ) {
        if name.ends_with("authorization")
            && let Some((scheme, credential)) = row.value.split_once(' ')
            && matches!(scheme.to_ascii_lowercase().as_str(), "bearer" | "basic")
        {
            let mut credential = credential.to_owned();
            protect(&mut credential, &name);
            row.value = format!("{scheme} {credential}");
        } else {
            protect(&mut row.value, &name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(url: &str, parent: Option<Uuid>) -> Content {
        Content::Request {
            parent,
            request: Box::new(Request {
                url: url.into(),
                ..Request::default()
            }),
        }
    }

    #[test]
    fn stale_drafts_keep_the_original_item_revision_after_a_pull() {
        let id = Uuid::new_v4();
        let remote = request("https://example.test/changed", None);
        let mut cache = Cache {
            remote: Tree::from([(
                id,
                Item {
                    revision: 3,
                    content: remote,
                },
            )]),
            drafts: BTreeMap::from([(
                id,
                Draft {
                    expected: Some(1),
                    parent: None,
                    request: Request::default(),
                },
            )]),
            ..Default::default()
        };
        cache.stage(
            BTreeMap::from([(id, request("https://example.test/draft", None))]),
            None,
        );
        assert_eq!(cache.pending[&id].expected, Some(1));
        assert_eq!(cache.conflicts(), vec![id]);
    }

    #[test]
    fn saving_again_during_sync_keeps_the_newer_local_edit() {
        let id = Uuid::new_v4();
        let first = request("https://example.test/first", None);
        let second = request("https://example.test/second", None);
        let mut cache = Cache::default();
        cache.stage(BTreeMap::from([(id, first.clone())]), None);
        let sent = cache.pending.clone();
        cache.stage(BTreeMap::from([(id, second.clone())]), None);
        cache.acknowledge(
            SyncResult {
                tree: Some(Tree::from([(
                    id,
                    Item {
                        revision: 1,
                        content: first,
                    },
                )])),
                revision: 1,
                conflicts: vec![],
                role: Role::Editor,
            },
            &sent,
        );
        assert_eq!(cache.pending[&id].expected, Some(1));
        assert_eq!(cache.working()[&id], second);
        assert!(cache.conflicts().is_empty());
    }

    #[test]
    fn deleting_a_folder_conflicts_with_a_concurrently_added_child() {
        let group = Uuid::new_v4();
        let child = Uuid::new_v4();
        let mut cache = Cache {
            remote: Tree::from([(
                group,
                Item {
                    revision: 1,
                    content: Content::Group {
                        parent: None,
                        name: "API".into(),
                        variables: [],
                    },
                },
            )]),
            ..Default::default()
        };
        cache.stage(BTreeMap::new(), None);
        cache.remote.insert(
            child,
            Item {
                revision: 2,
                content: request("https://example.test/new", Some(group)),
            },
        );
        assert_eq!(cache.conflicts(), vec![group]);
    }

    #[test]
    fn sharing_replaces_credentials_and_keeps_real_variable_references() -> Result<()> {
        let request = Request {
            url: "https://user:literal-secret@example.test/?api_key=literal-key&tag=x".into(),
            authorization: Authorization::Basic {
                username: "{{api_user}}".into(),
                password: "literal-password {{suffix}}".into(),
            },
            headers: vec![
                Pair {
                    name: "Authorization".into(),
                    value: "Bearer {{local_token}}".into(),
                    enabled: false,
                },
                Pair {
                    name: "Cookie".into(),
                    value: "session=literal-cookie".into(),
                    enabled: true,
                },
            ],
            ..Request::default()
        };
        let shared = share_request(request);
        assert_eq!(
            shared.url,
            "https://example.test/?api_key={{api_key}}&tag=x"
        );
        assert_eq!(
            shared.authorization,
            Authorization::Basic {
                username: "{{api_user}}".into(),
                password: "{{api_password}}".into(),
            }
        );
        assert_eq!(shared.headers[0].value, "Bearer {{local_token}}");
        assert!(!shared.headers[0].enabled);
        assert_eq!(shared.headers[1].value, "{{cookie}}");
        assert!(!serde_json::to_string(&shared)?.contains("literal-"));
        assert_eq!(share_request(shared.clone()), shared);
        Ok(())
    }

    #[test]
    fn local_drafts_pending_changes_and_sidebar_survive_reopening() -> Result<()> {
        let path = tempfile::tempdir()?;
        let file = path.path().join("account").join("workspace.json");
        let id = Uuid::new_v4();
        let group = Uuid::new_v4();
        let mut cache = Cache::default();
        cache.stage(
            BTreeMap::from([
                (id, request("https://example.test/shared", Some(group))),
                (
                    group,
                    Content::Group {
                        parent: None,
                        name: "API".into(),
                        variables: [],
                    },
                ),
            ]),
            None,
        );
        cache.drafts.insert(
            id,
            Draft {
                expected: None,
                parent: Some(group),
                request: Request {
                    url: "https://example.test/local".into(),
                    ..Request::default()
                },
            },
        );
        cache.sidebar = SidebarState {
            collapsed_collections: [group].into_iter().collect(),
            selected_request: Some(id),
            request_tabs: Some(vec![id]),
        };
        cache.save(&file)?;
        let reopened = Cache::open(&file)?;
        assert_eq!(reopened.pending, cache.pending);
        assert_eq!(reopened.drafts[&id].request, cache.drafts[&id].request);
        assert_eq!(reopened.sidebar, cache.sidebar);
        Ok(())
    }

    #[test]
    fn local_variable_connection_is_opt_in_and_saved_per_workspace() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let first = directory.path().join("first.json");
        let second = directory.path().join("second.json");
        let mut old_cache = serde_json::to_value(Cache::default())?;
        old_cache
            .as_object_mut()
            .unwrap()
            .remove("use_local_workspace_variables");
        fs::write(&first, serde_json::to_vec(&old_cache)?)?;

        let mut cache = Cache::open(&first)?;
        assert!(!cache.use_local_workspace_variables);
        cache.use_local_workspace_variables = true;
        cache.save(&first)?;
        assert!(Cache::open(&first)?.use_local_workspace_variables);
        assert!(!Cache::open(&second)?.use_local_workspace_variables);
        assert!(cache.pending.is_empty());
        Ok(())
    }

    #[test]
    fn older_cloud_variables_are_ignored_and_variable_only_changes_are_dropped() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let file = directory.path().join("workspace.json");
        let id = Uuid::new_v4();
        let group = serde_json::json!({
            "kind": "group", "parent": null, "name": "API",
            "variables": [{"name": "auth_token", "value": "legacy-token"}]
        });
        let pending_group = serde_json::json!({
            "kind": "group", "parent": null, "name": "API",
            "variables": [{"name": "auth_token", "value": "pending-token"}]
        });
        fs::write(
            &file,
            serde_json::to_vec(&serde_json::json!({
                "remote": {id.to_string(): {"revision": 1, "content": group}},
                "remote_revision": 1,
                "pending": {id.to_string(): {"expected": 1, "content": pending_group}},
                "drafts": {},
                "environment": [{"name": "saved_token", "value": "private-token"}]
            }))?,
        )?;

        let cache = Cache::open(&file)?;
        assert!(cache.pending.is_empty());
        assert_eq!(
            serde_json::to_value(&cache.working()[&id])?["variables"],
            serde_json::json!([])
        );
        cache.save(&file)?;
        assert_eq!(Cache::open(&file)?.environment[0].value, "private-token");
        Ok(())
    }
}
