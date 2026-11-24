use std::{fs, path::PathBuf};

use super::*;

fn pair_values(rows: &[Pair]) -> Value {
    json!(
        rows.iter()
            .filter(|row| !row.is_blank())
            .map(|row| { json!({"key": row.name, "value": row.value, "disabled": !row.enabled}) })
            .collect::<Vec<_>>()
    )
}

fn auth_data(auth: &Authorization) -> Value {
    match auth {
        Authorization::None => json!({"type": "noauth"}),
        Authorization::Bearer { token } => {
            json!({"type": "bearer", "credentials": [{"key": "token", "value": token}]})
        }
        Authorization::Basic { username, password } => json!({"type": "basic", "credentials": [
            {"key": "username", "value": username}, {"key": "password", "value": password}
        ]}),
    }
}

fn relative_path(root: &Path, target: &Path) -> Result<PathBuf> {
    let root = root.canonicalize()?;
    let target = target
        .canonicalize()
        .with_context(|| format!("Could not find the source schema {}", target.display()))?;
    let mut root = root.components().peekable();
    let mut target = target.components().peekable();
    ensure!(
        root.peek() == target.peek(),
        "The .proto file must be on the same drive as the export"
    );
    while root.peek().is_some() && root.peek() == target.peek() {
        root.next();
        target.next();
    }
    let mut relative = PathBuf::new();
    for _ in root {
        relative.push("..");
    }
    relative.extend(target.map(|part| part.as_os_str()));
    Ok(relative)
}

