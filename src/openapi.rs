mod source;

use std::{collections::HashMap, path::Path};

use anyhow::{Context, Result, ensure};
use reqwest::Url;
use serde_json::{Map, Value, json};

use crate::{
    request::{Authorization, Pair, Request, replace_query},
    storage::Collection,
    variables::Variable,
};

use source::Source;

const HTTP_METHODS: &[&str] = &[
    "get", "put", "post", "delete", "options", "head", "patch", "trace", "query",
];

#[derive(Debug)]
pub struct ImportedCollection {
    pub name: String,
    pub variables: Vec<Variable>,
    pub folders: Vec<Collection>,
    pub requests: Vec<(usize, Request)>,
}

pub fn read_file(path: &Path) -> Result<ImportedCollection> {
    convert(&Source::from_file(path)?)
}

pub fn read_url(url: &str) -> Result<ImportedCollection> {
    convert(&Source::from_url(url)?)
}

fn convert(source: &Source) -> Result<ImportedCollection> {
    let root = &source.root;
    let swagger = root["swagger"] == "2.0" || root["swagger"].as_f64() == Some(2.0);
    let openapi = root["openapi"].as_str().is_some_and(|version| {
        matches!(version.split('.').collect::<Vec<_>>().as_slice(),
            ["3", "0" | "1" | "2", patch] if patch.parse::<u64>().is_ok())
    });
    ensure!(
        swagger || openapi,
        "Expected a Swagger 2.0 or OpenAPI 3.0–3.2 specification"
    );
    let paths = root["paths"]
        .as_object()
        .context("The specification has no paths object")?;
    let mut converter = Converter {
        source,
        swagger,
        bases: HashMap::new(),
        folders: HashMap::new(),
        result: ImportedCollection {
            name: root["info"]["title"]
                .as_str()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or("Imported API")
                .trim()
                .into(),
            variables: Vec::new(),
            folders: Vec::new(),
            requests: Vec::new(),
        },
    };
    for (path, value) in paths.iter().filter(|(key, _)| key.starts_with('/')) {
        let (origin, item) = source
            .resolve(&source.url, value)
            .with_context(|| format!("Path {path}"))?;
        for (method, operation) in HTTP_METHODS
            .iter()
            .filter_map(|method| item.get(*method).map(|operation| (*method, operation)))
            .chain(
                object(&item["additionalOperations"])
                    .map(|(method, operation)| (method.as_str(), operation)),
            )
        {
            let method = method.to_ascii_uppercase();
            reqwest::Method::from_bytes(method.as_bytes()).context("Invalid HTTP method")?;
            let request = converter
                .request(path, &origin, &item, &method, operation)
                .with_context(|| format!("{method} {path}"))?;
            let parent = converter.folder_for(path);
            converter.result.requests.push((parent, request));
        }
    }
    ensure!(
        !converter.result.requests.is_empty(),
        "The specification contains no HTTP operations"
    );
    Ok(converter.result)
}

struct Converter<'a> {
    source: &'a Source,
    swagger: bool,
    bases: HashMap<String, String>,
    folders: HashMap<(usize, String), usize>,
    result: ImportedCollection,
}

