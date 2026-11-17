use serde::{Deserialize, Deserializer, Serialize};

pub const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pair {
    pub name: String,
    pub value: String,
}

impl Pair {
    pub fn is_blank(&self) -> bool {
        self.name.is_empty() && self.value.is_empty()
    }
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub name: String,
    pub method: String,
    pub url: String,
    #[serde(default, deserialize_with = "deserialize_headers")]
    pub headers: Vec<Pair>,
    #[serde(default, skip_serializing_if = "Authorization::is_none")]
    pub authorization: Authorization,
    pub body: String,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            name: String::new(),
            method: "GET".into(),
            url: String::new(),
            headers: Vec::new(),
            authorization: Authorization::None,
            body: String::new(),
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
                }
            })
            .collect(),
    })
}

pub fn url_query(url: &str) -> Option<&str> {
    let address = url.split('#').next().unwrap_or_default();
    address.split_once('?').map(|(_, query)| query)
}

pub fn query_rows(url: &str) -> Vec<Pair> {
    form_urlencoded::parse(url_query(url).unwrap_or_default().as_bytes())
        .map(|(name, value)| Pair {
            name: name.into_owned(),
            value: value.into_owned(),
        })
        .collect()
}

pub fn replace_query(url: &str, rows: &[Pair]) -> String {
    let (address, fragment) = url.split_once('#').unwrap_or((url, ""));
    let base = address.split('?').next().unwrap_or_default();
    let query = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(
            rows.iter()
                .filter(|row| !row.is_blank())
                .map(|row| (&row.name, &row.value)),
        )
        .finish();
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

pub fn format_json_body(body: &str) -> anyhow::Result<String> {
    // Validate without converting numbers or collecting object members into a map.
    serde_json::from_str::<serde::de::IgnoredAny>(body)?;

    let mut formatted = String::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    let mut depth = 0;
    let newline = |output: &mut String, depth: usize| {
        output.push('\n');
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
    Ok(formatted)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    pub fn title(&self) -> &str {
        match self.name.trim() {
            "" if self.url.trim().is_empty() => "Untitled request",
            "" => self.url.trim(),
            name => name,
        }
    }
}
