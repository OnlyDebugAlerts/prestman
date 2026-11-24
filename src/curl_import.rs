//! Import a single cURL command as data. Never invokes a shell, reads files or sends requests.
use std::{iter::Peekable, str::Chars};

use anyhow::{Context, Result, bail, ensure};

use crate::request::{Authorization, Pair, Request, merge_path_variables, query_rows};

fn command_text(text: &str) -> &str {
    text.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n'])
        .strip_prefix("$ ")
        .unwrap_or(text.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']))
}

pub fn is_curl(text: &str) -> bool {
    let first = command_text(text)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(['\'', '"']);
    if first.contains("://")
        || (first.contains('/')
            && !first.starts_with('/')
            && !first.starts_with("./")
            && !first.starts_with("../"))
    {
        return false;
    }
    first.rsplit(['/', '\\']).next().is_some_and(|name| {
        name.eq_ignore_ascii_case("curl") || name.eq_ignore_ascii_case("curl.exe")
    })
}

fn reject_expansion(chars: &mut Peekable<Chars<'_>>) -> Result<()> {
    ensure!(
        !chars
            .peek()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || matches!(ch, '_' | '{' | '(')),
        "Shell expansions are not supported. Use literal values or {{{{variables}}}}"
    );
    Ok(())
}

fn ansi_string(chars: &mut Peekable<Chars<'_>>) -> Result<String> {
    let mut bytes = Vec::new();
    while let Some(ch) = chars.next() {
        if ch == '\'' {
            return String::from_utf8(bytes).context("Non-UTF-8 cURL data is not supported");
        }
        if ch != '\\' {
            bytes.extend(ch.to_string().as_bytes());
            continue;
        }
        let escaped = chars.next().context("Unfinished escape in cURL")?;
        let code = match escaped {
            'a' => 7,
            'b' => 8,
            'e' | 'E' => 27,
            'f' => 12,
            'n' => 10,
            'r' => 13,
            't' => 9,
            'v' => 11,
            '\\' => b'\\',
            '\'' => b'\'',
            '"' => b'"',
            'x' | 'u' | 'U' => {
                let count = match escaped {
                    'x' => 2,
                    'u' => 4,
                    _ => 8,
                };
                let mut digits = String::new();
                for _ in 0..count {
                    if chars.peek().is_some_and(char::is_ascii_hexdigit) {
                        digits.push(chars.next().unwrap());
                    } else {
                        break;
                    }
                }
                ensure!(!digits.is_empty(), "Invalid escape in cURL");
                let value = u32::from_str_radix(&digits, 16)?;
                if escaped == 'x' {
                    ensure!(value != 0, "NUL bytes in shell arguments are not supported");
                    bytes.push(value as u8);
                } else {
                    let ch = char::from_u32(value).context("Invalid Unicode escape in cURL")?;
                    ensure!(ch != '\0', "NUL bytes in shell arguments are not supported");
                    bytes.extend(ch.to_string().as_bytes());
                }
                continue;
            }
            '0'..='7' => {
                let mut digits = escaped.to_string();
                for _ in 0..2 {
                    if chars.peek().is_some_and(|ch| matches!(ch, '0'..='7')) {
                        digits.push(chars.next().unwrap());
                    } else {
                        break;
                    }
                }
                let value = u16::from_str_radix(&digits, 8)?;
                ensure!(value > 0 && value <= 255, "Invalid byte escape in cURL");
                value as u8
            }
            other => {
                bytes.push(b'\\');
                bytes.extend(other.to_string().as_bytes());
                continue;
            }
        };
        bytes.push(code);
    }
    bail!("Unclosed ANSI quote in cURL")
}

fn words(text: &str) -> Result<Vec<String>> {
    ensure!(!text.contains('\0'), "NUL bytes in cURL are not supported");
    let mut chars = command_text(text).chars().peekable();
    let mut result = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote = None;
    while let Some(ch) = chars.next() {
        if quote == Some('\'') {
            if ch == '\'' {
                quote = None;
            } else {
                word.push(ch);
            }
            continue;
        }
        if ch == '\\' {
            let next = chars.next().context("Unfinished escape in cURL")?;
            if next == '\n' {
                continue;
            }
            if next == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
                continue;
            }
            if quote == Some('"') && !matches!(next, '\\' | '"' | '$' | '`') {
                word.push('\\');
            }
            word.push(next);
            started = true;
            continue;
        }
        if ch == '$' {
            if quote.is_none() && chars.peek() == Some(&'\'') {
                chars.next();
                word.push_str(&ansi_string(&mut chars)?);
                started = true;
                continue;
            }
            reject_expansion(&mut chars)?;
        }
        ensure!(ch != '`', "Shell command substitution is not supported");
        if quote == Some('"') {
            if ch == '"' {
                quote = None;
            } else {
                word.push(ch);
            }
            continue;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                started = true;
            }
            ch if ch.is_whitespace() => {
                if started {
                    result.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            '#' if !started => {
                for ch in chars.by_ref() {
                    if ch == '\n' {
                        break;
                    }
                }
            }
            ';' | '|' | '&' | '<' | '>' => bail!(
                "Paste a single cURL command without shell operators; quote URLs containing &"
            ),
            ch => {
                word.push(ch);
                started = true;
            }
        }
    }
    ensure!(quote.is_none(), "Unclosed quote in cURL");
    if started {
        result.push(word);
    }
    Ok(result)
}

