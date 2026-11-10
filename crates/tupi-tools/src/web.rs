use std::path::Path;
use std::time::Duration;

use tupi_kernel::ToolError;

#[derive(Debug, Clone)]
pub struct Block {
    pub anchor: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

pub fn curl(url: &str, cwd: &Path) -> Result<String, ToolError> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ToolError::InvalidArguments(format!(
            "url precisa ser http(s): {url}"
        )));
    }
    let args = vec![
        "-sSL".to_string(),
        "--max-time".to_string(),
        "30".to_string(),
        "--compressed".to_string(),
        "-A".to_string(),
        "Mozilla/5.0 (compatible; tupi/0.1)".to_string(),
        url.to_string(),
    ];
    let output = crate::proc::program("curl", &args, cwd, Duration::from_secs(45))?;
    if !output.success {
        let detail = if output.stderr.trim().is_empty() {
            output.stdout.trim()
        } else {
            output.stderr.trim()
        };
        return Err(ToolError::Other(format!(
            "falha ao baixar {url}: {}",
            crate::exec::clip(detail, 600, 100)
        )));
    }
    Ok(output.stdout)
}

pub fn decode_entities(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
}

pub fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for character in html.chars() {
        match character {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(character),
            _ => {}
        }
    }
    let decoded = decode_entities(&out);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn html_to_blocks(html: &str) -> Vec<Block> {
    let chars: Vec<char> = html.chars().collect();
    let mut text = String::new();
    let mut index = 0;
    let mut skip = 0usize;

    while index < chars.len() {
        if chars[index] == '<' {
            let mut end = index;
            while end < chars.len() && chars[end] != '>' {
                end += 1;
            }
            let tag: String = chars[index..=end.min(chars.len().saturating_sub(1))]
                .iter()
                .collect();
            let lower = tag.to_lowercase();
            if lower.starts_with("<script") || lower.starts_with("<style") {
                skip += 1;
            } else if lower.starts_with("</script") || lower.starts_with("</style") {
                skip = skip.saturating_sub(1);
            } else if skip == 0 {
                let name = lower
                    .trim_start_matches('<')
                    .trim_start_matches('/')
                    .split(|c: char| c.is_whitespace() || c == '>')
                    .next()
                    .unwrap_or("");
                if matches!(
                    name,
                    "p" | "div"
                        | "br"
                        | "li"
                        | "ul"
                        | "ol"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "tr"
                        | "td"
                        | "th"
                        | "pre"
                        | "section"
                        | "article"
                        | "header"
                        | "footer"
                        | "blockquote"
                        | "dd"
                        | "dt"
                ) {
                    text.push('\n');
                } else {
                    text.push(' ');
                }
            }
            index = end + 1;
            continue;
        }
        if skip == 0 {
            text.push(chars[index]);
        }
        index += 1;
    }

    let decoded = decode_entities(&text);
    let mut blocks = Vec::new();
    let mut current = String::new();
    for line in decoded.lines() {
        let line = line.trim();
        if line.is_empty() {
            if current.len() >= 40 {
                blocks.push(make_block(&current));
            }
            current.clear();
            continue;
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(line);
        if current.len() >= 1_200 {
            blocks.push(make_block(&current));
            current.clear();
        }
    }
    if current.len() >= 40 {
        blocks.push(make_block(&current));
    }
    blocks
}

pub fn text_blocks(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            if current.len() >= 40 {
                blocks.push(make_block(&current));
            }
            current.clear();
            continue;
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
        if current.len() >= 1_200 {
            blocks.push(make_block(&current));
            current.clear();
        }
    }
    if current.len() >= 40 {
        blocks.push(make_block(&current));
    }
    if blocks.is_empty() && !text.trim().is_empty() {
        blocks.push(make_block(text));
    }
    blocks
}

fn make_block(text: &str) -> Block {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let anchor = format!(
        "blk_{}",
        &blake3::hash(compact.as_bytes()).to_hex().to_string()[..8]
    );
    Block {
        anchor,
        text: compact,
    }
}

pub fn title(html: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let start = lower.find("<title")?;
    let open = lower[start..].find('>')? + start + 1;
    let end = lower[open..].find("</title>")? + open;
    let text = strip_tags(&html[open..end]);
    if text.is_empty() { None } else { Some(text) }
}

