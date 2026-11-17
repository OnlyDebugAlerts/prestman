use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Write},
    path::PathBuf,
};

use anyhow::{Context, Result, ensure};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::request::Request;

#[derive(Serialize, Deserialize)]
pub struct Collection {
    pub id: usize,
    pub name: String,
}

#[derive(Serialize, Deserialize)]
pub struct StoredRequest<T> {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection_id: Option<usize>,
    pub request: T,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Snapshot<C, R> {
    pub collections: C,
    pub requests: Vec<StoredRequest<R>>,
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
    _lock: File,
}

impl Storage {
    pub fn open() -> Result<(Self, SavedWorkspace)> {
        let project = ProjectDirs::from("com", "", "Prestman")
            .context("Could not find your application data directory")?;
        Self::open_in(project.data_dir().to_owned())
    }

    fn open_in(directory: PathBuf) -> Result<(Self, SavedWorkspace)> {
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
        let workspace = match File::open(&path) {
            Ok(file) => match serde_json::from_reader(BufReader::new(file))
                .with_context(|| format!("Could not read {}", path.display()))?
            {
                RequestFile::Workspace(workspace) => workspace,
                RequestFile::Legacy(requests) => SavedWorkspace {
                    collections: Vec::new(),
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
                && workspace
                    .requests
                    .iter()
                    .all(|item| { item.collection_id.is_none_or(|id| ids.contains(&id)) }),
            "Invalid collection IDs in {}",
            path.display()
        );
        Ok((
            Self {
                directory,
                _lock: lock,
            },
            workspace,
        ))
    }

    pub fn save<'a>(
        &self,
        collections: &[Collection],
        requests: impl Iterator<Item = StoredRequest<&'a Request>>,
    ) -> Result<()> {
        let path = self.directory.join("requests.json");
        let mut file = NamedTempFile::new_in(&self.directory)
            .context("Could not create a temporary workspace file")?;
        {
            let mut writer = BufWriter::new(file.as_file_mut());
            serde_json::to_writer_pretty(
                &mut writer,
                &Snapshot {
                    collections,
                    requests: requests.collect(),
                },
            )?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
        file.as_file().sync_all()?;
        file.persist(&path)
            .with_context(|| format!("Could not replace {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                id: 1,
                name: "API".into(),
            },
            Collection {
                id: 4,
                name: "Пустая коллекция".into(),
            },
        ];
        storage.save(
            &collections,
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
        )?;
        drop(storage);

        let (_, restored) = Storage::open_in(directory.path().to_owned())?;
        assert_eq!(restored.collections.len(), 2);
        assert_eq!(restored.collections[1].name, "Пустая коллекция");
        assert_eq!(restored.requests.len(), 2);
        assert_eq!(restored.requests[0].collection_id, Some(1));
        assert_eq!(restored.requests[1].collection_id, None);
        assert!(restored.requests[0].request == request);
        assert!(restored.requests[1].request == other);
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
}
