use std::{
    collections::{BTreeSet, HashSet},
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::cloud::CloudServer;
use crate::request::Request;
use crate::variables::Variable;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    pub id: usize,
    pub name: String,
    #[serde(default)]
    pub variables: Vec<Variable>,
    /// None identifies a collection; a parent identifies a folder in its tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<usize>,
}

/// Root collection first, then each containing folder, ending with `id`.
pub fn lineage(collections: &[Collection], id: usize) -> Vec<&Collection> {
    let mut result = Vec::new();
    let mut next = Some(id);
    while let Some(id) = next {
        let Some(group) = collections.iter().find(|group| group.id == id) else {
            break;
        };
        if result.len() >= collections.len() {
            return Vec::new();
        }
        result.push(group);
        next = group.parent_id;
    }
    result.reverse();
    result
}

pub fn descendants(collections: &[Collection], id: usize) -> HashSet<usize> {
    collections
        .iter()
        .filter(|group| {
            lineage(collections, group.id)
                .iter()
                .any(|ancestor| ancestor.id == id)
        })
        .map(|group| group.id)
        .collect()
}

#[derive(Serialize, Deserialize)]
pub struct StoredRequest<T> {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection_id: Option<usize>,
    pub request: T,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidebarState<Id: Ord> {
    pub collapsed_collections: BTreeSet<Id>,
    pub selected_request: Option<Id>,
    // Older files omit this field. An empty list means every tab was closed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_tabs: Option<Vec<Id>>,
}

impl<Id: Ord> SidebarState<Id> {
    /// Return the ordered open request indexes and the selected request index.
    pub(crate) fn restore_request_tabs(
        &self,
        resolve: impl Fn(&Id) -> Option<usize>,
    ) -> (Vec<usize>, usize) {
        let selected = self.selected_request.as_ref().and_then(&resolve);
        let Some(stored_tabs) = &self.request_tabs else {
            let selected = selected.unwrap_or_default();
            return (vec![selected], selected);
        };
        let mut seen = HashSet::new();
        let tabs: Vec<_> = stored_tabs
            .iter()
            .filter_map(resolve)
            .filter(|&index| seen.insert(index))
            .collect();
        let selected = selected
            .filter(|index| seen.contains(index))
            .or_else(|| tabs.first().copied())
            .or(selected)
            .unwrap_or_default();
        (tabs, selected)
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct Snapshot<C, R> {
    pub collections: C,
    pub requests: Vec<StoredRequest<R>>,
    #[serde(default)]
    pub global_variables: Vec<Variable>,
    #[serde(default)]
    pub environment_variables: Vec<Variable>,
    #[serde(default)]
    pub sidebar: SidebarState<usize>,
}

type SavedWorkspace = Snapshot<Vec<Collection>, Request>;

#[derive(Deserialize)]
#[serde(untagged)]
enum RequestFile {
    Workspace(SavedWorkspace),
    Legacy(Vec<Request>),
}

pub struct Storage {
    directory: PathBuf,
    lock: Arc<File>,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct SavedCloudWorkspace {
    pub account: String,
    pub workspace: uuid::Uuid,
}

impl Storage {
    pub fn open() -> Result<(Self, SavedWorkspace)> {
        let project = ProjectDirs::from("com", "", "Prestman")
            .context("Could not find your application data directory")?;
        Self::open_in(project.data_dir().to_owned())
    }

    pub(crate) fn open_in(directory: PathBuf) -> Result<(Self, SavedWorkspace)> {
        fs::create_dir_all(&directory)
            .with_context(|| format!("Could not create {}", directory.display()))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("workspace.lock"))
            .context("Could not open the workspace lock")?;
        lock.try_lock().context(
            "Could not lock the workspace. Close any other Prestman window and reopen the app",
        )?;

        let path = directory.join("requests.json");
        let mut workspace = match File::open(&path) {
            Ok(file) => match serde_json::from_reader(BufReader::new(file))
                .with_context(|| format!("Could not read {}", path.display()))?
            {
                RequestFile::Workspace(workspace) => workspace,
                RequestFile::Legacy(requests) => SavedWorkspace {
                    collections: Vec::new(),
                    global_variables: Vec::new(),
                    environment_variables: Vec::new(),
                    sidebar: SidebarState::default(),
                    requests: requests
                        .into_iter()
                        .map(|request| StoredRequest {
                            collection_id: None,
                            request,
                        })
                        .collect(),
                },
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => SavedWorkspace::default(),
            Err(error) => {
                return Err(error).with_context(|| format!("Could not open {}", path.display()));
            }
        };
        let ids: HashSet<_> = workspace.collections.iter().map(|item| item.id).collect();
        ensure!(
            ids.len() == workspace.collections.len()
                && !ids.contains(&usize::MAX)
                && workspace.collections.iter().all(|group| group
                    .parent_id
                    .is_none_or(|parent| ids.contains(&parent))
                    && !lineage(&workspace.collections, group.id).is_empty())
                && workspace
                    .requests
                    .iter()
                    .all(|item| { item.collection_id.is_none_or(|id| ids.contains(&id)) }),
            "Invalid collection IDs in {}",
            path.display()
        );
        workspace
            .sidebar
            .collapsed_collections
            .retain(|id| ids.contains(id));
        workspace.sidebar.selected_request = workspace
            .sidebar
            .selected_request
            .filter(|&index| index < workspace.requests.len());
        Ok((
            Self {
                directory,
                lock: Arc::new(lock),
            },
            workspace,
        ))
    }

    pub(crate) fn cloud_cache_path(&self, account: &str, workspace: uuid::Uuid) -> PathBuf {
        self.directory
            .join("cloud")
            .join(account)
            .join(format!("{workspace}.json"))
    }

    pub(crate) fn history_path(&self) -> PathBuf {
        self.directory.join("history.json")
    }

    pub(crate) fn load_active_cloud_workspace(&self) -> Result<Option<SavedCloudWorkspace>> {
        let file = match File::open(self.directory.join("active-cloud-workspace.json")) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).context("Could not open the saved workspace selection");
            }
        };
        serde_json::from_reader(BufReader::new(file))
            .map(Some)
            .context("Could not read the saved workspace selection")
    }

