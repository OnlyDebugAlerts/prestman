//! Postman collection export.
pub mod v3;

use std::{io::Write, path::Path};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::{
    request::{Authorization, Pair, Request, query_rows},
    storage::Collection,
};

const SCHEMA: &str = "https://schema.getpostman.com/json/collection/v2.1.0/collection.json";

fn export_url(raw: &str, query: &[Pair]) -> Value {
    // Postman's SDK builds the URL from these fields; `raw` alone in an object
    // is insufficient. Split without URL parsing so {{templates}} stay intact.
    let mut url = json!({"raw": raw, "query": query.iter().map(|row| json!({"key": row.name, "value": row.value, "disabled": !row.enabled})).collect::<Vec<_>>()});
    let (address, hash) = raw
        .split_once('#')
        .map_or((raw, None), |(address, hash)| (address, Some(hash)));
    if let Some(hash) = hash {
        url["hash"] = hash.into();
    }
    let base = address.split('?').next().unwrap_or_default();
    let authority_path = if let Some((protocol, rest)) = base.split_once("://") {
        url["protocol"] = protocol.into();
        rest
    } else {
        base
    };
    let mut authority = if let Some((authority, path)) = authority_path.split_once('/') {
        url["path"] = json!(path.split('/').collect::<Vec<_>>());
        authority
    } else {
        authority_path
    };
    if let Some((credentials, host)) = authority.rsplit_once('@') {
        let (user, password) = credentials.split_once(':').unwrap_or((credentials, ""));
        url["auth"] = json!({"user": user, "password": password});
        authority = host;
    }
    if let Some((host, port)) = authority.rsplit_once(':') {
        if !port.contains(']') && (!host.contains(':') || host.ends_with(']')) {
            url["port"] = port.into();
            authority = host;
        }
    }
    url["host"] = json!([authority]);
    url
}

#[cfg(test)]
pub fn encode<'a>(
    collection: &Collection,
    requests: impl Iterator<Item = &'a Request>,
) -> Result<Vec<u8>> {
    encode_tree(
        collection,
        &[],
        requests.map(|request| (collection.id, request)),
    )
}

pub fn encode_tree<'a>(
    collection: &Collection,
    groups: &[Collection],
    requests: impl Iterator<Item = (usize, &'a Request)>,
) -> Result<Vec<u8>> {
    let items = requests.map(|(parent, request)| {
        ensure!(request.grpc.is_none() && request.graphql.is_none(), "Use Postman v3 to export gRPC or GraphQL requests");
        let auth = match &request.authorization {
            Authorization::None => json!({"type": "noauth"}),
            Authorization::Bearer { token } => json!({"type": "bearer", "bearer": [{"key": "token", "value": token, "type": "string"}]}),
            Authorization::Basic { username, password } => json!({"type": "basic", "basic": [{"key": "username", "value": username, "type": "string"}, {"key": "password", "value": password, "type": "string"}]}),
        };
        let pairs = |rows: &[Pair]| rows.iter().map(|row| json!({"key": row.name, "value": row.value, "disabled": !row.enabled})).collect::<Vec<_>>();
        let query = if request.query.is_empty() { query_rows(&request.url) } else { request.query.clone() };
        let mut events = Vec::new();
        for (listen, source) in [("prerequest", &request.pre_request_script), ("test", &request.post_response_script)] {
            if !source.is_empty() {
                events.push(json!({"listen": listen, "script": {"type": "text/javascript", "exec": source.split('\n').collect::<Vec<_>>()}}));
            }
        }
        let mut url = export_url(&request.url, &query);
        if !request.path_variables.is_empty() { url["variable"] = json!(pairs(&request.path_variables)); }
        Ok((parent, json!({"name": request.name, "event": events, "request": {
            "method": request.method, "url": url,
            "header": pairs(&request.headers), "auth": auth,
            "body": {"mode": "raw", "raw": request.body}
        }})))
    }).collect::<Result<Vec<_>>>()?;
    fn children(
        parent: usize,
        groups: &[Collection],
        requests: &[(usize, Value)],
        depth: usize,
    ) -> Result<Vec<Value>> {
        ensure!(
            depth <= groups.len() + 1,
            "Folder hierarchy contains a cycle"
        );
        let mut items = Vec::new();
        for folder in groups
            .iter()
            .filter(|folder| folder.parent_id == Some(parent))
        {
            let variables = folder.variables.iter().map(|row| json!({"key": row.name, "value": row.value, "type": "string", "disabled": !row.enabled})).collect::<Vec<_>>();
            items.push(json!({"name": folder.name, "item": children(folder.id, groups, requests, depth + 1)?, "variable": variables}));
        }
        items.extend(
            requests
                .iter()
                .filter(|(id, _)| *id == parent)
                .map(|(_, item)| item.clone()),
        );
        Ok(items)
    }
    let mut valid = crate::storage::descendants(groups, collection.id);
    valid.insert(collection.id);
    ensure!(
        items.iter().all(|(parent, _)| valid.contains(parent)),
        "A request belongs to a folder outside this collection"
    );
    let items = children(collection.id, groups, &items, 0)?;
    let variables = collection.variables.iter().map(|row| json!({"key": row.name, "value": row.value, "type": "string", "disabled": !row.enabled})).collect::<Vec<_>>();
    serde_json::to_vec_pretty(&json!({"info": {"name": collection.name, "schema": SCHEMA}, "item": items, "variable": variables})).context("Could not encode collection")
}

pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    let directory = path
        .parent()
        .context("The export path has no parent directory")?;
    let mut file =
        tempfile::NamedTempFile::new_in(directory).context("Could not create the export file")?;
    file.write_all(bytes)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("Could not export to {}", path.display()))?;
    Ok(())
}

pub fn filename(name: &str) -> String {
    format!("{}.postman_collection.json", safe_stem(name, "Collection"))
}

fn safe_stem(name: &str, fallback: &str) -> String {
    let name = name
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                '_'
            } else {
                ch
            }
        })
        .take(100)
        .collect::<String>();
    let name = name.trim_matches([' ', '.']);
    if name.is_empty() {
        fallback.into()
    } else {
        name.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_v2_http_collection_with_structured_url_and_path_variables() -> Result<()> {
        let collection = Collection {
            id: 3,
            name: "API".into(),
            parent_id: None,
            variables: vec![],
        };
        let request = Request {
            name: "Get widget".into(),
            url: "https://example.test:443/widgets/:id?view=detail".into(),
            path_variables: vec![Pair {
                name: "id".into(),
                value: "a b".into(),
                enabled: true,
            }],
            ..Request::default()
        };
        let document: Value =
            serde_json::from_slice(&encode(&collection, [&request].into_iter())?)?;
        let url = &document["item"][0]["request"]["url"];
        assert_eq!(document["info"]["schema"], SCHEMA);
        assert_eq!(url["host"], json!(["example.test"]));
        assert_eq!(url["port"], "443");
        assert_eq!(url["path"], json!(["widgets", ":id"]));
        assert_eq!(url["variable"][0]["value"], "a b");
        assert_eq!(url["query"][0]["value"], "detail");
        Ok(())
    }
}
