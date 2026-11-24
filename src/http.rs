use std::{
    io::Read,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use reqwest::{
    Method, Url,
    blocking::{Client, RequestBuilder},
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue},
};

use crate::request::{Authorization, Pair, Request, prepare_body};

pub struct Response {
    pub status: u16,
    pub reason: &'static str,
    pub elapsed: Duration,
    pub body_bytes: usize,
    pub is_json: bool,
    pub body: Arc<str>,
    pub raw_body: Arc<str>,
    pub headers: String,
    pub trailers: String,
    pub grpc_server_streaming: Option<bool>,
}

pub fn client() -> Result<Client> {
    Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("Prestman/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("Could not initialize the HTTP client")
}

pub fn send(client: &Client, request: Request) -> Result<Response> {
    send_with_timeout(client, request, Duration::from_secs(30))
}

pub fn send_with_timeout(client: &Client, request: Request, timeout: Duration) -> Result<Response> {
    let outgoing = build_request(client, request)?.timeout(timeout);
    let started = Instant::now();
    let mut incoming = outgoing.send().context("Could not send the request")?;
    let status = incoming.status();
    let declares_json = incoming
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let mime = value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            mime.ends_with("/json") || mime.ends_with("+json")
        });
    let headers = incoming
        .headers()
        .iter()
        .map(|(name, value)| format!("{name}: {}", String::from_utf8_lossy(value.as_bytes())))
        .collect::<Vec<_>>()
        .join("\n");

    let mut bytes = Vec::new();
    incoming
        .read_to_end(&mut bytes)
        .context("Could not read the response body")?;
    let elapsed = started.elapsed();
    let body_bytes = bytes.len();
    let starts_with_json = matches!(
        bytes.iter().find(|byte| !byte.is_ascii_whitespace()),
        Some(b'{' | b'[')
    );
    let pretty_json = if declares_json || starts_with_json {
        serde_json::from_slice::<serde_json::Value>(&bytes)
            .and_then(|json| serde_json::to_string_pretty(&json))
            .ok()
    } else {
        None
    };
    let is_json = declares_json || pretty_json.is_some();
    let raw_body: Arc<str> = String::from_utf8(bytes)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
        .into();
    let body = pretty_json
        .map(Arc::from)
        .unwrap_or_else(|| raw_body.clone());
    Ok(Response {
        status: status.as_u16(),
        reason: status.canonical_reason().unwrap_or_default(),
        elapsed,
        body_bytes,
        is_json,
        body,
        raw_body,
        headers,
        trailers: String::new(),
        grpc_server_streaming: None,
    })
}

fn build_request(client: &Client, request: Request) -> Result<RequestBuilder> {
    let url = Url::parse(request.url.trim()).context("Enter an absolute HTTP or HTTPS URL")?;
    if !matches!(url.scheme(), "http" | "https") || !url.has_host() {
        bail!("Enter an absolute HTTP or HTTPS URL");
    }
    let method = if request.graphql.is_some() {
        Method::POST
    } else {
        Method::from_bytes(request.method.as_bytes()).context("Invalid HTTP method")?
    };
    let mut headers = parse_headers(&request.headers, !request.authorization.is_none())?;
    if request.graphql.is_some() {
        headers
            .entry(CONTENT_TYPE)
            .or_insert(HeaderValue::from_static("application/json"));
        headers.entry(ACCEPT).or_insert(HeaderValue::from_static(
            "application/graphql-response+json, application/json",
        ));
    }
    let mut outgoing = client.request(method, url).headers(headers);
    outgoing = match request.authorization {
        Authorization::None => outgoing,
        Authorization::Bearer { token } => {
            let token = token.trim();
            if token.is_empty() {
                bail!("Enter a Bearer token");
            }
            outgoing.bearer_auth(token)
        }
        Authorization::Basic { username, password } => {
            if username.contains(':') {
                bail!("A Basic Auth username cannot contain a colon");
            }
            outgoing.basic_auth(username, Some(password))
        }
    };
    if let Some(options) = &request.graphql {
        outgoing = outgoing.body(crate::graphql::payload(&request.body, options)?);
    } else if !request.body.is_empty() {
        outgoing = outgoing.body(prepare_body(&request.body, &request.headers)?);
    }
    Ok(outgoing)
}