fn pair(name: impl Into<String>, value: impl Into<String>) -> Pair {
    Pair {
        name: name.into(),
        value: value.into(),
        enabled: true,
    }
}

fn header(text: &str) -> Result<Pair> {
    ensure!(
        !text.starts_with('@'),
        "Header files are not supported; paste headers with -H"
    );
    ensure!(
        !text.contains(['\r', '\n']),
        "A cURL header must be a single line"
    );
    let (name, value, enabled) = if let Some((name, value)) = text.split_once(':') {
        (name.trim(), value.trim_start(), !value.trim().is_empty())
    } else if let Some(name) = text.strip_suffix(';') {
        (name.trim(), "", true)
    } else {
        bail!("Expected a header in the form 'Name: value'")
    };
    ensure!(!name.is_empty(), "A cURL header has an empty name");
    Ok(Pair {
        name: name.into(),
        value: value.into(),
        enabled,
    })
}

fn encode(text: &str) -> String {
    let mut result = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            result.push(byte as char);
        } else {
            use std::fmt::Write as _;
            write!(result, "%{byte:02X}").expect("String write");
        }
    }
    result
}

fn urlencode(text: &str) -> Result<String> {
    if let Some(value) = text.strip_prefix('=') {
        return Ok(encode(value));
    }
    if let Some((name, value)) = text.split_once('=') {
        return Ok(format!("{name}={}", encode(value)));
    }
    ensure!(
        !text.contains('@'),
        "File input in --data-urlencode is not supported"
    );
    Ok(encode(text))
}

fn append_query(url: &str, data: &str) -> String {
    if data.is_empty() {
        return url.into();
    }
    let (address, fragment) = url
        .split_once('#')
        .map_or((url, None), |(url, fragment)| (url, Some(fragment)));
    let join = if !address.contains('?') {
        "?"
    } else if address.ends_with(['?', '&']) {
        ""
    } else {
        "&"
    };
    format!(
        "{address}{join}{data}{}",
        fragment
            .map(|fragment| format!("#{fragment}"))
            .unwrap_or_default()
    )
}