impl Converter<'_> {
    fn folder_for(&mut self, path: &str) -> usize {
        let segments = path.split('/').filter(|segment| !segment.is_empty());
        let root = path.trim_matches('/').is_empty().then_some("/");
        let mut parent = 0;
        for segment in root.into_iter().chain(segments) {
            parent = *self
                .folders
                .entry((parent, segment.into()))
                .or_insert_with(|| {
                    let id = self.result.folders.len() + 1;
                    self.result.folders.push(Collection {
                        id,
                        name: segment.into(),
                        parent_id: (parent != 0).then_some(parent),
                        variables: Vec::new(),
                    });
                    id
                });
        }
        parent
    }

    fn variable(&mut self, name: &str, value: String) -> String {
        if !self
            .result
            .variables
            .iter()
            .any(|variable| variable.name == name)
        {
            self.result.variables.push(Variable {
                name: name.into(),
                value,
                enabled: true,
            });
        }
        format!("{{{{{name}}}}}")
    }

    fn base(&mut self, url: String) -> String {
        if let Some(name) = self.bases.get(&url) {
            return format!("{{{{{name}}}}}");
        }
        let name = if self.bases.is_empty() {
            "baseUrl".into()
        } else {
            format!("baseUrl{}", self.bases.len() + 1)
        };
        self.bases.insert(url.clone(), name.clone());
        self.variable(&name, url)
    }

    fn request(
        &mut self,
        path: &str,
        origin: &Url,
        item: &Value,
        method: &str,
        operation: &Value,
    ) -> Result<Request> {
        ensure!(operation.is_object(), "Invalid operation");
        let root = &self.source.root;
        let base = if self.swagger {
            swagger_base(&self.source.url, root, operation)?
        } else {
            let (server_origin, server) = [
                (&operation["servers"], origin),
                (&item["servers"], origin),
                (&root["servers"], &self.source.url),
            ]
            .into_iter()
            .find_map(|(servers, origin)| list(servers).next().map(|server| (origin, server)))
            .unwrap_or((&self.source.url, &Value::Null));
            openapi_base(server_origin, server)?
        };
        let base = self.base(base.trim_end_matches('/').into());
        let mut request = Request {
            name: ["summary", "operationId"]
                .into_iter()
                .find_map(|key| {
                    operation[key]
                        .as_str()
                        .filter(|name| !name.trim().is_empty())
                })
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{method} {path}")),
            method: method.into(),
            url: format!("{base}{}", request_path(path)),
            headers: Vec::new(),
            ..Request::default()
        };
        let mut parameters: Vec<(Url, Value)> = Vec::new();
        for value in list(&item["parameters"]).chain(list(&operation["parameters"])) {
            let (origin, value) = self.source.resolve(origin, value)?;
            ensure!(
                value["name"].is_string() && value["in"].is_string(),
                "A parameter is missing its name or location"
            );
            if let Some(index) = parameters
                .iter()
                .position(|(_, old)| old["name"] == value["name"] && old["in"] == value["in"])
            {
                parameters[index] = (origin, value);
            } else {
                parameters.push((origin, value));
            }
        }

        let mut cookies = Vec::new();
        let mut form = Map::new();
        let mut swagger_body = None;
        for (origin, parameter) in parameters {
            let name = parameter["name"].as_str().expect("validated parameter");
            let location = parameter["in"]
                .as_str()
                .expect("validated parameter location");
            let value = parameter_sample(self.source, &origin, &parameter)?;
            let enabled = parameter["required"] == true || location == "path";
            match location {
                "path" => request
                    .path_variables
                    .push(pair(name, plain_value(&value, ","), true)),
                "query" => request.query.extend(query_pairs(
                    name,
                    value,
                    &parameter,
                    enabled,
                    self.swagger,
                )),
                "header"
                    if !matches!(
                        name.to_ascii_lowercase().as_str(),
                        "accept" | "content-type" | "authorization"
                    ) =>
                {
                    request
                        .headers
                        .push(pair(name, plain_value(&value, ","), enabled))
                }
                "cookie" => cookies.push(pair(name, plain_value(&value, ","), enabled)),
                "body" if self.swagger => swagger_body = Some(value),
                "formData" if self.swagger && parameter["type"] != "file" => {
                    form.insert(name.into(), value);
                }
                _ => {}
            }
        }
        self.apply_auth(operation, &mut request, &mut cookies)?;
        for enabled in [true, false] {
            let value = cookies
                .iter()
                .filter(|cookie| cookie.enabled == enabled)
                .map(|cookie| format!("{}={}", cookie.name, cookie.value))
                .collect::<Vec<_>>()
                .join("; ");
            if !value.is_empty() {
                request.headers.push(pair("Cookie", value, enabled));
            }
        }
        if self.swagger {
            let media = if operation["consumes"].is_array() {
                &operation["consumes"]
            } else {
                &root["consumes"]
            };
            let default = if !form.is_empty() {
                "application/x-www-form-urlencoded"
            } else {
                "application/json"
            };
            let media = list(media)
                .filter_map(Value::as_str)
                .min_by_key(|media| media_priority(media))
                .unwrap_or(default);
            if let Some(body) = swagger_body {
                set_body(&mut request, media, body)?;
            } else if !form.is_empty() {
                set_body(&mut request, media, Value::Object(form))?;
            }
        } else if !operation["requestBody"].is_null() {
            let (origin, body) = self.source.resolve(origin, &operation["requestBody"])?;
            if let Some((media, entry)) = preferred_media(&body["content"]) {
                let (origin, entry) = self.source.resolve(&origin, entry)?;
                let value = sample_with_examples(self.source, &origin, &entry, &entry["schema"])?;
                set_body(&mut request, media, value)?;
            }
        }
        request.url = replace_query(&request.url, &request.query);
        Ok(request)
    }

    fn apply_auth(
        &mut self,
        operation: &Value,
        request: &mut Request,
        cookies: &mut Vec<Pair>,
    ) -> Result<()> {
        let root = &self.source.root;
        let security = operation.get("security").unwrap_or(&root["security"]);
        let schemes = if self.swagger {
            &root["securityDefinitions"]
        } else {
            &root["components"]["securitySchemes"]
        };
        // Requirements are alternatives. Choose the first one we can represent.
        for requirement in list(security).filter_map(Value::as_object) {
            let mut resolved = Vec::new();
            for name in requirement.keys() {
                let (_, scheme) = self.source.resolve(&self.source.url, &schemes[name])?;
                resolved.push((name, scheme));
            }
            if !resolved.iter().all(|(_, scheme)| {
                matches!(
                    (
                        scheme["type"].as_str(),
                        scheme["scheme"].as_str(),
                        scheme["in"].as_str()
                    ),
                    (Some("basic" | "oauth2" | "openIdConnect"), _, _)
                        | (Some("http"), Some("basic" | "bearer"), _)
                        | (Some("apiKey"), _, Some("header" | "query" | "cookie"))
                )
            }) {
                continue;
            }
            for (name, scheme) in resolved {
                let variable = format!("auth_{name}");
                match scheme["type"].as_str().unwrap_or_default() {
                    "apiKey" => {
                        let Some(key) = scheme["name"].as_str() else {
                            continue;
                        };
                        let row = pair(key, self.variable(&variable, String::new()), true);
                        let rows = match scheme["in"].as_str() {
                            Some("header") => &mut request.headers,
                            Some("query") => &mut request.query,
                            _ => &mut *cookies,
                        };
                        rows.retain(|previous| !previous.name.eq_ignore_ascii_case(key));
                        rows.push(row);
                    }
                    "basic" | "http"
                        if scheme["type"] == "basic" || scheme["scheme"] == "basic" =>
                    {
                        request.authorization = Authorization::Basic {
                            username: self.variable(&format!("{variable}_username"), String::new()),
                            password: self.variable(&format!("{variable}_password"), String::new()),
                        };
                    }
                    _ => {
                        request.authorization = Authorization::Bearer {
                            token: self.variable(&variable, String::new()),
                        }
                    }
                }
            }
            break;
        }
        Ok(())
    }
}

