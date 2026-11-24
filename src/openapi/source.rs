use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use percent_encoding::percent_decode_str;
use reqwest::{Url, blocking::Client, redirect::Policy};
use serde_json::Value;

const MAX_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DOCUMENTS: usize = 16;

pub(super) struct Source {
    pub url: Url,
    pub root: Arc<Value>,
    documents: RefCell<HashMap<Url, (Url, Arc<Value>)>>,
    local_directory: Option<PathBuf>,
    client: Option<Client>,
}

impl Source {
    pub fn from_file(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .context("Could not locate the specification file")?;
        let local_directory = path
            .parent()
            .context("The specification file has no parent directory")?
            .to_owned();
        let url = Url::from_file_path(&path)
            .map_err(|_| anyhow::anyhow!("Invalid specification file path"))?;
        let root = Arc::new(parse(
            File::open(&path).context("Could not open the specification file")?,
        )?);
        Ok(Self {
            url,
            root,
            documents: RefCell::default(),
            local_directory: Some(local_directory),
            client: None,
        })
    }

    pub fn from_url(address: &str) -> Result<Self> {
        let mut url = Url::parse(address.trim()).context("Enter an absolute HTTP or HTTPS URL")?;
        validate_http_url(&url)?;
        url.set_fragment(None);
        let response = fetch(&client(None)?, &url)?;
        let mut url = response.url().clone();
        url.set_fragment(None);
        let root = Arc::new(parse(response)?);
        let client = client(Some(url.clone()))?;
        Ok(Self {
            url,
            root,
            documents: RefCell::default(),
            local_directory: None,
            client: Some(client),
        })
    }

    fn document(&self, url: &Url) -> Result<(Url, Arc<Value>)> {
        if *url == self.url {
            return Ok((url.clone(), self.root.clone()));
        }
        if let Some(document) = self.documents.borrow().get(url) {
            return Ok(document.clone());
        }
        ensure!(
            self.documents.borrow().len() < MAX_DOCUMENTS - 1,
            "The specification references too many documents"
        );
        let (location, document) = if let Some(directory) = &self.local_directory {
            let path = url
                .to_file_path()
                .map_err(|_| anyhow::anyhow!("Local specifications must reference local files"))?;
            let path = path
                .canonicalize()
                .context("Could not find the referenced file")?;
            ensure!(
                path.starts_with(directory),
                "Referenced files must be inside the specification directory"
            );
            let location = Url::from_file_path(&path)
                .map_err(|_| anyhow::anyhow!("Invalid referenced file path"))?;
            (location, parse(File::open(path)?)?)
        } else {
            validate_http_url(url)?;
            ensure!(
                url.origin() == self.url.origin(),
                "Referenced documents must use the specification's origin"
            );
            let response = fetch(
                self.client.as_ref().expect("remote specification client"),
                url,
            )?;
            let mut location = response.url().clone();
            location.set_fragment(None);
            (location, parse(response)?)
        };
        let entry = (location.clone(), Arc::new(document));
        let mut documents = self.documents.borrow_mut();
        documents.insert(url.clone(), entry.clone());
        if location != *url {
            documents.insert(location, entry.clone());
        }
        Ok(entry)
    }

    fn reference(&self, origin: &Url, reference: &str) -> Result<(Url, Value)> {
        let mut url = origin
            .join(reference)
            .context("Invalid specification reference")?;
        let fragment = percent_decode_str(url.fragment().unwrap_or_default()).decode_utf8()?;
        let fragment = fragment.into_owned();
        url.set_fragment(None);
        let (location, document) = self.document(&url)?;
        let value = document
            .pointer(&fragment)
            .with_context(|| format!("Could not resolve reference {reference}"))?;
        Ok((location, value.clone()))
    }

    pub fn resolve(&self, origin: &Url, value: &Value) -> Result<(Url, Value)> {
        let mut url = origin.clone();
        let mut value = value.clone();
        let mut visited = HashSet::new();
        while let Some(reference) = value["$ref"].as_str() {
            ensure!(
                visited.len() < 32 && visited.insert((url.clone(), reference.to_owned())),
                "Circular specification reference: {reference}"
            );
            (url, value) = self.reference(&url, reference)?;
        }
        Ok((url, value))
    }
}

fn client(root: Option<Url>) -> Result<Client> {
    Ok(Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(20))
        .redirect(Policy::custom(move |attempt| {
            if let Some(root) = &root {
                if attempt.url().origin() != root.origin() {
                    return attempt
                        .error("Referenced documents must stay on the specification's origin");
                }
            }
            if attempt.previous().len() >= 8 || validate_http_url(attempt.url()).is_err() {
                attempt.error("Invalid specification redirect")
            } else {
                attempt.follow()
            }
        }))
        .build()?)
}

fn validate_http_url(url: &Url) -> Result<()> {
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.has_host()
            && url.username().is_empty()
            && url.password().is_none(),
        "Use an HTTP or HTTPS URL without embedded credentials"
    );
    Ok(())
}

fn fetch(client: &Client, url: &Url) -> Result<reqwest::blocking::Response> {
    client
        .get(url.clone())
        .header(
            "Accept",
            "application/json, application/yaml, text/yaml, */*;q=0.1",
        )
        .send()
        .map_err(reqwest::Error::without_url)
        .context("Could not download the specification")?
        .error_for_status()
        .map_err(reqwest::Error::without_url)
        .context("The specification URL returned an error")
}

fn parse(input: impl Read) -> Result<Value> {
    let mut bytes = Vec::new();
    input.take(MAX_DOCUMENT_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_DOCUMENT_BYTES,
        "The specification document exceeds 16 MiB"
    );
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    if let Ok(value) = serde_json::from_slice(bytes) {
        return Ok(value);
    }
    if bytes
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        == Some(b'<')
    {
        bail!(
            "This link points to an HTML page. Enter the direct swagger.json or OpenAPI YAML URL."
        );
    }
    let mut value: serde_yaml::Value =
        serde_yaml::from_slice(bytes).context("Invalid specification JSON or YAML")?;
    value.apply_merge().context("Invalid YAML merge")?;
    serde_json::to_value(value).context("The specification contains invalid object keys or values")
}
