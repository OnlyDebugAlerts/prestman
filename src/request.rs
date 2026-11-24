use serde::{Deserialize, Deserializer, Serialize};

pub const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pair {
    pub name: String,
    pub value: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

impl Default for Pair {
    fn default() -> Self {
        Self {
            name: String::new(),
            value: String::new(),
            enabled: true,
        }
    }
}

impl Pair {
    pub fn is_blank(&self) -> bool {
        self.name.is_empty() && self.value.is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Authorization {
    #[default]
    None,
    Bearer {
        token: String,
    },
    Basic {
        username: String,
        password: String,
    },
}

impl Authorization {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GrpcOptions {
    pub secure: Option<bool>,
    pub timeout_ms: u64,
    pub max_response_bytes: Option<usize>,
    pub include_default_fields: bool,
    pub server_name: String,
    pub verify_tls: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub on_message_script: String,
    pub proto: Option<GrpcProto>,
}

impl Default for GrpcOptions {
    fn default() -> Self {
        Self {
            secure: None,
            timeout_ms: 0,
            max_response_bytes: None,
            include_default_fields: false,
            server_name: String::new(),
            verify_tls: true,
            on_message_script: String::new(),
            proto: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrpcProto {
    pub path: std::path::PathBuf,
    pub descriptors: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphqlOptions {
    pub variables: String,
    pub operation_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub name: String,
    pub method: String,
    pub url: String,
    #[serde(default, deserialize_with = "deserialize_headers")]
    pub headers: Vec<Pair>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query: Vec<Pair>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path_variables: Vec<Pair>,
    #[serde(default, skip_serializing_if = "Authorization::is_none")]
    pub authorization: Authorization,
    pub body: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pre_request_script: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub post_response_script: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grpc: Option<GrpcOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graphql: Option<GraphqlOptions>,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            name: String::new(),
            method: "GET".into(),
            url: String::new(),
            headers: vec![Pair {
                name: "Content-Type".into(),
                value: "application/json".into(),
                ..Pair::default()
            }],
            query: Vec::new(),
            path_variables: Vec::new(),
            authorization: Authorization::None,
            body: String::new(),
            pre_request_script: String::new(),
            post_response_script: String::new(),
            grpc: None,
            graphql: None,
        }
    }
}

fn deserialize_headers<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Pair>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Headers {
        Rows(Vec<Pair>),
        Legacy(String),
    }

    Ok(match Headers::deserialize(deserializer)? {
        Headers::Rows(rows) => rows,
        Headers::Legacy(text) => text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let (name, value) = line.split_once(':').unwrap_or((line, ""));
                Pair {
                    name: name.trim().into(),
                    value: value.trim().into(),
                    ..Pair::default()
                }
            })
            .collect(),
    })
}

pub fn url_query(url: &str) -> Option<&str> {
    let address = url.split('#').next().unwrap_or_default();
    address.split_once('?').map(|(_, query)| query)
}

/// The path only: ports, credentials, query values and fragments are excluded.
pub fn path_range(url: &str) -> std::ops::Range<usize> {
    let end = url.find(['?', '#']).unwrap_or(url.len());
    let address = &url[..end];
    let authority = address.find("://").map_or(0, |at| at + 3);
    let start = address[authority..]
        .find('/')
        .map_or(end, |at| authority + at);
    start..end
}

pub fn path_parameter(segment: &str) -> Option<&str> {
    segment
        .strip_prefix(':')
        .filter(|name| !name.is_empty() && !name.contains([':', '{', '}']))
}

/// Keep per-request values while URL edits add new :parameters.
pub fn merge_path_variables(url: &str, previous: &[Pair]) -> Vec<Pair> {
    let mut rows = previous.to_vec();
    for name in url[path_range(url)].split('/').filter_map(path_parameter) {
        if !rows.iter().any(|row| row.name == name) {
            rows.push(Pair {
                name: name.into(),
                ..Pair::default()
            });
        }
    }
    rows
}

pub fn query_rows(url: &str) -> Vec<Pair> {
    form_urlencoded::parse(url_query(url).unwrap_or_default().as_bytes())
        .map(|(name, value)| Pair {
            name: name.into_owned(),
            value: value.into_owned(),
            ..Pair::default()
        })
        .collect()
}

pub fn merge_query_rows(url: &str, previous: &[Pair]) -> Vec<Pair> {
    let mut active = query_rows(url).into_iter();
    let mut rows: Vec<_> = previous
        .iter()
        .filter_map(|row| {
            if !row.enabled || row.is_blank() {
                Some(row.clone())
            } else {
                active.next()
            }
        })
        .collect();
    rows.extend(active);
    rows
}

pub fn replace_query(url: &str, rows: &[Pair]) -> String {
    let (address, fragment) = url.split_once('#').unwrap_or((url, ""));
    let base = address.split('?').next().unwrap_or_default();
    let query = rows
        .iter()
        .filter(|row| row.enabled && !row.is_blank())
        .map(|row| {
            format!(
                "{}={}",
                encode_query_template(&row.name),
                encode_query_template(&row.value)
            )
        })
        .collect::<Vec<_>>()
        .join("&");
    let mut url_with_query = base.to_owned();
    if !query.is_empty() {
        url_with_query.push('?');
        url_with_query.push_str(&query);
    }
    if url.contains('#') {
        url_with_query.push('#');
        url_with_query.push_str(fragment);
    }
    url_with_query
}

fn encode_query_template(mut text: &str) -> String {
    let encode = |text: &str| form_urlencoded::byte_serialize(text.as_bytes()).collect::<String>();
    let mut output = String::new();
    while let Some(start) = text.find("{{") {
        let Some(end) = text[start + 2..].find("}}") else {
            break;
        };
        let end = start + 2 + end + 2;
        output.push_str(&encode(&text[..start]));
        output.push_str(&text[start..end]);
        text = &text[end..];
    }
    output.push_str(&encode(text));
    output
}

/// Replace comments with whitespace so token boundaries and line positions survive.
pub fn strip_json_comments(body: &str) -> anyhow::Result<String> {
    let bytes = body.as_bytes();
    let mut clean = bytes.to_vec();
    let mut index = 0;
    let mut quoted = false;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' if quoted => {
                index += 2;
                continue;
            }
            b'"' => quoted = !quoted,
            b'/' if !quoted && matches!(bytes.get(index + 1), Some(b'/' | b'*')) => {
                let start = index;
                let block = bytes[index + 1] == b'*';
                index += 2;
                if block {
                    while index + 1 < bytes.len() && &bytes[index..index + 2] != b"*/" {
                        index += 1;
                    }
                    anyhow::ensure!(
                        index + 1 < bytes.len(),
                        "Unterminated block comment in JSON body"
                    );
                    index += 2;
                } else {
                    while index < bytes.len() && !matches!(bytes[index], b'\n' | b'\r') {
                        index += 1;
                    }
                }
                for byte in &mut clean[start..index] {
                    if !matches!(*byte, b'\n' | b'\r') {
                        *byte = b' ';
                    }
                }
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    Ok(String::from_utf8(clean).expect("Comments were replaced by ASCII whitespace"))
}

pub fn prepare_body(body: &str, headers: &[Pair]) -> anyhow::Result<String> {
    let content_type = headers
        .iter()
        .rev()
        .find(|row| row.enabled && row.name.trim().eq_ignore_ascii_case("content-type"));
    let is_json = content_type.map(|row| {
        let mime = row
            .value
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        mime.ends_with("/json") || mime.ends_with("+json") || mime.ends_with("/jsonc")
    });
    if is_json == Some(false) {
        return Ok(body.to_owned());
    }
    let trimmed = body.trim_start();
    if is_json == Some(true) || trimmed.starts_with(['{', '[']) {
        return strip_json_comments(body);
    }
    if trimmed.starts_with("//") || trimmed.starts_with("/*") {
        let clean = strip_json_comments(body)?;
        if clean.trim_start().starts_with(['{', '[']) {
            return Ok(clean);
        }
    }
    Ok(body.to_owned())
}

pub fn format_json_body(body: &str) -> anyhow::Result<String> {
    // Validate without converting numbers or collecting object members into a map.
    serde_json::from_str::<serde::de::IgnoredAny>(&strip_json_comments(body)?)?;

    let mut formatted = String::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    let mut depth = 0;
    let newline = |output: &mut String, depth: usize| {
        while output.ends_with([' ', '\t']) {
            output.pop();
        }
        if !output.ends_with('\n') {
            output.push('\n');
        }
        output.extend(std::iter::repeat_n(' ', depth * 2));
    };

    while let Some(ch) = chars.next() {
        match ch {
            ' ' | '\t' | '\n' | '\r' => {}
            '{' | '[' => {
                anyhow::ensure!(depth < 128, "JSON nesting exceeds 128 levels");
                formatted.push(ch);
                while matches!(chars.peek(), Some(' ' | '\t' | '\n' | '\r')) {
                    chars.next();
                }
                let close = if ch == '{' { '}' } else { ']' };
                if chars.next_if_eq(&close).is_some() {
                    formatted.push(close);
                } else {
                    depth += 1;
                    newline(&mut formatted, depth);
                }
            }
            '}' | ']' => {
                depth -= 1;
                newline(&mut formatted, depth);
                formatted.push(ch);
            }
            ',' => {
                formatted.push(ch);
                newline(&mut formatted, depth);
            }
            ':' => formatted.push_str(": "),
            '/' if matches!(chars.peek(), Some('/' | '*')) => {
                if formatted
                    .chars()
                    .last()
                    .is_some_and(|ch| !ch.is_whitespace())
                {
                    formatted.push(' ');
                }
                let kind = chars.next().unwrap();
                formatted.push('/');
                formatted.push(kind);
                if kind == '/' {
                    while chars.peek().is_some_and(|ch| !matches!(ch, '\r' | '\n')) {
                        formatted.push(chars.next().unwrap());
                    }
                    newline(&mut formatted, depth);
                } else {
                    let mut previous = '*';
                    for ch in chars.by_ref() {
                        formatted.push(ch);
                        if previous == '*' && ch == '/' {
                            break;
                        }
                        previous = ch;
                    }
                    formatted.push(' ');
                }
            }
            '"' => {
                formatted.push(ch);
                let mut escaped = false;
                for ch in chars.by_ref() {
                    formatted.push(ch);
                    if ch == '"' && !escaped {
                        break;
                    }
                    escaped = ch == '\\' && !escaped;
                }
            }
            _ => formatted.push(ch),
        }
    }
    Ok(formatted.trim_end().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_comments_preserve_strings_unicode_line_positions_and_number_tokens()
    -> anyhow::Result<()> {
        let source = r#"// Начало {{unused}}
{"url":"https://example.test/a//b", "text":"/*literal*/ and \"quoted\"", /* блок
комментария */ "n":1.000000000000000001,"ok":true} // конец"#;
        let clean = strip_json_comments(source)?;
        assert_eq!(clean.len(), source.len());
        assert_eq!(clean.lines().count(), source.lines().count());
        assert!(!clean.contains("unused"));
        assert!(clean.contains("https://example.test/a//b"));
        assert!(clean.contains("/*literal*/"));
        assert!(clean.contains("1.000000000000000001"));
        serde_json::from_str::<serde::de::IgnoredAny>(&clean)?;
        assert!(format_json_body("[1/* separate tokens */2]").is_err());
        assert!(strip_json_comments("{ /* not closed").is_err());
        assert_eq!(strip_json_comments("// final line")?.trim(), "");
        Ok(())
    }

    #[test]
    fn json_formatting_keeps_comments_and_is_idempotent() -> anyhow::Result<()> {
        for source in [
            "// heading\n{\"a\":1,// inline\n\"b\":[1,/*item*/2]} // end",
            "{/* empty object */}",
            "[/* first */{}, // second\n []]",
            "/* multiline\n comment */ {\"url\":\"https://example.test\",\"v\":true}",
        ] {
            let formatted = format_json_body(source)?;
            assert_eq!(format_json_body(&formatted)?, formatted);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&strip_json_comments(source)?)?,
                serde_json::from_str::<serde_json::Value>(&strip_json_comments(&formatted)?)?
            );
            assert!(formatted.contains("//") || formatted.contains("/*"));
        }
        Ok(())
    }

    #[test]
    fn comment_removal_respects_non_json_content_types() -> anyhow::Result<()> {
        let body = "https://example.test\n/* keep this exact text */";
        assert_eq!(
            prepare_body(
                body,
                &[Pair {
                    name: "Content-Type".into(),
                    value: "text/plain".into(),
                    ..Pair::default()
                }]
            )?,
            body
        );
        let source = "/* heading */{\"a\": 1 // note\n}";
        assert!(!prepare_body(source, &[])?.contains("heading"));
        assert!(
            !prepare_body(
                source,
                &[Pair {
                    name: "Content-Type".into(),
                    value: "application/problem+json; charset=utf-8".into(),
                    ..Pair::default()
                }]
            )?
            .contains("note")
        );
        Ok(())
    }

    #[test]
    fn new_requests_have_json_content_type_without_changing_loaded_headers()
    -> serde_json::Result<()> {
        let request = Request::default();
        assert_eq!(request.headers.len(), 1);
        assert_eq!(request.headers[0].name, "Content-Type");
        assert_eq!(request.headers[0].value, "application/json");
        assert!(request.headers[0].enabled);
        let old: Request = serde_json::from_str(
            r#"{"name":"old","method":"GET","url":"https://example.test","body":""}"#,
        )?;
        assert!(old.headers.is_empty());
        let custom: Request = serde_json::from_str(
            r#"{"name":"custom","method":"POST","url":"https://example.test","body":"","headers":[{"name":"Content-Type","value":"text/plain"}]}"#,
        )?;
        assert_eq!(custom.headers[0].value, "text/plain");
        Ok(())
    }

    #[test]
    fn query_edits_keep_order_repeated_keys_and_fragment() {
        let url = "https://example.test/path?tag=one&tag=two%2B&q=old+value&empty=#frag?a=b";
        let mut rows = query_rows(url);
        assert_eq!(rows[1].name, "tag");
        assert_eq!(rows[1].value, "two+");
        assert_eq!(rows[2].value, "old value");
        assert_eq!(rows[3].value, "");
        rows[2].value = "новое & значение".into();
        let updated = replace_query(url, &rows);
        assert!(updated.starts_with("https://example.test/path?tag=one&tag=two%2B&"));
        assert!(updated.ends_with("&empty=#frag?a=b"));
        assert_eq!(query_rows(&updated), rows);
        assert_eq!(
            replace_query(&updated, &[]),
            "https://example.test/path#frag?a=b"
        );
    }

    #[test]
    fn reads_legacy_headers_and_writes_key_value_rows() -> serde_json::Result<()> {
        let request: Request = serde_json::from_str(
            r#"{
            "name": "example", "method": "GET", "url": "https://example.test?a=1",
            "headers": "X-Test: one\r\nX-Test: two:three\n\nX-Empty:", "body": ""
        }"#,
        )?;
        assert_eq!(request.headers.len(), 3);
        assert!(request.headers.iter().all(|row| row.enabled));
        assert_eq!(request.headers[1].name, "X-Test");
        assert_eq!(request.headers[1].value, "two:three");
        assert_eq!(request.headers[2].value, "");
        let saved = serde_json::to_value(&request)?;
        assert!(saved["headers"].is_array());
        assert_eq!(
            serde_json::from_value::<Request>(saved)?.headers,
            request.headers
        );
        Ok(())
    }

    #[test]
    fn disabled_pairs_survive_saving_url_edits_and_reenabling() -> serde_json::Result<()> {
        let original = "https://example.test/path?tag=one&tag=two&empty=#frag";
        let mut request = Request {
            url: original.into(),
            query: query_rows(original),
            headers: serde_json::from_str(
                r#"[{"name":"X-Old","value":"yes"},{"name":"X-Off","value":"no","enabled":false}]"#,
            )?,
            ..Request::default()
        };
        assert!(request.headers[0].enabled);
        request.query[1].enabled = false;
        request.url = replace_query(&request.url, &request.query);
        assert_eq!(request.url, "https://example.test/path?tag=one&empty=#frag");

        let mut restored: Request = serde_json::from_str(&serde_json::to_string(&request)?)?;
        assert_eq!(restored, request);
        restored.url = "https://example.test/path?tag=updated&empty=&new=3#other".into();
        restored.query = merge_query_rows(&restored.url, &restored.query);
        assert_eq!(restored.query.len(), 4);
        assert!(!restored.query[1].enabled);
        assert_eq!(restored.query[1].value, "two");
        assert_eq!(replace_query(&restored.url, &restored.query), restored.url);
        restored.query[1].enabled = true;
        assert_eq!(
            replace_query(&restored.url, &restored.query),
            "https://example.test/path?tag=updated&tag=two&empty=&new=3#other"
        );
        assert_eq!(
            merge_query_rows("https://example.test", &request.query),
            vec![request.query[1].clone()]
        );
        Ok(())
    }

    #[test]
    fn path_discovery_ignores_ports_queries_and_keeps_existing_values() {
        let previous = vec![Pair {
            name: "code".into(),
            value: "existing".into(),
            enabled: false,
        }];
        let url = "https://user:pass@[::1]:8443/themes/:code/:id/:id?test=:query#/:fragment";
        let rows = merge_path_variables(url, &previous);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], previous[0]);
        assert_eq!(rows[1].name, "id");
        assert_eq!(merge_path_variables("{{base_url}}/:code", &rows), rows);
    }

    #[test]
    fn json_body_formatting_preserves_member_order_and_literal_spelling() -> anyhow::Result<()> {
        let source = r#" {"z":[ { }, [ ], 1.0000000000000000001, 1e500, "Привет, [\"мир\"] \\ ","\u0061"],"a":-0,"a":true} "#;
        let expected = r#"{
  "z": [
    {},
    [],
    1.0000000000000000001,
    1e500,
    "Привет, [\"мир\"] \\ ",
    "\u0061"
  ],
  "a": -0,
  "a": true
}"#;
        assert_eq!(format_json_body(source)?, expected);
        assert_eq!(format_json_body(expected)?, expected);
        assert_eq!(format_json_body("  null  ")?, "null");
        assert!(format_json_body("[1,]").is_err());
        assert!(format_json_body("{} {} ").is_err());
        let deep = format!("{}0{}", "[".repeat(129), "]".repeat(129));
        assert!(format_json_body(&deep).is_err());
        Ok(())
    }
}

impl Request {
    pub fn grpc() -> Self {
        Self {
            method: String::new(),
            headers: Vec::new(),
            body: "{}".into(),
            grpc: Some(GrpcOptions::default()),
            ..Self::default()
        }
    }

    pub fn graphql() -> Self {
        Self {
            method: "POST".into(),
            body: "query {\n  __typename\n}".into(),
            graphql: Some(GraphqlOptions {
                variables: "{}".into(),
                ..GraphqlOptions::default()
            }),
            ..Self::default()
        }
    }

    pub fn method_badge(&self) -> &str {
        if self.grpc.is_some() {
            "gRPC"
        } else if self.graphql.is_some() {
            "GQL"
        } else {
            &self.method
        }
    }

    pub fn title(&self) -> &str {
        match self.name.trim() {
            "" if self.url.trim().is_empty() => "Untitled request",
            "" => self.url.trim(),
            name => name,
        }
    }
}