fn list(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

fn object(value: &Value) -> impl Iterator<Item = (&String, &Value)> {
    value.as_object().into_iter().flatten()
}

fn pair(name: impl Into<String>, value: String, enabled: bool) -> Pair {
    Pair {
        name: name.into(),
        value,
        enabled,
    }
}

fn request_path(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if let Some(name) = segment
                .strip_prefix('{')
                .and_then(|part| part.strip_suffix('}'))
                .filter(|name| !name.is_empty() && !name.contains(['{', '}', ':']))
            {
                format!(":{name}")
            } else {
                segment.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn swagger_base(origin: &Url, root: &Value, operation: &Value) -> Result<String> {
    let scheme = list(&operation["schemes"])
        .chain(list(&root["schemes"]))
        .filter_map(Value::as_str)
        .find(|scheme| matches!(*scheme, "http" | "https"))
        .unwrap_or(if origin.scheme() == "https" {
            "https"
        } else {
            "http"
        });
    let host = root["host"].as_str().map(str::to_owned).unwrap_or_else(|| {
        let mut host = origin.host_str().unwrap_or("localhost").to_owned();
        if let Some(port) = origin.port() {
            host.push_str(&format!(":{port}"));
        }
        host
    });
    let path = root["basePath"].as_str().unwrap_or("/");
    base_url(origin, &format!("{scheme}://{host}{path}"))
}

fn openapi_base(origin: &Url, server: &Value) -> Result<String> {
    let address = server["url"].as_str().unwrap_or("/");
    let mut expanded = address.to_owned();
    for (name, variable) in object(&server["variables"]) {
        let value = variable["default"]
            .as_str()
            .with_context(|| format!("Server variable {name} has no default"))?;
        expanded = expanded.replace(&format!("{{{name}}}"), value);
    }
    base_url(origin, &expanded)
}

fn base_url(origin: &Url, address: &str) -> Result<String> {
    let fallback = Url::parse("http://localhost/").expect("valid fallback URL");
    let origin = if origin.scheme() == "file" {
        &fallback
    } else {
        origin
    };
    let url = origin.join(address).context("Invalid API server URL")?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.has_host()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.username().is_empty()
            && url.password().is_none(),
        "API server must be an HTTP or HTTPS URL without credentials, query, or fragment"
    );
    Ok(url.into())
}

fn parameter_sample(source: &Source, origin: &Url, parameter: &Value) -> Result<Value> {
    if let Some((_, entry)) = preferred_media(&parameter["content"]) {
        let (origin, entry) = source.resolve(origin, entry)?;
        return sample_with_examples(source, &origin, &entry, &entry["schema"]);
    }
    let schema = parameter.get("schema").unwrap_or(parameter);
    sample_with_examples(source, origin, parameter, schema)
}

fn example_value(source: &Source, origin: &Url, field: &Value) -> Result<Option<Value>> {
    if let Some(value) = field.get("example").or_else(|| field.get("x-example")) {
        return Ok(Some(value.clone()));
    }
    for (_, example) in object(&field["examples"]) {
        let (_, example) = source.resolve(origin, example)?;
        if let Some(value) = example.get("value").or_else(|| example.get("dataValue")) {
            return Ok(Some(value.clone()));
        }
    }
    Ok(None)
}

fn sample_with_examples(
    source: &Source,
    origin: &Url,
    field: &Value,
    schema: &Value,
) -> Result<Value> {
    match example_value(source, origin, field)? {
        Some(example) => Ok(example),
        None => schema_sample(source, origin, schema, 0, &mut 2048),
    }
}

fn schema_sample(
    source: &Source,
    origin: &Url,
    schema: &Value,
    depth: usize,
    remaining: &mut usize,
) -> Result<Value> {
    if depth >= 10 || *remaining == 0 {
        return Ok(Value::Null);
    }
    *remaining -= 1;
    let (origin, schema) = source.resolve(origin, schema)?;
    if let Some(value) = schema
        .get("example")
        .or_else(|| list(&schema["examples"]).next())
        .or_else(|| schema.get("default"))
        .or_else(|| schema.get("const"))
        .or_else(|| list(&schema["enum"]).next())
    {
        return Ok(value.clone());
    }
    if let Some(variant) = list(&schema["oneOf"]).chain(list(&schema["anyOf"])).next() {
        return schema_sample(source, &origin, variant, depth + 1, remaining);
    }
    let kind = schema["type"].as_str().or_else(|| {
        list(&schema["type"])
            .filter_map(Value::as_str)
            .find(|kind| *kind != "null")
    });
    if kind == Some("object") || schema["properties"].is_object() || schema["allOf"].is_array() {
        let mut fields = Map::new();
        for part in list(&schema["allOf"]) {
            if let Value::Object(part) = schema_sample(source, &origin, part, depth + 1, remaining)?
            {
                fields.extend(part);
            }
        }
        for (key, property) in object(&schema["properties"]) {
            if *remaining == 0 {
                break;
            }
            let (property_origin, resolved) = source.resolve(&origin, property)?;
            if property["readOnly"] != true && resolved["readOnly"] != true {
                fields.insert(
                    key.clone(),
                    schema_sample(source, &property_origin, &resolved, depth + 1, remaining)?,
                );
            }
        }
        return Ok(Value::Object(fields));
    }
    Ok(match kind {
        Some("array") => {
            let count = schema["minItems"].as_u64().unwrap_or(1).min(8);
            let count = count.min(schema["maxItems"].as_u64().unwrap_or(count)) as usize;
            let mut items = Vec::new();
            for _ in 0..count {
                if *remaining == 0 {
                    break;
                }
                items.push(schema_sample(
                    source,
                    &origin,
                    &schema["items"],
                    depth + 1,
                    remaining,
                )?);
            }
            Value::Array(items)
        }
        Some("integer" | "number") => schema
            .get("minimum")
            .or_else(|| schema.get("maximum"))
            .cloned()
            .unwrap_or(json!(0)),
        Some("boolean") => json!(false),
        Some("null") => Value::Null,
        _ => json!(match schema["format"].as_str() {
            Some("date") => "2000-01-01",
            Some("date-time") => "2000-01-01T00:00:00Z",
            Some("uuid") => "00000000-0000-4000-8000-000000000000",
            Some("email") => "user@example.com",
            Some("uri" | "url") => "https://example.com/",
            Some("binary" | "byte" | "password") => "",
            _ => "string",
        }),
    })
}

fn plain_value(value: &Value, separator: &str) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => String::new(),
        Value::Array(values) => values
            .iter()
            .map(|value| plain_value(value, separator))
            .collect::<Vec<_>>()
            .join(separator),
        Value::Object(values) => values
            .iter()
            .flat_map(|(key, value)| [key.clone(), plain_value(value, separator)])
            .collect::<Vec<_>>()
            .join(separator),
        value => value.to_string(),
    }
}