fn parse_headers(rows: &[Pair], replaces_authorization: bool) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    for (index, row) in rows.iter().enumerate() {
        if !row.enabled || row.is_blank() {
            continue;
        }
        let number = index + 1;
        let name = HeaderName::from_bytes(row.name.trim().as_bytes())
            .with_context(|| format!("Invalid header name in row {number}"))?;
        if replaces_authorization && name == AUTHORIZATION {
            continue;
        }
        let value = HeaderValue::from_bytes(row.value.trim().as_bytes())
            .with_context(|| format!("Invalid header value in row {number}"))?;
        headers.append(name, value);
    }
    Ok(headers)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
    };

    use super::*;

    #[test]
    fn json_comments_are_removed_from_the_http_body_only() -> Result<()> {
        let request = Request {
            url: "https://example.test".into(),
            method: "POST".into(),
            body: "{/* comment */\"url\":\"https://example.test\",\"ok\":true // done\n}".into(),
            ..Request::default()
        };
        let built = build_request(&Client::new(), request.clone())?.build()?;
        let sent = built.body().unwrap().as_bytes().unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(sent)?,
            serde_json::json!({"url":"https://example.test","ok":true})
        );
        assert!(request.body.contains("/* comment */"));
        Ok(())
    }

    #[test]
    fn authorization_options_send_one_header_and_override_manual_values() -> Result<()> {
        let client = Client::new();
        let request = Request {
            url: "https://example.test".into(),
            headers: vec![Pair {
                name: "authorization".into(),
                value: "Custom abc".into(),
                ..Pair::default()
            }],
            ..Request::default()
        };
        for (authorization, expected) in [
            (Authorization::None, "Custom abc"),
            (
                Authorization::Bearer {
                    token: "token".into(),
                },
                "Bearer token",
            ),
            (
                Authorization::Basic {
                    username: "user".into(),
                    password: "pass".into(),
                },
                "Basic dXNlcjpwYXNz",
            ),
        ] {
            let outgoing = build_request(
                &client,
                Request {
                    authorization,
                    ..request.clone()
                },
            )?
            .build()?;
            let headers: Vec<_> = outgoing.headers().get_all(AUTHORIZATION).iter().collect();
            assert_eq!(headers.len(), 1);
            assert_eq!(headers[0], expected);
        }
        Ok(())
    }

    #[test]
    fn disabled_headers_are_skipped_before_validation() -> Result<()> {
        let request = Request {
            url: "https://example.test".into(),
            headers: vec![
                Pair {
                    name: "invalid header".into(),
                    value: "bad\r\nvalue".into(),
                    enabled: false,
                },
                Pair {
                    name: "X-Test".into(),
                    value: "enabled".into(),
                    ..Pair::default()
                },
                Pair {
                    name: "X-Test".into(),
                    value: "disabled".into(),
                    enabled: false,
                },
            ],
            ..Request::default()
        };
        let outgoing = build_request(&Client::new(), request)?.build()?;
        assert_eq!(outgoing.headers().get_all("x-test").iter().count(), 1);
        assert_eq!(outgoing.headers()["x-test"], "enabled");
        Ok(())
    }

    #[test]
    fn sends_body_and_repeated_headers_and_displays_error_status() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()?;
        let server = thread::spawn(move || -> std::io::Result<()> {
            let (mut stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            let mut reader = BufReader::new(&stream);
            let mut headers = String::new();
            loop {
                let start = headers.len();
                let count = reader.read_line(&mut headers)?;
                if count == 0 || &headers[start..] == "\r\n" {
                    break;
                }
            }
            let mut body = [0; 7];
            reader.read_exact(&mut body)?;
            assert!(headers.starts_with("POST /test?x=1 HTTP/1.1\r\n"));
            assert!(headers.contains("x-test: one\r\n"));
            assert!(headers.contains("x-test: two\r\n"));
            assert!(headers.contains("authorization: Bearer local-token\r\n"));
            assert_eq!(&body, b"{\"a\":1}");
            stream.write_all(b"HTTP/1.1 418 I'm a teapot\r\nContent-Length: 11\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"ok\":true}")
        });
        let request = crate::variables::resolve_request(
            &Request {
                method: "POST".into(),
                url: "{{base_url}}/test?x={{number}}".into(),
                headers: ["one", "two"]
                    .map(|value| Pair {
                        name: "X-Test".into(),
                        value: value.into(),
                        ..Pair::default()
                    })
                    .to_vec(),
                body: "{\"a\":{{number}}}".into(),
                authorization: Authorization::Bearer {
                    token: "{{token}}".into(),
                },
                ..Request::default()
            },
            &[
                crate::variables::Variable {
                    name: "base_url".into(),
                    value: format!("http://{address}"),
                    enabled: true,
                },
                crate::variables::Variable {
                    name: "number".into(),
                    value: "999".into(),
                    enabled: true,
                },
                crate::variables::Variable {
                    name: "token".into(),
                    value: "local-token".into(),
                    enabled: true,
                },
            ],
            &[crate::variables::Variable {
                name: "number".into(),
                value: "1".into(),
                enabled: true,
            }],
        )?;
        let response = send(&client, request)?;
        server.join().expect("test server panicked")?;
        assert_eq!(response.status, 418);
        assert!(response.is_json);
        assert_eq!(response.body.as_ref(), "{\n  \"ok\": true\n}");
        assert!(response.headers.contains("content-type: application/json"));
        Ok(())
    }

    #[test]
    fn reads_and_formats_json_past_the_previous_preview_limit() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let item = format!(r#"{{"value":"{}"}},"#, "x".repeat(512));
        let payload = format!(r#"[{}{{"value":"LAST_ITEM"}}]"#, item.repeat(4999));
        let expected_bytes = payload.len();
        assert!(expected_bytes > 256 * 1024);
        let server = thread::spawn(move || -> std::io::Result<()> {
            let (mut stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            for line in BufReader::new(&stream).lines() {
                if line?.is_empty() {
                    break;
                }
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {expected_bytes}\r\nContent-Type: application/vnd.prestman+json; charset=utf-8\r\nConnection: close\r\n\r\n"
            )?;
            stream.write_all(payload.as_bytes())
        });
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()?;
        let response = send(
            &client,
            Request {
                url: format!("http://{address}/large"),
                ..Request::default()
            },
        )?;
        server.join().expect("test server panicked")?;
        assert_eq!(response.body_bytes, expected_bytes);
        assert!(response.is_json);
        assert!(response.body.ends_with("\"value\": \"LAST_ITEM\"\n  }\n]"));
        assert_eq!(
            serde_json::from_str::<Vec<serde_json::Value>>(&response.body)?.len(),
            5000
        );
        Ok(())
    }
}
