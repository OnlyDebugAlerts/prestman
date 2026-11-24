use std::collections::HashMap;

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::request::{
    Authorization, Pair, Request, path_parameter, path_range, prepare_body, replace_query,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    pub value: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

impl Default for Variable {
    fn default() -> Self {
        Self {
            name: String::new(),
            value: String::new(),
            enabled: true,
        }
    }
}

/// Apply only the user's edits to the current values, preserving script updates
/// that arrived while the variable editor was open.
pub fn merge_edits(
    original: &[Variable],
    edited: &[Variable],
    current: &[Variable],
) -> Vec<Variable> {
    let mut merged = current.to_vec();
    for previous in original {
        if !edited.iter().any(|row| row.name == previous.name) {
            merged.retain(|row| row.name != previous.name);
        }
    }
    for row in edited {
        let previous = original.iter().find(|previous| previous.name == row.name);
        if previous == Some(row) {
            continue;
        }
        if let Some(current) = merged.iter_mut().find(|current| current.name == row.name) {
            if previous.is_none_or(|previous| previous.value != row.value) {
                current.value = row.value.clone();
            }
            if previous.is_none_or(|previous| previous.enabled != row.enabled) {
                current.enabled = row.enabled;
            }
        } else {
            merged.push(row.clone());
        }
    }
    merged
}

/// Resolves a copy for sending; the saved request keeps its templates.
pub fn resolve_text(text: &str, scopes: impl Iterator<Item = Variable>) -> Result<String> {
    let variables = scopes.collect::<Vec<_>>();
    Resolver {
        values: variables
            .iter()
            .filter(|row| row.enabled && !row.name.trim().is_empty())
            .map(|row| (row.name.trim(), row.value.as_str()))
            .collect(),
        resolved: HashMap::new(),
        stack: Vec::new(),
    }
    .text(text)
}

/// Resolves a copy for sending; the saved request keeps its templates.
pub fn resolve_request(
    request: &Request,
    globals: &[Variable],
    collection: &[Variable],
) -> Result<Request> {
    let mut resolver = Resolver {
        values: globals
            .iter()
            .chain(collection)
            .filter(|variable| variable.enabled && !variable.name.trim().is_empty())
            .map(|variable| (variable.name.trim(), variable.value.as_str()))
            .collect(),
        resolved: HashMap::new(),
        stack: Vec::new(),
    };
    let mut outgoing = request.clone();
    outgoing.headers.retain(|header| header.enabled);
    if outgoing.query.iter().any(|row| !row.enabled) {
        outgoing.url = replace_query(&outgoing.url, &outgoing.query);
    }
    outgoing.url = resolver.url(&outgoing.url)?;
    outgoing.url = resolver.path_variables(&outgoing.url, &outgoing.path_variables)?;
    for header in &mut outgoing.headers {
        header.name = resolver.text(&header.name)?;
        header.value = resolver.text(&header.value)?;
    }
    let body = if let Some(grpc) = &mut outgoing.grpc {
        grpc.server_name = resolver.text(&grpc.server_name)?;
        crate::request::strip_json_comments(&outgoing.body)?
    } else if let Some(graphql) = &mut outgoing.graphql {
        graphql.variables =
            resolver.text(&crate::request::strip_json_comments(&graphql.variables)?)?;
        graphql.operation_name = resolver.text(&graphql.operation_name)?;
        outgoing.body.clone()
    } else {
        prepare_body(&outgoing.body, &outgoing.headers)?
    };
    outgoing.body = resolver.text(&body)?;
    match &mut outgoing.authorization {
        Authorization::None => {}
        Authorization::Bearer { token } => *token = resolver.text(token)?,
        Authorization::Basic { username, password } => {
            *username = resolver.text(username)?;
            *password = resolver.text(password)?;
        }
    }
    Ok(outgoing)
}

struct Resolver<'a> {
    values: HashMap<&'a str, &'a str>,
    resolved: HashMap<String, String>,
    stack: Vec<String>,
}