fn query_pairs(
    name: &str,
    value: Value,
    parameter: &Value,
    enabled: bool,
    swagger: bool,
) -> Vec<Pair> {
    if parameter["content"].is_object() {
        let value = if value.is_string() {
            plain_value(&value, ",")
        } else {
            value.to_string()
        };
        return vec![pair(name, value, enabled)];
    }
    let style = parameter["style"].as_str().unwrap_or("form");
    let format = parameter["collectionFormat"].as_str().unwrap_or("csv");
    let explode = if swagger {
        format == "multi"
    } else {
        parameter["explode"].as_bool().unwrap_or(style == "form")
    };
    let separator = match (style, format) {
        ("spaceDelimited", _) | (_, "ssv") => " ",
        ("pipeDelimited", _) | (_, "pipes") => "|",
        (_, "tsv") => "\t",
        _ => ",",
    };
    match value {
        Value::Array(values) if explode => values
            .iter()
            .map(|value| pair(name, plain_value(value, ","), enabled))
            .collect(),
        Value::Object(values) if style == "deepObject" || explode => values
            .iter()
            .map(|(key, value)| {
                let key = if style == "deepObject" {
                    format!("{name}[{key}]")
                } else {
                    key.clone()
                };
                pair(key, plain_value(value, ","), enabled)
            })
            .collect(),
        value => vec![pair(name, plain_value(&value, separator), enabled)],
    }
}

