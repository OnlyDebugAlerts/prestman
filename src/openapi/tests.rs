use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

use super::*;
use crate::variables::resolve_request;

#[test]
fn openapi_yaml_imports_local_references_servers_and_runnable_requests() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("openapi.yaml");
    fs::write(
        directory.path().join("schemas.yaml"),
        r#"
Payload:
  allOf:
    - type: object
      properties:
        id: { type: integer, readOnly: true }
        name: { type: string, example: Ada }
    - type: object
      properties:
        active: { type: boolean, default: true }
"#,
    )?;
    fs::write(
        &path,
        r#"
openapi: 3.1.0
info: { title: Widget API, version: '1' }
servers:
  - url: https://{region}.example.test/v1
    variables:
      region: { default: eu }
security:
  - key: []
components:
  securitySchemes:
    key: { type: apiKey, in: header, name: X-API-Key }
  parameters:
    Widget:
      name: id
      in: path
      required: true
      schema: { type: string, example: 'a b' }
  requestBodies:
    Payload:
      content:
        application/json:
          schema: { $ref: 'schemas.yaml#/Payload' }
paths:
  /widgets/{id}:
    parameters:
      - $ref: '#/components/parameters/Widget'
      - { name: limit, in: query, schema: { type: integer, default: 1 } }
    patch:
      summary: Update widget
      parameters:
        - name: limit
          in: query
          required: true
          schema: { type: integer, example: 2 }
        - name: tag
          in: query
          schema: { type: array, items: { type: string }, example: [a, b] }
        - { name: X-Tenant, in: header, required: true, example: main, schema: { type: string } }
      requestBody: { $ref: '#/components/requestBodies/Payload' }
      responses:
        200: { description: OK }
  /widgets/{id}.{format}:
    servers: [{ url: https://files.example.test/v2 }]
    get:
      security: []
      operationId: download
      parameters:
        - $ref: '#/components/parameters/Widget'
        - { name: format, in: path, required: true, schema: { type: string, enum: [json] } }
      responses:
        '200': { description: OK }
"#,
    )?;
    let mut imported = read_file(&path)?;
    assert_eq!(imported.name, "Widget API");
    assert_eq!(imported.folders[0].name, "widgets");
    assert_eq!(imported.requests.len(), 2);
    let patch = &imported
        .requests
        .iter()
        .find(|(_, req)| req.method == "PATCH")
        .unwrap()
        .1;
    assert_eq!(patch.name, "Update widget");
    assert_eq!(patch.query.iter().filter(|row| !row.enabled).count(), 2);
    assert_eq!(
        serde_json::from_str::<Value>(&patch.body)?,
        json!({"name":"Ada", "active":true})
    );
    imported
        .variables
        .iter_mut()
        .find(|var| var.name == "auth_key")
        .unwrap()
        .value = "dummy".into();
    let resolved = resolve_request(patch, &[], &imported.variables)?;
    assert_eq!(
        resolved.url,
        "https://eu.example.test/v1/widgets/a%20b?limit=2"
    );
    assert!(
        resolved
            .headers
            .iter()
            .any(|row| row.name == "X-API-Key" && row.value == "dummy")
    );
    assert!(
        resolved
            .headers
            .iter()
            .any(|row| row.name == "X-Tenant" && row.value == "main")
    );
    let download = &imported
        .requests
        .iter()
        .find(|(_, req)| req.name == "download")
        .unwrap()
        .1;
    assert_eq!(
        resolve_request(download, &[], &imported.variables)?.url,
        "https://files.example.test/v2/widgets/a%20b.json"
    );
    assert!(download.headers.is_empty());
    Ok(())
}

#[test]
fn swagger_json_uses_legacy_base_url_and_form_data() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("swagger.json");
    let spec = json!({
        "swagger": "2.0", "info": {"title": "Forms", "version": "1"},
        "host": "example.test:8080", "basePath": "/api", "schemes": ["https"],
        "consumes": ["application/x-www-form-urlencoded"],
        "securityDefinitions": {"basicAuth": {"type":"basic"}},
        "security": [{"basicAuth":[]}],
        "paths": {"/users": {"post": {
            "parameters": [
                {"name":"name", "in":"formData", "type":"string", "required":true, "default":"Jane Doe"},
                {"name":"tag", "in":"query", "type":"array", "items":{"type":"integer"}, "required":true,
                    "collectionFormat":"multi", "default":[1,2]}
            ],
            "responses":{"201":{"description":"Created"}}
        }}}
    });
    fs::write(&path, serde_json::to_vec(&spec)?)?;
    let imported = read_file(&path)?;
    let request = resolve_request(&imported.requests[0].1, &[], &imported.variables)?;
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.url,
        "https://example.test:8080/api/users?tag=1&tag=2"
    );
    assert_eq!(request.body, "name=Jane+Doe");
    assert_eq!(
        request.headers[0].value,
        "application/x-www-form-urlencoded"
    );
    assert!(matches!(request.authorization, Authorization::Basic { .. }));
    fs::write(&path, r#"{"info":{"name":"Postman Collection"},"item":[]}"#)?;
    assert!(
        read_file(&path)
            .unwrap_err()
            .to_string()
            .contains("Expected a Swagger")
    );
    Ok(())
}