impl Resolver<'_> {
    fn path_variables(&mut self, url: &str, variables: &[Pair]) -> Result<String> {
        if variables.is_empty() {
            return Ok(url.to_owned());
        }
        let range = path_range(url);
        let segments = url[range.clone()]
            .split('/')
            .map(|segment| {
                if let Some(name) = path_parameter(segment) {
                    return Ok(self
                        .path_value(name, variables, true)?
                        .unwrap_or_else(|| segment.to_owned()));
                }
                let mut output = String::new();
                let mut rest = segment;
                while let Some(start) = rest.find('{') {
                    let Some(offset) = rest[start + 1..].find('}') else {
                        break;
                    };
                    let end = start + 1 + offset;
                    let name = &rest[start + 1..end];
                    output.push_str(&rest[..start]);
                    if let Some(value) =
                        self.path_value(name, variables, segment == &rest[start..=end])?
                    {
                        output.push_str(&value);
                    } else {
                        output.push_str(&rest[start..=end]);
                    }
                    rest = &rest[end + 1..];
                }
                output.push_str(rest);
                Ok(output)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(format!(
            "{}{}{}",
            &url[..range.start],
            segments.join("/"),
            &url[range.end..]
        ))
    }

    fn path_value(
        &mut self,
        name: &str,
        variables: &[Pair],
        allow_slash: bool,
    ) -> Result<Option<String>> {
        let Some(row) = variables.iter().find(|row| row.enabled && row.name == name) else {
            return Ok(None);
        };
        let value = self.text(&row.value)?;
        let mut encoded = String::new();
        for byte in value.bytes() {
            if byte <= b' '
                || byte >= 0x7f
                || (!allow_slash && byte == b'/')
                || matches!(
                    byte,
                    b'?' | b'#' | b'"' | b'<' | b'>' | b'`' | b'{' | b'}' | b'\\'
                )
            {
                use std::fmt::Write as _;
                write!(encoded, "%{byte:02X}").expect("writing to a String");
            } else {
                encoded.push(byte as char);
            }
        }
        Ok(Some(encoded))
    }

    fn url(&mut self, url: &str) -> Result<String> {
        let (address, fragment) = url.split_once('#').unwrap_or((url, ""));
        let mut resolved = if let Some((base, query)) = address.split_once('?') {
            format!("{}?{}", self.text(base)?, self.expand(query, true)?)
        } else {
            self.text(address)?
        };
        if url.contains('#') {
            resolved.push('#');
            resolved.push_str(&self.text(fragment)?);
        }
        Ok(resolved)
    }

    fn text(&mut self, text: &str) -> Result<String> {
        self.expand(text, false)
    }

    fn expand(&mut self, mut text: &str, query: bool) -> Result<String> {
        let mut output = String::new();
        while let Some(start) = text.find("{{") {
            output.push_str(&text[..start]);
            let after_open = &text[start + 2..];
            let Some(end) = after_open.find("}}") else {
                output.push_str(&text[start..]);
                return Ok(output);
            };
            let name = after_open[..end].trim();
            let value = self.variable(name)?;
            if query {
                output.extend(form_urlencoded::byte_serialize(value.as_bytes()));
            } else {
                output.push_str(&value);
            }
            text = &after_open[end + 2..];
        }
        output.push_str(text);
        Ok(output)
    }

    fn variable(&mut self, name: &str) -> Result<String> {
        if let Some(value) = self.resolved.get(name) {
            return Ok(value.clone());
        }
        let Some(&value) = self.values.get(name) else {
            bail!("Undefined variable: {{{{{name}}}}}. Add it to Globals or this collection.");
        };
        ensure!(
            !self.stack.iter().any(|item| item == name),
            "Circular variable reference: {{{{{name}}}}}"
        );
        ensure!(self.stack.len() < 32, "Variable nesting exceeds 32 levels");
        self.stack.push(name.to_owned());
        let result = self.text(value);
        self.stack.pop();
        let resolved = result?;
        self.resolved.insert(name.to_owned(), resolved.clone());
        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::{Pair, query_rows, replace_query};

    fn variable(name: &str, value: &str) -> Variable {
        Variable {
            name: name.into(),
            value: value.into(),
            enabled: true,
        }
    }

    #[test]
    fn path_values_are_request_scoped_and_only_replace_path_segments() -> Result<()> {
        let request = Request {
            url: "{{base_url}}/themes/:code/:code/:code_suffix?code={{code}}&literal=:code#fragment:code".into(),
            path_variables: vec![Pair { name: "code".into(), value: "{{theme}}".into(), ..Pair::default() }],
            headers: vec![Pair { name: "X-Code".into(), value: "{{code}}".into(), ..Pair::default() }],
            body: "{\"code\":\"{{code}}\"}".into(),
            ..Request::default()
        };
        let globals = [
            variable("base_url", "https://[::1]:8443"),
            variable("code", "global"),
            variable("theme", "dark /test?x=1# Привет"),
        ];
        let resolved = resolve_request(&request, &globals, &[])?;
        let value = "dark%20/test%3Fx=1%23%20%D0%9F%D1%80%D0%B8%D0%B2%D0%B5%D1%82";
        assert_eq!(
            resolved.url,
            format!(
                "https://[::1]:8443/themes/{value}/{value}/:code_suffix?code=global&literal=:code#fragment:code"
            )
        );
        assert_eq!(resolved.headers[0].value, "global");
        assert_eq!(resolved.body, "{\"code\":\"global\"}");
        assert!(request.url.contains(":code"));
        Ok(())
    }

    #[test]
    fn blank_disabled_unused_and_braced_path_values_are_supported() -> Result<()> {
        let request = Request {
            url: "https://example.com/:empty/:off/{code}/{{code}}/:escaped".into(),
            path_variables: vec![
                Pair {
                    name: "empty".into(),
                    ..Pair::default()
                },
                Pair {
                    name: "off".into(),
                    value: "{{unknown}}".into(),
                    enabled: false,
                },
                Pair {
                    name: "code".into(),
                    value: "path".into(),
                    ..Pair::default()
                },
                Pair {
                    name: "unused".into(),
                    value: "{{unknown}}".into(),
                    ..Pair::default()
                },
                Pair {
                    name: "escaped".into(),
                    value: "a%2Fb ".into(),
                    ..Pair::default()
                },
            ],
            ..Request::default()
        };
        assert_eq!(
            resolve_request(&request, &[variable("code", "global")], &[])?.url,
            "https://example.com//:off/path/global/a%2Fb%20"
        );
        Ok(())
    }

    #[test]
    fn commented_out_body_variables_are_not_resolved_or_sent() -> Result<()> {
        let request = Request {url:"https://example.test".into(), body:"{\n// {{unknown}}\n\"value\":{{number}}, /* {{also_unknown}} */\n\"url\":\"https://example.test\"}".into(), ..Request::default()};
        let resolved = resolve_request(&request, &[variable("number", "42")], &[])?;
        let json: serde_json::Value = serde_json::from_str(&resolved.body)?;
        assert_eq!(json["value"], 42);
        assert_eq!(json["url"], "https://example.test");
        assert!(request.body.contains("// {{unknown}}"));
        Ok(())
    }

    #[test]
    fn editor_saves_preserve_new_script_variables_and_unedited_script_values() {
        let original = vec![variable("token", "old"), variable("remove", "old")];
        let edited = vec![
            Variable {
                enabled: false,
                ..variable("token", "old")
            },
            variable("manual", "new"),
        ];
        let current = vec![
            variable("token", "from-script"),
            variable("remove", "old"),
            variable("kid_id_42", "kid"),
        ];
        let merged = merge_edits(&original, &edited, &current);
        assert_eq!(
            merged,
            vec![
                Variable {
                    enabled: false,
                    ..variable("token", "from-script")
                },
                variable("kid_id_42", "kid"),
                variable("manual", "new")
            ]
        );
        assert_eq!(merge_edits(&original, &original, &current), current);
    }

    #[test]
    fn collection_overrides_globals_in_all_request_fields_without_changing_templates() -> Result<()>
    {
        let globals = [
            variable("host", "global.test"),
            variable("token", "global-token"),
            variable("key", "X-Token"),
        ];
        let collection = [
            variable("host", "collection.test"),
            variable("base", "https://{{host}}"),
            variable("value", "Привет"),
        ];
        let request = Request {
            url: "{{base}}/items?value={{value}}".into(),
            headers: vec![Pair {
                name: "{{key}}".into(),
                value: "{{token}}".into(),
                ..Pair::default()
            }],
            body: r#"{"value":"{{value}}"}"#.into(),
            authorization: Authorization::Bearer {
                token: "{{token}}".into(),
            },
            ..Request::default()
        };
        let resolved = resolve_request(&request, &globals, &collection)?;
        assert_eq!(
            reqwest::Url::parse(&resolved.url)?
                .query_pairs()
                .next()
                .unwrap()
                .1,
            "Привет"
        );
        assert_eq!(resolved.headers[0].name, "X-Token");
        assert_eq!(resolved.headers[0].value, "global-token");
        assert_eq!(resolved.body, r#"{"value":"Привет"}"#);
        assert!(
            matches!(resolved.authorization, Authorization::Bearer { token } if token == "global-token")
        );
        assert_eq!(request.url, "{{base}}/items?value={{value}}");
        let basic = resolve_request(
            &Request {
                authorization: Authorization::Basic {
                    username: "{{host}}".into(),
                    password: "{{token}}".into(),
                },
                ..request
            },
            &globals,
            &collection,
        )?;
        assert!(
            matches!(basic.authorization, Authorization::Basic { username, password } if username == "collection.test" && password == "global-token")
        );
        Ok(())
    }

    #[test]
    fn disabled_values_fall_back_to_globals_and_empty_values_are_valid() -> Result<()> {
        let globals = [variable("host", "global.test"), variable("empty", "")];
        let collection = [Variable {
            enabled: false,
            ..variable("host", "disabled.test")
        }];
        let request = Request {
            url: "https://{{host}}/{{empty}}".into(),
            ..Request::default()
        };
        assert_eq!(
            resolve_request(&request, &globals, &collection)?.url,
            "https://global.test/"
        );
        assert!(resolve_request(&request, &[], &collection).is_err());
        Ok(())
    }

    #[test]
    fn reports_unknown_and_circular_references_without_expanding_unused_variables() -> Result<()> {
        let request = Request {
            url: "{{a}}".into(),
            ..Request::default()
        };
        assert!(
            resolve_request(&request, &[], &[])
                .unwrap_err()
                .to_string()
                .contains("Undefined variable")
        );
        let cycle = [variable("a", "{{b}}"), variable("b", "{{a}}")];
        assert!(
            resolve_request(&request, &cycle, &[])
                .unwrap_err()
                .to_string()
                .contains("Circular")
        );
        let plain = Request {
            url: "https://example.test".into(),
            ..Request::default()
        };
        assert_eq!(resolve_request(&plain, &cycle, &[])?.url, plain.url);
        Ok(())
    }

    #[test]
    fn query_table_edits_preserve_templates_for_resolution() -> Result<()> {
        let url = "{{base}}/search?q={{search term}}&fixed=a%2Bb#fragment";
        let mut rows = query_rows(url);
        rows.push(Pair {
            name: "{{key}}".into(),
            value: "{{value}}".into(),
            ..Pair::default()
        });
        let edited = replace_query(url, &rows);
        assert!(edited.contains("q={{search term}}"));
        assert_eq!(query_rows(&edited), rows);
        let variables = [
            variable("base", "https://example.test"),
            variable("search term", "hello"),
            variable("key", "page"),
            variable("value", "2"),
        ];
        assert_eq!(
            resolve_request(
                &Request {
                    url: edited,
                    ..Request::default()
                },
                &variables,
                &[]
            )?
            .url,
            "https://example.test/search?q=hello&fixed=a%2Bb&page=2#fragment"
        );
        Ok(())
    }

    #[test]
    fn query_values_cannot_change_query_structure_and_literals_are_not_double_encoded() -> Result<()>
    {
        let request = Request {
            url: "{{base}}/search?q={{value}}&literal=a%2Bb&again={{value}}#{{fragment}}".into(),
            ..Request::default()
        };
        let variables = [
            variable("base", "https://example.test"),
            variable("value", "a+b & Привет#/?="),
            variable("fragment", "details"),
        ];
        let resolved = resolve_request(&request, &variables, &[])?;
        let url = reqwest::Url::parse(&resolved.url)?;
        let pairs = url.query_pairs().collect::<Vec<_>>();
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[0].1, variables[1].value);
        assert_eq!(pairs[1].1, "a+b");
        assert_eq!(pairs[2].1, variables[1].value);
        assert_eq!(url.fragment(), Some("details"));
        Ok(())
    }

    #[test]
    fn disabled_request_rows_do_not_resolve_missing_variables() -> Result<()> {
        let request = Request {
            url: "https://example.test?q={{missing}}&q={{value}}#result".into(),
            query: vec![
                Pair {
                    name: "q".into(),
                    value: "{{missing}}".into(),
                    enabled: false,
                },
                Pair {
                    name: "q".into(),
                    value: "{{value}}".into(),
                    ..Pair::default()
                },
            ],
            headers: vec![
                Pair {
                    name: "{{unknown}}".into(),
                    value: "{{missing}}".into(),
                    enabled: false,
                },
                Pair {
                    name: "X-Test".into(),
                    value: "{{value}}".into(),
                    ..Pair::default()
                },
            ],
            ..Request::default()
        };
        let resolved = resolve_request(&request, &[variable("value", "one & two")], &[])?;
        assert_eq!(resolved.url, "https://example.test?q=one+%26+two#result");
        assert_eq!(resolved.headers.len(), 1);
        assert_eq!(resolved.headers[0].value, "one & two");
        assert_eq!(request.headers.len(), 2);
        assert!(!request.query[0].enabled);
        Ok(())
    }
}
