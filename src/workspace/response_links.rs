use super::*;

/// Find HTTP(S) links at a byte offset, inspecting only the clicked line.
fn link_at(text: &str, offset: usize) -> Option<String> {
    if offset >= text.len() || !text.is_char_boundary(offset) {
        return None;
    }
    let start = text[..offset].rfind('\n').map_or(0, |at| at + 1);
    let end = text[offset..]
        .find('\n')
        .map_or(text.len(), |at| offset + at);
    let line = &text[start..end];
    let offset = offset - start;

    // JSON strings can escape slashes, ampersands and other URL characters.
    let mut quoted = None;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quoted.is_some() {
            escaped = true;
            continue;
        }
        if ch == '"' {
            if let Some(begin) = quoted.take() {
                if offset > begin && offset < index {
                    if let Ok(url) = serde_json::from_str::<String>(&line[begin..index + 1]) {
                        if is_web_url(&url) {
                            return Some(url);
                        }
                    }
                }
            } else {
                quoted = Some(index);
            }
        }
    }

    for (start, _) in line.match_indices("http") {
        let remaining = &line[start..];
        if !["http://", "https://", "http:\\/\\/", "https:\\/\\/"]
            .iter()
            .any(|scheme| remaining.starts_with(scheme))
        {
            continue;
        }
        let end = remaining
            .find(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | '<' | '>' | '`'))
            .unwrap_or(remaining.len());
        let mut candidate = &remaining[..end];
        // Ignore prose punctuation while keeping balanced URL parentheses.
        loop {
            let Some(last) = candidate.chars().last() else {
                break;
            };
            let unmatched = match last {
                ')' => Some(('(', ')')),
                ']' => Some(('[', ']')),
                '}' => Some(('{', '}')),
                _ => None,
            }
            .is_some_and(|(open, close)| {
                candidate.matches(close).count() > candidate.matches(open).count()
            });
            if matches!(last, '.' | ',' | ';' | ':' | '!') || unmatched {
                candidate = &candidate[..candidate.len() - last.len_utf8()];
            } else {
                break;
            }
        }
        if offset < start || offset >= start + candidate.len() {
            continue;
        }
        let url = if candidate.contains('\\') {
            serde_json::from_str::<String>(&format!("\"{candidate}\"")).ok()?
        } else {
            candidate.to_owned()
        };
        if is_web_url(&url) {
            return Some(url);
        }
    }
    None
}

fn is_web_url(url: &str) -> bool {
    !url.chars().any(char::is_whitespace)
        && reqwest::Url::parse(url)
            .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
}

impl Workspace {
    pub(super) fn open_response_link(&self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        // The editor handles the press first, so its caret reflects the click,
        // including scrolling, wrapping and folded JSON blocks.
        let modified = if cfg!(target_os = "macos") {
            event.modifiers.platform
        } else {
            event.modifiers.control
        };
        if !modified || event.modifiers.alt || event.modifiers.shift || event.click_count != 1 {
            return;
        }
        let editor = self.response_editor().read(cx);
        let text = editor.value();
        let offset = editor.selected_range().start;
        let previous = text
            .get(..offset)
            .and_then(|text| text.char_indices().next_back())
            .map(|(index, _)| index);
        for index in [Some(offset), previous].into_iter().flatten() {
            let Some(ch) = text.get(index..).and_then(|text| text.chars().next()) else {
                continue;
            };
            let Some(bounds) = editor.range_to_bounds(&(index..index + ch.len_utf8())) else {
                continue;
            };
            // A click in the gutter, padding or after the line must not open a URL.
            if bounds.contains(&event.position) {
                if let Some(url) = link_at(&text, index) {
                    cx.open_url(&url);
                    cx.stop_propagation();
                }
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::link_at;

    #[test]
    fn json_links_decode_escapes_and_preserve_url_punctuation() {
        let body = r#"{"ссылка": "https:\/\/example.com\/a?q=1\u0026b=2!", "other": 1}"#;
        assert_eq!(
            link_at(body, body.find("example").unwrap()).as_deref(),
            Some("https://example.com/a?q=1&b=2!")
        );
        assert_eq!(link_at(body, body.find("other").unwrap()), None);
        assert_eq!(link_at(body, body.find("\"https").unwrap()), None);
    }

    #[test]
    fn text_and_headers_support_multiple_links_and_balanced_parentheses() {
        let body = "Привет (https://example.com/a_(b)). See http://localhost:8080/test,\nLocation: https://example.org/next";
        assert_eq!(
            link_at(body, body.find("example.com").unwrap()).as_deref(),
            Some("https://example.com/a_(b)")
        );
        assert_eq!(
            link_at(body, body.find("localhost").unwrap()).as_deref(),
            Some("http://localhost:8080/test")
        );
        assert_eq!(
            link_at(body, body.find("example.org").unwrap()).as_deref(),
            Some("https://example.org/next")
        );
        assert_eq!(link_at(body, 0), None);
        assert_eq!(link_at(body, body.len()), None);
    }

    #[test]
    fn only_opens_web_urls_on_the_clicked_line() {
        for body in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "https://",
            "text\nhttps://example.com",
        ] {
            assert_eq!(link_at(body, 0), None);
        }
    }
}