    pub(crate) fn save_active_cloud_workspace(
        &self,
        workspace: &SavedCloudWorkspace,
    ) -> Result<()> {
        self.write_json("active-cloud-workspace.json", workspace)
            .context("Could not save the selected cloud workspace")
    }

    pub(crate) fn clear_active_cloud_workspace(&self) -> Result<()> {
        match fs::remove_file(self.directory.join("active-cloud-workspace.json")) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("Could not clear the selected cloud workspace"),
        }
    }

    pub(crate) fn load_cloud_server(&self) -> Result<Option<CloudServer>> {
        let path = self.directory.join("cloud-server.json");
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("Could not open the saved cloud server"),
        };
        let server: CloudServer = serde_json::from_reader(BufReader::new(file))
            .context("Could not read the saved cloud server")?;
        CloudServer::parse(&server.url, server.key.as_deref().unwrap_or_default()).map(Some)
    }

    pub(crate) fn save_cloud_server(&self, server: &CloudServer) -> Result<()> {
        self.write_json("cloud-server.json", server)
    }

    pub(crate) fn cloud_session_store(&self) -> CloudSessionStore {
        CloudSessionStore {
            path: self.directory.join("cloud-session.json"),
            _workspace_lock: self.lock.clone(),
        }
    }

    pub fn save<'a>(
        &self,
        collections: &[Collection],
        global_variables: &[Variable],
        environment_variables: &[Variable],
        requests: impl Iterator<Item = StoredRequest<&'a Request>>,
        sidebar: SidebarState<usize>,
    ) -> Result<()> {
        self.write_json(
            "requests.json",
            &Snapshot {
                collections,
                requests: requests.collect(),
                global_variables: global_variables.to_vec(),
                environment_variables: environment_variables.to_vec(),
                sidebar,
            },
        )
    }

    fn write_json(&self, name: &str, value: &impl Serialize) -> Result<()> {
        write_json(&self.directory.join(name), value)
    }
}

pub(crate) struct CloudSessionStore {
    path: PathBuf,
    _workspace_lock: Arc<File>,
}

#[derive(Deserialize, Serialize)]
struct SavedCloudSession {
    origin: String,
    refresh_token: String,
}