fn media_priority(media: &str) -> u8 {
    let mime = media.split(';').next().unwrap_or_default().trim();
    if mime == "application/json" || mime.ends_with("+json") {
        0
    } else if mime == "application/x-www-form-urlencoded" {
        1
    } else if mime == "multipart/form-data" {
        2
    } else if mime.starts_with("text/") {
        3
    } else {
        4
    }
}

fn preferred_media(content: &Value) -> Option<(&str, &Value)> {
    object(content)
        .min_by_key(|(media, _)| media_priority(media))
        .map(|(media, entry)| (media.as_str(), entry))
}

fn set_body(request: &mut Request, media: &str, body: Value) -> Result<()> {
    let mut content_type = media.to_owned();
    let mime = media.split(';').next().unwrap_or_default().trim();
    request.body = match mime {
        "application/x-www-form-urlencoded" if body.is_object() => {
            form_urlencoded::Serializer::new(String::new())
                .extend_pairs(object(&body).map(|(key, value)| (key, plain_value(value, ","))))
                .finish()
        }
        "multipart/form-data" if body.is_object() => {
            let mut boundary = "PrestmanOpenApiBoundary".to_owned();
            while body.to_string().contains(&boundary) {
                boundary.push('x');
            }
            let mut form = String::new();
            for (key, value) in object(&body) {
                ensure!(
                    !key.contains(['\r', '\n', '"', '\\']),
                    "Invalid multipart field name"
                );
                form.push_str(&format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{key}\"\r\n\r\n{}\r\n",
                    plain_value(value, ",")
                ));
            }
            form.push_str(&format!("--{boundary}--\r\n"));
            content_type = format!("multipart/form-data; boundary={boundary}");
            form
        }
        "application/octet-stream" => String::new(),
        mime if mime == "application/json" || mime.ends_with("+json") => {
            serde_json::to_string_pretty(&body)?
        }
        _ => body
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| body.to_string()),
    };
    request
        .headers
        .push(pair("Content-Type", content_type, true));
    Ok(())
}

#[cfg(test)]
mod tests;