pub fn rank(blocks: Vec<Block>, question: Option<&str>) -> Vec<Block> {
    let Some(question) = question.map(str::trim).filter(|q| !q.is_empty()) else {
        return blocks;
    };
    let terms: Vec<String> = question
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|term| term.len() >= 3)
        .map(str::to_string)
        .collect();
    if terms.is_empty() {
        return blocks;
    }
    let mut scored: Vec<(usize, Block)> = blocks
        .into_iter()
        .map(|block| {
            let lower = block.text.to_lowercase();
            let score = terms
                .iter()
                .filter(|term| lower.contains(term.as_str()))
                .count();
            (score, block)
        })
        .collect();
    scored.sort_by_key(|item| std::cmp::Reverse(item.0));
    scored.into_iter().map(|(_, block)| block).collect()
}

pub fn url_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = &text[index + 1..index + 3];
            if let Ok(value) = u8::from_str_radix(hex, 16) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(tag[start..end].to_string())
}

fn unredirect(url: &str) -> String {
    let Some(index) = url.find("uddg=") else {
        return url.to_string();
    };
    let rest = &url[index + 5..];
    let end = rest.find('&').unwrap_or(rest.len());
    percent_decode(&rest[..end])
}

pub fn parse_duckduckgo(html: &str) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    let mut rest = html;

    while let Some(position) = rest.find("result__a") {
        let start = rest[..position].rfind("<a").unwrap_or(position);
        let Some(tag_end) = rest[position..].find('>').map(|offset| position + offset) else {
            break;
        };
        let tag = &rest[start..tag_end];
        let href = attribute(tag, "href").unwrap_or_default();
        let text_start = tag_end + 1;
        let Some(text_end) = rest[text_start..]
            .find("</a>")
            .map(|offset| text_start + offset)
        else {
            break;
        };
        let title = strip_tags(&rest[text_start..text_end]);

        let after = &rest[text_end..];
        let snippet = match after.find("result__snippet") {
            Some(snippet_position) => {
                let snippet_start = after[snippet_position..]
                    .find('>')
                    .map(|offset| snippet_position + offset + 1)
                    .unwrap_or(after.len());
                let snippet_end = after[snippet_start..]
                    .find("</a>")
                    .map(|offset| snippet_start + offset)
                    .unwrap_or(after.len());
                strip_tags(&after[snippet_start..snippet_end])
            }
            None => String::new(),
        };

        if !href.is_empty() && !title.is_empty() {
            hits.push(SearchHit {
                title,
                url: unredirect(&href),
                snippet,
            });
        }
        rest = &rest[text_end + 4..];
    }

    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_become_text_and_scripts_disappear() {
        let html = "<html><head><title>T &amp; A</title><script>var x = 1;</script></head><body><p>Olá <b>mundo</b>, este parágrafo é bem maior que quarenta caracteres.</p><p>Segundo bloco com mais de quarenta caracteres para valer.</p></body></html>";
        assert_eq!(title(html).as_deref(), Some("T & A"));
        let blocks = html_to_blocks(html);
        assert!(blocks.iter().any(|block| block.text.contains("mundo")));
        assert!(!blocks.iter().any(|block| block.text.contains("var x")));
        assert!(blocks.iter().all(|block| block.anchor.starts_with("blk_")));
    }

    #[test]
    fn the_question_ranks_the_blocks() {
        let blocks = vec![
            Block {
                anchor: "a".to_string(),
                text: "nada relevante aqui".to_string(),
            },
            Block {
                anchor: "b".to_string(),
                text: "o token é validado aqui".to_string(),
            },
        ];
        let ranked = rank(blocks, Some("onde o token é validado"));
        assert_eq!(ranked[0].anchor, "b");
    }

    #[test]
    fn a_duckduckgo_result_is_parsed() {
        let html = "<a rel=\"nofollow\" class=\"result__a\" href=\"//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa&amp;rut=1\">Titulo A</a><a class=\"result__snippet\">Trecho <b>a</b>.</a>";
        let hits = parse_duckduckgo(html);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Titulo A");
        assert_eq!(hits[0].url, "https://example.com/a");
        assert_eq!(hits[0].snippet, "Trecho a .");
    }

    #[test]
    fn percent_encoding_round_trips() {
        assert_eq!(percent_decode(&url_encode("a b&c")), "a b&c");
    }
}