impl CloudSessionStore {
    pub(crate) fn load(&self, origin: &str) -> Result<Option<String>> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("Could not open the saved cloud session"),
        };
        let saved: SavedCloudSession = serde_json::from_reader(BufReader::new(file))
            .context("Could not read the saved cloud session")?;
        Ok((saved.origin == origin).then_some(saved.refresh_token))
    }

    pub(crate) fn save(&self, origin: &str, refresh_token: &str) -> Result<()> {
        write_json(
            &self.path,
            &SavedCloudSession {
                origin: origin.into(),
                refresh_token: refresh_token.into(),
            },
        )
        .context("Could not save the cloud session")
    }

    pub(crate) fn forget(&self) -> Result<()> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("Could not remove the saved cloud session"),
        }
    }
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let directory = path.parent().context("Missing data directory")?;
    let mut file = NamedTempFile::new_in(directory)
        .with_context(|| format!("Could not create a temporary file for {}", path.display()))?;
    {
        let mut writer = BufWriter::new(file.as_file_mut());
        serde_json::to_writer_pretty(&mut writer, value)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
    file.as_file().sync_all()?;
    file.persist(path)
        .with_context(|| format!("Could not replace {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_active_cloud_workspace_across_restarts_until_returning_to_local() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let (storage, _) = Storage::open_in(directory.path().into())?;
        assert!(storage.load_active_cloud_workspace()?.is_none());
        let account = "cloud-account";
        let workspace = uuid::Uuid::new_v4();
        storage.save_active_cloud_workspace(&SavedCloudWorkspace {
            account: account.into(),
            workspace: uuid::Uuid::new_v4(),
        })?;
        storage.save_active_cloud_workspace(&SavedCloudWorkspace {
            account: account.into(),
            workspace,
        })?;
        drop(storage);

        let (storage, _) = Storage::open_in(directory.path().into())?;
        let saved = storage.load_active_cloud_workspace()?.unwrap();
        assert_eq!(saved.account, account);
        assert_eq!(saved.workspace, workspace);
        storage.clear_active_cloud_workspace()?;
        storage.clear_active_cloud_workspace()?;
        drop(storage);

        let (storage, _) = Storage::open_in(directory.path().into())?;
        assert!(storage.load_active_cloud_workspace()?.is_none());
        Ok(())
    }

    #[test]
    fn saves_cloud_connection_separately_and_validates_it_on_reopen() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let (storage, _) = Storage::open_in(directory.path().into())?;
        assert!(storage.load_cloud_server()?.is_none());

        let server = CloudServer::parse(
            "http://127.0.0.1:8800/",
            "header.eyJyb2xlIjoiYW5vbiJ9.signature",
        )?;
        storage.save_cloud_server(&server)?;
        storage.save_cloud_server(&CloudServer::parse(
            "https://cloud.example.test/",
            "sb_publishable_example",
        )?)?;
        assert!(!directory.path().join("requests.json").exists());
        drop(storage);

        let (storage, _) = Storage::open_in(directory.path().into())?;
        let saved = storage.load_cloud_server()?.unwrap();
        assert_eq!(saved.url, "https://cloud.example.test");
        assert_eq!(saved.key.as_deref(), Some("sb_publishable_example"));
        assert!(CloudServer::parse("http://cloud.example.test", "").is_err());
        assert!(CloudServer::parse("https://cloud.example.test/auth/v1", "").is_err());

        storage.save_cloud_server(&CloudServer::parse("https://self-hosted.example.test", "")?)?;
        let saved = storage.load_cloud_server()?.unwrap();
        assert_eq!(saved.url, "https://self-hosted.example.test");
        assert!(saved.key.is_none());
        fs::write(
            directory.path().join("cloud-server.json"),
            r#"{"url":"https://cloud.example.test","key":"sb_secret_example"}"#,
        )?;
        assert!(storage.load_cloud_server().is_err());
        Ok(())
    }

    #[test]
    fn saves_nested_folders_and_rejects_invalid_parent_links_without_changing_the_file()
    -> Result<()> {
        let root = tempfile::tempdir()?;
        let (storage, _) = Storage::open_in(root.path().into())?;
        let groups = vec![
            Collection {
                id: 1,
                name: "API".into(),
                parent_id: None,
                variables: Vec::new(),
            },
            Collection {
                id: 2,
                name: "Users".into(),
                parent_id: Some(1),
                variables: Vec::new(),
            },
            Collection {
                id: 3,
                name: "Admin".into(),
                parent_id: Some(2),
                variables: Vec::new(),
            },
        ];
        let request = Request::default();
        storage.save(
            &groups,
            &[],
            &[],
            std::iter::once(StoredRequest {
                collection_id: Some(3),
                request: &request,
            }),
            SidebarState::default(),
        )?;
        drop(storage);
        let (storage, loaded) = Storage::open_in(root.path().into())?;
        assert_eq!(loaded.collections, groups);
        assert_eq!(loaded.requests[0].collection_id, Some(3));
        assert_eq!(
            lineage(&loaded.collections, 3)
                .iter()
                .map(|group| group.id)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(descendants(&loaded.collections, 2), HashSet::from([2, 3]));
        drop(storage);
        for broken in [
            r#"{"collections":[{"id":1,"name":"a","parent_id":1}],"requests":[]}"#,
            r#"{"collections":[{"id":1,"name":"a","parent_id":2},{"id":2,"name":"b","parent_id":1}],"requests":[]}"#,
            r#"{"collections":[{"id":1,"name":"a","parent_id":99}],"requests":[]}"#,
        ] {
            fs::write(root.path().join("requests.json"), broken)?;
            assert!(Storage::open_in(root.path().into()).is_err());
            assert_eq!(
                fs::read_to_string(root.path().join("requests.json"))?,
                broken
            );
        }
        Ok(())
    }

    #[test]
    fn reads_legacy_requests_and_persists_collections_and_membership() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("requests.json");
        let request = Request {
            url: "https://example.test".into(),
            ..Request::default()
        };
        let other = Request {
            name: "Другой запрос".into(),
            pre_request_script: "pm.variables.set('value', 'test');".into(),
            post_response_script: "pm.test('ok', () => pm.response.to.have.status(200));".into(),
            body: "{\"message\": \"Привет\"}".into(),
            authorization: crate::request::Authorization::Bearer {
                token: "test-token".into(),
            },
            ..Request::default()
        };
        fs::write(&path, serde_json::to_vec(&[&request, &other])?)?;
        let (storage, legacy) = Storage::open_in(directory.path().to_owned())?;
        assert!(legacy.collections.is_empty());
        assert_eq!(legacy.requests.len(), 2);
        assert!(
            legacy
                .requests
                .iter()
                .all(|item| item.collection_id.is_none())
        );
        assert!(legacy.requests[0].request == request);

        let collections = [
            Collection {
                parent_id: None,
                id: 1,
                name: "API".into(),
                variables: vec![Variable {
                    name: "host".into(),
                    value: "collection.test".into(),
                    enabled: true,
                }],
            },
            Collection {
                parent_id: None,
                id: 4,
                name: "Пустая коллекция".into(),
                variables: Vec::new(),
            },
        ];
        storage.save(
            &collections,
            &[Variable {
                name: "token".into(),
                value: "test-token".into(),
                enabled: false,
            }],
            &[Variable {
                name: "token".into(),
                value: "env-token".into(),
                enabled: true,
            }],
            [
                StoredRequest {
                    collection_id: Some(1),
                    request: &request,
                },
                StoredRequest {
                    collection_id: None,
                    request: &other,
                },
            ]
            .into_iter(),
            SidebarState::default(),
        )?;
        drop(storage);

        let (_, restored) = Storage::open_in(directory.path().to_owned())?;
        assert_eq!(restored.collections.len(), 2);
        assert_eq!(restored.collections[1].name, "Пустая коллекция");
        assert_eq!(restored.collections[0].variables, collections[0].variables);
        assert_eq!(restored.global_variables[0].name, "token");
        assert_eq!(restored.global_variables[0].value, "test-token");
        assert_eq!(restored.environment_variables[0].value, "env-token");
        assert!(!restored.global_variables[0].enabled);
        assert_eq!(restored.requests.len(), 2);
        assert_eq!(restored.requests[0].collection_id, Some(1));
        assert_eq!(restored.requests[1].collection_id, None);
        assert!(restored.requests[0].request == request);
        assert!(restored.requests[1].request == other);
        assert!(!restored.requests[1].request.pre_request_script.is_empty());
        Ok(())
    }

    #[test]
    fn an_unreadable_collection_is_left_intact() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("requests.json");
        fs::write(&path, "broken json")?;
        assert!(Storage::open_in(directory.path().to_owned()).is_err());
        assert_eq!(fs::read_to_string(path)?, "broken json");
        Ok(())
    }

    #[test]
    fn restores_tabs_in_order_and_distinguishes_empty_and_legacy_state() -> Result<()> {
        let mut sidebar: SidebarState<usize> =
            serde_json::from_str(r#"{"collapsed_collections":[],"selected_request":2}"#)?;
        let resolve = |&index: &usize| (index < 3).then_some(index);
        assert_eq!(sidebar.restore_request_tabs(resolve), (vec![2], 2));

        sidebar.request_tabs = Some(vec![1, 99, 2, 1, 0]);
        assert_eq!(sidebar.restore_request_tabs(resolve), (vec![1, 2, 0], 2));

        sidebar.selected_request = Some(0);
        sidebar.request_tabs = Some(vec![2, 1]);
        assert_eq!(sidebar.restore_request_tabs(resolve), (vec![2, 1], 2));

        sidebar.request_tabs = Some(Vec::new());
        let saved = serde_json::to_vec(&sidebar)?;
        let reopened: SidebarState<usize> = serde_json::from_slice(&saved)?;
        assert_eq!(reopened.restore_request_tabs(resolve), (Vec::new(), 0));
        Ok(())
    }

    #[test]
    fn workspaces_created_before_variables_still_load() -> Result<()> {
        let directory = tempfile::tempdir()?;
        fs::write(
            directory.path().join("requests.json"),
            r#"{"collections":[{"id":1,"name":"API"}],"requests":[]}"#,
        )?;
        let (_, restored) = Storage::open_in(directory.path().to_owned())?;
        assert!(restored.global_variables.is_empty());
        assert!(restored.environment_variables.is_empty());
        assert!(restored.collections[0].variables.is_empty());
        assert_eq!(restored.sidebar, SidebarState::default());
        Ok(())
    }
}