pub fn parse(text: &str) -> Result<Request> {
    ensure!(is_curl(text), "Expected a cURL command");
    let args = words(text)?;
    let mut url = None;
    let mut method = None;
    let mut head = false;
    let mut get = false;
    let mut json = false;
    let mut has_body = false;
    let mut body = String::new();
    let mut headers = Vec::new();
    let mut authorization = Authorization::None;
    let mut cookie = None;
    let mut agent = None;
    let mut referer = None;
    let mut query = Vec::new();
    let mut end_options = false;
    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if arg == "--" && !end_options {
            end_options = true;
            continue;
        }
        if end_options || !arg.starts_with('-') || arg == "-" {
            ensure!(url.is_none(), "A cURL import supports one URL at a time");
            url = Some(arg.clone());
            continue;
        }
        let (option, attached) = if arg.starts_with("--") {
            arg.split_once('=')
                .map_or((arg.as_str(), None), |(key, value)| {
                    (key, Some(value.to_owned()))
                })
        } else if arg.len() > 2
            && matches!(
                arg.as_bytes()[1],
                b'X' | b'H' | b'd' | b'u' | b'b' | b'A' | b'e'
            )
        {
            (&arg[..2], Some(arg[2..].to_owned()))
        } else {
            (arg.as_str(), None)
        };
        let takes_value = matches!(
            option,
            "--url"
                | "-X"
                | "--request"
                | "-H"
                | "--header"
                | "-d"
                | "--data"
                | "--data-ascii"
                | "--data-raw"
                | "--data-binary"
                | "--data-urlencode"
                | "--json"
                | "--url-query"
                | "-u"
                | "--user"
                | "--oauth2-bearer"
                | "-b"
                | "--cookie"
                | "-A"
                | "--user-agent"
                | "-e"
                | "--referer"
        );
        let value = if takes_value {
            if let Some(value) = attached {
                value
            } else {
                let value = args
                    .get(i)
                    .with_context(|| format!("Missing value for {option}"))?
                    .clone();
                i += 1;
                value
            }
        } else {
            ensure!(attached.is_none(), "Unexpected value for {option}");
            String::new()
        };
        match option {
            "--url" => {
                ensure!(url.is_none(), "A cURL import supports one URL at a time");
                url = Some(value);
            }
            "-X" | "--request" => {
                reqwest::Method::from_bytes(value.as_bytes())
                    .context("Invalid HTTP method in cURL")?;
                method = Some(value);
            }
            "-H" | "--header" => headers.push(header(&value)?),
            "-d" | "--data" | "--data-ascii" | "--data-raw" | "--data-binary"
            | "--data-urlencode" | "--json" => {
                ensure!(
                    option == "--data-raw" || !value.starts_with('@'),
                    "File input is not supported; paste the contents using --data-raw"
                );
                let value = if option == "--data-urlencode" {
                    urlencode(&value)?
                } else {
                    value
                };
                if has_body && option != "--json" {
                    body.push('&');
                }
                body.push_str(&value);
                has_body = true;
                json |= option == "--json";
            }
            "--url-query" => query.push(if let Some(raw) = value.strip_prefix('+') {
                raw.to_owned()
            } else {
                urlencode(&value)?
            }),
            "-u" | "--user" => {
                let (username, password) = value.split_once(':').unwrap_or((&value, ""));
                authorization = Authorization::Basic {
                    username: username.into(),
                    password: password.into(),
                };
            }
            "--oauth2-bearer" => authorization = Authorization::Bearer { token: value },
            "-b" | "--cookie" => {
                ensure!(
                    value.is_empty() || value.contains('='),
                    "Cookie files are not supported; paste cookies as name=value"
                );
                cookie = Some(value);
            }
            "-A" | "--user-agent" => agent = Some(value),
            "-e" | "--referer" => {
                ensure!(
                    !value.ends_with(";auto"),
                    "Automatic Referer on redirects is not supported"
                );
                referer = Some(value);
            }
            "-G" | "--get" => get = true,
            "-I" | "--head" => head = true,
            "--location" | "--compressed" | "--globoff" | "--silent" | "--show-error"
            | "--include" | "--verbose" | "--fail" | "--fail-with-body" | "--basic" => {}
            flags
                if flags.starts_with('-')
                    && !flags.starts_with("--")
                    && flags[1..].chars().all(|flag| "LsSigvfGI".contains(flag)) =>
            {
                head |= flags.contains('I');
                get |= flags.contains('G');
            }
            "-F" | "--form" | "--form-string" | "-T" | "--upload-file" => {
                bail!("Multipart forms and file uploads are not supported by cURL import yet")
            }
            other => bail!("Unsupported cURL option: {other}"),
        }
    }
    let mut url = url.context("No URL found in cURL")?;
    ensure!(!url.trim().is_empty(), "The cURL URL is empty");
    if !url.contains("://") && !url.starts_with("{{") {
        url = format!("http://{url}");
    }
    ensure!(
        url.starts_with("{{")
            || url
                .get(..7)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
            || url
                .get(..8)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://")),
        "Only HTTP and HTTPS URLs are supported"
    );
    if !url.contains("{{") {
        let parsed = reqwest::Url::parse(&url).context("Invalid HTTP URL in cURL")?;
        ensure!(
            matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some(),
            "Only HTTP and HTTPS URLs are supported"
        );
    }
    ensure!(
        !head || !has_body || get,
        "Combining --head with a request body is not supported"
    );
    for value in query {
        url = append_query(&url, &value);
    }
    if get {
        url = append_query(&url, &body);
        body.clear();
    }
    let contains = |headers: &[Pair], name: &str| {
        headers
            .iter()
            .any(|row| row.name.eq_ignore_ascii_case(name))
    };
    if json {
        if !contains(&headers, "Content-Type") {
            headers.push(pair("Content-Type", "application/json"));
        }
        if !contains(&headers, "Accept") {
            headers.push(pair("Accept", "application/json"));
        }
    } else if has_body && !get && !contains(&headers, "Content-Type") {
        headers.push(pair("Content-Type", "application/x-www-form-urlencoded"));
    }
    for (name, value) in [
        ("Cookie", cookie),
        ("User-Agent", agent),
        ("Referer", referer),
    ] {
        if let Some(value) = value {
            if !contains(&headers, name) {
                headers.push(pair(name, value));
            }
        }
    }
    if contains(&headers, "Authorization") {
        authorization = Authorization::None;
    }
    let method = method.unwrap_or_else(|| {
        if head {
            "HEAD"
        } else if get || !has_body {
            "GET"
        } else {
            "POST"
        }
        .into()
    });
    Ok(Request {
        method,
        query: query_rows(&url),
        path_variables: merge_path_variables(&url, &[]),
        url,
        headers,
        body,
        authorization,
        ..Request::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_browser_curl_with_multiline_json_unicode_repeated_headers_and_query() -> Result<()> {
        let command = r#"curl 'https://example.test/items?tag=one&tag=two' \
          -H 'Content-Type: application/json' \
          -H 'X-Repeat: one' -H 'X-Repeat: two' \
          --data-raw '{"title":"Привет","message":"it'\''s valid"}'"#;
        let request = parse(command)?;
        assert_eq!(request.method, "POST");
        assert_eq!(request.url, "https://example.test/items?tag=one&tag=two");
        assert_eq!(request.query.len(), 2);
        assert_eq!(
            request
                .headers
                .iter()
                .filter(|header| header.name == "X-Repeat")
                .map(|header| header.value.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&request.body)?,
            serde_json::json!({"title":"Привет","message":"it's valid"})
        );
        Ok(())
    }

    #[test]
    fn supports_double_quotes_ansi_quotes_compact_flags_and_crlf_continuations() -> Result<()> {
        let command = "curl.exe \"http://example.test/path\" \\\r\n -XPATCH -H\"X-Test: \\\"one\\\"\" --data-binary $'a\\nb\\t\\u041f\\xD1\\x80'";
        let request = parse(command)?;
        assert_eq!(request.method, "PATCH");
        assert_eq!(request.headers[0].value, "\"one\"");
        assert_eq!(request.body, "a\nb\tПр");
        assert_eq!(
            request.headers[1].value,
            "application/x-www-form-urlencoded"
        );
        Ok(())
    }

    #[test]
    fn json_sets_defaults_concatenates_pieces_and_honors_explicit_headers() -> Result<()> {
        let request = parse(
            r#"curl --url=https://example.test --json '{"a":' --json '1}' -H 'Accept: text/plain'"#,
        )?;
        assert_eq!(request.body, r#"{"a":1}"#);
        assert_eq!(request.method, "POST");
        assert_eq!(
            request.headers,
            vec![
                pair("Accept", "text/plain"),
                pair("Content-Type", "application/json")
            ]
        );
        let request = parse("curl https://example.test --json '{}' -H 'Accept:' -H 'X-Empty;'")?;
        assert!(!request.headers[0].enabled);
        assert_eq!(request.headers[1], pair("X-Empty", ""));
        assert_eq!(
            request
                .headers
                .iter()
                .filter(|header| header.name == "Accept")
                .count(),
            1
        );
        Ok(())
    }

    #[test]
    fn get_moves_encoded_data_into_existing_query_and_preserves_the_fragment() -> Result<()> {
        let request = parse(
            "curl -sSLG 'https://example.test/items?tag=one#section' --data-urlencode 'q=два слова' -d 'tag=two'",
        )?;
        assert_eq!(request.method, "GET");
        assert!(request.body.is_empty());
        assert!(request.headers.is_empty());
        assert_eq!(
            request.url,
            "https://example.test/items?tag=one&q=%D0%B4%D0%B2%D0%B0%20%D1%81%D0%BB%D0%BE%D0%B2%D0%B0&tag=two#section"
        );
        assert_eq!(request.query[1].value, "два слова");
        let request = parse(
            "curl https://example.test --url-query 'name=one + two' --url-query '+raw=yes' -XDELETE",
        )?;
        assert_eq!(request.method, "DELETE");
        assert_eq!(
            request.url,
            "https://example.test?name=one%20%2B%20two&raw=yes"
        );
        Ok(())
    }

    #[test]
    fn imports_auth_cookies_user_agent_and_referer_without_default_json_headers() -> Result<()> {
        let request = parse(
            "curl example.test -u 'name:p:a:ss' -b 'session=abc; user=42' -A 'Test Agent' -e https://origin.test",
        )?;
        assert_eq!(request.url, "http://example.test");
        assert_eq!(
            request.authorization,
            Authorization::Basic {
                username: "name".into(),
                password: "p:a:ss".into()
            }
        );
        assert_eq!(
            request.headers,
            vec![
                pair("Cookie", "session=abc; user=42"),
                pair("User-Agent", "Test Agent"),
                pair("Referer", "https://origin.test")
            ]
        );
        let bearer = parse("curl '{{base_url}}/items/:id' --oauth2-bearer '{{token}}'")?;
        assert_eq!(
            bearer.authorization,
            Authorization::Bearer {
                token: "{{token}}".into()
            }
        );
        assert_eq!(bearer.path_variables[0].name, "id");
        let custom =
            parse("curl https://example.test -u user:pass -H 'Authorization: Custom token'")?;
        assert_eq!(custom.authorization, Authorization::None);
        assert_eq!(custom.headers, vec![pair("Authorization", "Custom token")]);
        Ok(())
    }

    #[test]
    fn empty_data_custom_methods_and_literal_shell_text_are_preserved() -> Result<()> {
        assert_eq!(parse("curl https://example.test -d ''")?.method, "POST");
        assert_eq!(parse("curl https://example.test -I")?.method, "HEAD");
        assert_eq!(
            parse("curl https://example.test -X PROPFIND")?.method,
            "PROPFIND"
        );
        assert_eq!(
            parse("curl https://example.test --data-raw '@literal'")?.body,
            "@literal"
        );
        assert_eq!(
            parse("curl https://example.test -d 'one=1' -d 'two=2'")?.body,
            "one=1&two=2"
        );
        assert_eq!(
            parse("curl https://example.test --data-raw '$(never-run) `never-run` $TOKEN'")?.body,
            "$(never-run) `never-run` $TOKEN"
        );
        Ok(())
    }

    #[test]
    fn recognizes_commands_without_intercepting_regular_url_paste() {
        for text in [
            "curl https://example.test",
            "$ curl https://example.test",
            "  /usr/bin/curl 'https://example.test'",
            "curl.exe https://example.test",
            "'curl' https://example.test",
        ] {
            assert!(is_curl(text), "{text}");
        }
        for text in [
            "https://example.test/curl",
            "example.test/curl",
            "curl.example.test",
            "curling",
            "{{url}}",
            "",
        ] {
            assert!(!is_curl(text), "{text}");
        }
    }

    #[test]
    fn imported_curl_sends_expected_http_request() -> Result<()> {
        use std::{
            io::{BufRead, BufReader, Read, Write},
            net::TcpListener,
            thread,
            time::Duration,
        };

        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let request = parse(&format!(
            r#"curl 'http://{address}/items?tag=one&tag=two' -X PATCH
             -H 'X-Repeat: one' -H 'X-Repeat: two'
             --user 'tester:secret' --json '{{"ok":true}}'"#
        ))?;
        let server = thread::spawn(move || -> std::io::Result<(String, String)> {
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
            let mut body = [0; 11];
            reader.read_exact(&mut body)?;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
            Ok((headers, String::from_utf8_lossy(&body).into_owned()))
        });
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()?;
        let response = crate::http::send(&client, request)?;
        let (headers, body) = server.join().expect("local HTTP server panicked")?;
        assert_eq!(response.status, 200);
        assert!(headers.starts_with("PATCH /items?tag=one&tag=two HTTP/1.1\r\n"));
        assert!(headers.contains("x-repeat: one\r\n"));
        assert!(headers.contains("x-repeat: two\r\n"));
        assert!(headers.contains("content-type: application/json\r\n"));
        assert!(headers.contains("accept: application/json\r\n"));
        assert!(headers.contains("authorization: Basic dGVzdGVyOnNlY3JldA==\r\n"));
        assert_eq!(body, r#"{"ok":true}"#);
        Ok(())
    }

    #[test]
    fn rejects_incomplete_unsupported_multiple_or_shell_commands() {
        for command in [
            "curl",
            "curl -H",
            "curl 'https://example.test",
            "curl https://a.test https://b.test",
            "curl https://example.test --next",
            "curl https://example.test --data @file.json",
            "curl https://example.test --form file=@file",
            "curl https://example.test --cookie cookies.txt",
            "curl https://example.test -k",
            "curl https://example.test; echo bad",
            "curl https://example.test | cat",
            "curl $(echo https://example.test)",
            "curl \"$URL\"",
            "curl ftp://example.test",
            "curl ftp://{{host}}",
            "curl https://example.test --data-binary $'\\xff'",
            "curl https://example.test --data-binary $'\\0'",
        ] {
            assert!(parse(command).is_err(), "{command}");
        }
    }
}
