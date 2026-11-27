use std::ops::Range;

use gpui_kit::{HighlightStyle, component::input::TextDecoration, rgb};

use super::view::{AMBER, AMBER_BG, VIOLET, VIOLET_BG};
use crate::request::{path_parameter, path_range};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    Variable,
    Path,
}

fn tokens(url: &str) -> Vec<(Range<usize>, Token)> {
    let mut runs = Vec::new();
    let mut offset = 0;
    while let Some(start) = url[offset..].find("{{") {
        let start = offset + start;
        let Some(end) = url[start + 2..].find("}}") else {
            break;
        };
        let end = start + 2 + end;
        let name = &url[start + 2..end];
        if !name.trim().is_empty() && !name.contains(['{', '}']) {
            runs.push((start..end + 2, Token::Variable));
        }
        offset = end + 2;
    }
    let path = path_range(url);
    let mut start = path.start;
    for segment in url[path].split('/') {
        if path_parameter(segment).is_some() {
            runs.push((start..start + segment.len(), Token::Path));
        }
        start += segment.len() + 1;
    }
    runs.sort_unstable_by_key(|(range, _)| range.start);
    runs
}

pub(super) fn decorations(url: &str) -> Vec<TextDecoration> {
    tokens(url)
        .into_iter()
        .map(|(range, token)| {
            let (foreground, background) = match token {
                Token::Variable => (VIOLET, VIOLET_BG),
                Token::Path => (AMBER, AMBER_BG),
            };
            TextDecoration::new(
                range,
                HighlightStyle {
                    color: Some(rgb(foreground).into()),
                    background_color: Some(rgb(background).into()),
                    ..Default::default()
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_templates_in_all_url_parts_and_only_path_parameters_in_the_path() {
        let url = "{{scheme}}://user:password@[::1]:8443/темы/:id/:other?name={{query}}&literal=:query#{{fragment}}/:anchor";
        let actual = tokens(url)
            .into_iter()
            .map(|(range, kind)| (&url[range], kind))
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            vec![
                ("{{scheme}}", Token::Variable),
                (":id", Token::Path),
                (":other", Token::Path),
                ("{{query}}", Token::Variable),
                ("{{fragment}}", Token::Variable),
            ]
        );
    }

    #[test]
    fn plain_urls_and_unfinished_placeholders_have_no_highlights() {
        for url in [
            "https://example.com:8080/users?id=123",
            "{{}}",
            "{{   }}",
            "https://example.com/{{incomplete",
            "http://[::1]:8080/path",
            "https://example.com/:/{{code}}",
        ] {
            let actual = tokens(url);
            if url.ends_with("{{code}}") {
                assert_eq!(&url[actual[0].0.clone()], "{{code}}");
                assert_eq!(actual.len(), 1);
            } else {
                assert!(actual.is_empty(), "{url}: {actual:?}");
            }
        }
    }

    #[test]
    fn decorations_match_utf8_byte_ranges_without_overlapping() {
        let url = "{{хост}}/путь/:код?one={{значение}}&two={{значение}}";
        let highlights = decorations(url);
        assert_eq!(highlights.len(), 4);
        for pair in highlights.windows(2) {
            assert!(pair[0].range.end <= pair[1].range.start);
        }
        for decoration in highlights {
            assert!(url.is_char_boundary(decoration.range.start));
            assert!(url.is_char_boundary(decoration.range.end));
            assert!(decoration.style.color.is_some());
        }
    }
}