#[test]
fn url_import_follows_redirects_and_resolves_server_and_refs_against_the_document() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let server = thread::spawn(move || -> Result<()> {
        for (route, status, body) in [
            ("/spec", "302 Found\r\nLocation: /docs/swagger.yaml", ""),
            (
                "/docs/swagger.yaml",
                "200 OK",
                r#"
openapi: 3.2.0
info: { title: Online API, version: '1' }
servers: [{ url: ../gateway }]
paths:
  /status:
    get:
      parameters:
        - $ref: 'params.yaml#/check'
      responses:
        '200': { description: OK }
"#,
            ),
            (
                "/docs/params.yaml",
                "302 Found\r\nLocation: /shared/params.yaml",
                "",
            ),
            (
                "/shared/params.yaml",
                "200 OK",
                "check: { $ref: 'definitions.yaml#/check' }",
            ),
            (
                "/shared/definitions.yaml",
                "200 OK",
                "check: { name: check, in: query, required: true, schema: { type: boolean, default: true } }",
            ),
        ] {
            let (mut stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            reader.read_line(&mut line)?;
            assert_eq!(line, format!("GET {route} HTTP/1.1\r\n"));
            loop {
                line.clear();
                if reader.read_line(&mut line)? == 0 || line == "\r\n" {
                    break;
                }
            }
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )?;
        }
        Ok(())
    });
    let imported = read_url(&format!("http://{address}/spec"))?;
    server.join().expect("specification server panicked")?;
    let request = resolve_request(&imported.requests[0].1, &[], &imported.variables)?;
    assert_eq!(
        request.url,
        format!("http://{address}/gateway/status?check=true")
    );
    assert!(read_url("file:///tmp/swagger.json").is_err());
    Ok(())
}

#[test]
fn paths_keep_every_segment_in_nested_folders_even_for_single_requests() -> Result<()> {
    let file = tempfile::NamedTempFile::new()?;
    fs::write(
        file.path(),
        r#"
openapi: 3.1.0
info: { title: Nested API, version: '1' }
paths:
  /health:
    get: { summary: Health }
  /api/v1/accounts:
    get: { summary: List accounts }
  /api/v1/accounts/{accountId}:
    get: { summary: Read account }
    patch: { summary: Update account }
  /api/v1/accounts/{accountId}/permissions:
    get: { summary: Permissions }
  /api/v1/orders:
    get: { summary: List orders }
"#,
    )?;
    let imported = read_file(file.path())?;
    let parent = |name: &str| {
        imported
            .requests
            .iter()
            .find(|(_, request)| request.name == name)
            .unwrap()
            .0
    };
    let folder = |name: &str| {
        imported
            .folders
            .iter()
            .find(|folder| folder.name == name)
            .unwrap()
    };
    let api = folder("api");
    let version = folder("v1");
    let accounts = folder("accounts");
    let account = folder("{accountId}");
    let permissions = folder("permissions");
    let orders = folder("orders");
    let health = folder("health");
    assert_eq!(api.parent_id, None);
    assert_eq!(version.parent_id, Some(api.id));
    assert_eq!(accounts.parent_id, Some(version.id));
    assert_eq!(account.parent_id, Some(accounts.id));
    assert_eq!(permissions.parent_id, Some(account.id));
    assert_eq!(orders.parent_id, Some(version.id));
    assert_eq!(health.parent_id, None);
    assert_eq!(parent("Health"), health.id);
    assert_eq!(parent("List orders"), orders.id);
    assert_eq!(parent("List accounts"), accounts.id);
    assert_eq!(parent("Permissions"), permissions.id);
    assert_eq!(parent("Update account"), account.id);
    assert_eq!(imported.requests.len(), 6);
    Ok(())
}