fn request_data(request: &Request, schema_root: &Path) -> Result<Value> {
    let is_grpc = request.grpc.is_some();
    let scripts = [
        (
            if is_grpc {
                "beforeInvoke"
            } else if request.graphql.is_some() {
                "beforeQuery"
            } else {
                "beforeRequest"
            },
            &request.pre_request_script,
        ),
        ("afterResponse", &request.post_response_script),
    ]
    .into_iter()
    .chain(
        request
            .grpc
            .as_ref()
            .map(|options| ("onIncomingMessage", &options.on_message_script)),
    )
    .filter(|(_, code)| !code.trim().is_empty())
    .map(|(phase, code)| json!({"type": phase, "code": code, "language": "text/javascript"}))
    .collect::<Vec<_>>();
    let auth = auth_data(&request.authorization);
    if let Some(options) = &request.grpc {
        let mut settings = json!({
            "strictSSL": options.verify_tls,
            "includeDefaultFields": options.include_default_fields,
            "connectionTimeout": options.timeout_ms,
        });
        if let Some(secure) = options.secure {
            settings["secureConnection"] = secure.into();
        }
        if let Some(limit) = options.max_response_bytes {
            settings["maxResponseMessageSize"] = limit.into();
        }
        if !options.server_name.is_empty() {
            settings["serverNameOverride"] = options.server_name.clone().into();
        }
        let schema = if let Some(proto) = &options.proto {
            json!({"source": "file", "location": relative_path(schema_root, &proto.path)?.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/")})
        } else {
            json!({"source": "auto"})
        };
        Ok(json!({
            "$kind": "grpc-request", "name": request.title(), "url": request.url,
            "methodPath": request.method, "message": {"content": request.body},
            "metadata": pair_values(&request.headers), "auth": auth,
            "schema": schema, "settings": settings, "scripts": scripts,
        }))
    } else if let Some(options) = &request.graphql {
        Ok(json!({
            "$kind": "graphql-request", "name": request.title(), "url": request.url,
            "query": request.body, "variables": options.variables, "operationName": options.operation_name,
            "headers": pair_values(&request.headers), "auth": auth,
            "schema": {"source": "auto"}, "scripts": scripts,
        }))
    } else {
        let body = if request.body.is_empty() {
            json!({"type": "none"})
        } else {
            json!({"type": "text", "content": request.body})
        };
        let query = if request.query.is_empty() {
            query_rows(&request.url)
        } else {
            request.query.clone()
        };
        Ok(json!({
            "$kind": "http-request", "name": request.title(), "url": request.url,
            "method": request.method, "body": body, "headers": pair_values(&request.headers),
            "queryParams": pair_values(&query), "pathVariables": pair_values(&request.path_variables),
            "auth": auth, "scripts": scripts,
        }))
    }
}

pub fn request_filename(request: &Request) -> String {
    format!("{}.request.yaml", safe_stem(request.title(), "Request"))
}

pub fn write_request(path: &Path, request: &Request) -> Result<()> {
    let root = path
        .parent()
        .context("The export path has no parent directory")?;
    let data = request_data(request, root)?;
    write(path, serde_yaml::to_string(&data)?.as_bytes())
}

pub fn write_directory(
    path: &Path,
    collection: &Collection,
    groups: &[Collection],
    requests: &[(usize, Request)],
) -> Result<()> {
    ensure!(
        !path.exists(),
        "Choose a new directory for the Postman collection"
    );
    let parent = path
        .parent()
        .context("The export path has no parent directory")?;
    let staging = tempfile::tempdir_in(parent)?;
    write_group(
        staging.path(),
        staging.path(),
        collection,
        groups,
        requests,
        true,
    )?;
    ensure!(
        !path.exists(),
        "The Postman collection directory already exists"
    );
    fs::rename(staging.path(), path)
        .with_context(|| format!("Could not export to {}", path.display()))?;
    Ok(())
}

fn write_group(
    directory: &Path,
    schema_root: &Path,
    group: &Collection,
    groups: &[Collection],
    requests: &[(usize, Request)],
    is_root: bool,
) -> Result<()> {
    let resources = directory.join(".resources");
    fs::create_dir(&resources)?;
    let mut data = json!({"$kind": "collection", "name": group.name});
    if is_root {
        data["variables"] = json!(
            group
                .variables
                .iter()
                .map(|row| {
                    json!({"key": row.name, "value": row.value, "disabled": !row.enabled})
                })
                .collect::<Vec<_>>()
        );
    } else {
        ensure!(
            group.variables.is_empty(),
            "Postman v3 does not support variables scoped to folder {}",
            group.name
        );
    }
    fs::write(
        resources.join("definition.yaml"),
        serde_yaml::to_string(&data)?,
    )?;
    for child in groups
        .iter()
        .filter(|child| child.parent_id == Some(group.id))
    {
        let name = safe_stem(&child.name, "Folder");
        let dest = unused_path(directory, &name, "");
        fs::create_dir(&dest)?;
        write_group(&dest, schema_root, child, groups, requests, false)?;
    }
    for (_, request) in requests.iter().filter(|(parent, _)| *parent == group.id) {
        let name = safe_stem(request.title(), "Request");
        let dest = unused_path(directory, &name, ".request.yaml");
        fs::write(
            dest,
            serde_yaml::to_string(&request_data(request, schema_root)?)?,
        )?;
    }
    Ok(())
}

fn unused_path(directory: &Path, name: &str, suffix: &str) -> PathBuf {
    (1..)
        .map(|i| {
            let stem = if i == 1 {
                name.to_owned()
            } else {
                format!("{name} ({i})")
            };
            directory.join(format!("{stem}{suffix}"))
        })
        .find(|path| !path.exists())
        .expect("available filename")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_grpc_as_postman_v3_yaml() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("Echo.request.yaml");
        let request = Request {
            name: "Echo".into(),
            method: "test.Echo/Say".into(),
            url: "grpcs://example.test:443".into(),
            ..Request::grpc()
        };
        write_request(&path, &request)?;
        let exported: Value = serde_yaml::from_slice(&fs::read(path)?)?;
        assert_eq!(exported["$kind"], "grpc-request");
        assert_eq!(exported["methodPath"], "test.Echo/Say");
        assert_eq!(exported["schema"]["source"], "auto");
        Ok(())
    }
    #[test]
    fn exports_graphql_with_variables_operation_and_scripts() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut request = Request::graphql();
        request.body = "query Viewer($id: ID!) { viewer(id: $id) { name } }".into();
        request.pre_request_script = "console.log(pm.info.eventName);".into();
        let options = request.graphql.as_mut().unwrap();
        options.variables = r#"{"id":"42"}"#.into();
        options.operation_name = "Viewer".into();
        let data = request_data(&request, directory.path())?;
        assert_eq!(data["$kind"], "graphql-request");
        assert_eq!(data["query"], request.body);
        assert_eq!(data["variables"], r#"{"id":"42"}"#);
        assert_eq!(data["operationName"], "Viewer");
        assert_eq!(data["scripts"][0]["type"], "beforeQuery");
        Ok(())
    }
}
