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
    fn is_non_global_ip(ip: std::net::IpAddr) -> bool {
        let v4 = match ip {
            std::net::IpAddr::V4(v4) => Some(v4),
            std::net::IpAddr::V6(v6) => {
                if v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || (v6.segments()[0] & 0xfe00) == 0xfc00
                    || (v6.segments()[0] & 0xffc0) == 0xfe80
                    || (v6.segments()[0] == 0x2001 && v6.segments()[1] == 0x0db8)
                {
                    return true;
                }
                v6.to_ipv4_mapped().or_else(|| v6.to_ipv4())
            }
        };
        if let Some(v4) = v4 {
            let octets = v4.octets();
            return v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || octets[0] == 0
                || (octets[0] == 100 && (octets[1] & 0xc0) == 64)
                || (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19))
                || octets[0] >= 240;
        }
        false
    }

    fn parse_and_validate_url(
        url: &str,
    ) -> Result<(String, u16, Option<std::net::IpAddr>), ToolError> {
        use std::net::ToSocketAddrs;

        let (default_port, rest) = if let Some(rest) = url.strip_prefix("http://") {
            (80u16, rest)
        } else if let Some(rest) = url.strip_prefix("https://") {
            (443u16, rest)
        } else {
            return Err(ToolError::InvalidArguments(format!(
                "url precisa ser http(s): {url}"
            )));
        };

        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let host_port = authority.rsplit('@').next().unwrap_or(authority);
        let (host, port) = if let Some(bracketed) = host_port.strip_prefix('[') {
            let (h, after) = bracketed.split_once(']').unwrap_or((bracketed, ""));
            let p = after
                .strip_prefix(':')
                .and_then(|s| s.parse::<u16>().ok())
                .unwrap_or(default_port);
            (h.trim().to_ascii_lowercase(), p)
        } else if let Some((h, p_str)) = host_port.rsplit_once(':') {
            let p = p_str.parse::<u16>().unwrap_or(default_port);
            (h.trim().to_ascii_lowercase(), p)
        } else {
            (host_port.trim().to_ascii_lowercase(), default_port)
        };

        if host.is_empty()
            || host == "localhost"
            || host.ends_with(".localhost")
            || host.ends_with(".local")
            || host.ends_with(".internal")
            || host.ends_with(".nip.io")
            || host.ends_with(".sslip.io")
            || host == "localtest.me"
            || host.ends_with(".localtest.me")
        {
            return Err(ToolError::InvalidArguments(format!(
                "host local ou privado recusado: {url}"
            )));
        }

        if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            if is_non_global_ip(ip) {
                return Err(ToolError::InvalidArguments(format!(
                    "host local ou privado recusado: {url}"
                )));
            }
            return Ok((host, port, None));
        }

        let addrs: Vec<std::net::SocketAddr> = (host.as_str(), port)
            .to_socket_addrs()
            .map_err(|error| ToolError::Other(format!("falha ao resolver {host}:{port}: {error}")))?
            .collect();
        if addrs.is_empty() {
            return Err(ToolError::Other(format!(
                "nenhum endereço resolvido para {host}"
            )));
        }
        if addrs.iter().any(|addr| is_non_global_ip(addr.ip())) {
            return Err(ToolError::InvalidArguments(format!(
                "host local ou privado recusado: {url}"
            )));
        }

        Ok((host, port, Some(addrs[0].ip())))
    }

    fn resolve_redirect(base_url: &str, location: &str) -> String {
        let loc = location.trim();
        if loc.starts_with("http://") || loc.starts_with("https://") {
            return loc.to_string();
        }
        let (scheme, rest) = if let Some(r) = base_url.strip_prefix("https://") {
            ("https://", r)
        } else if let Some(r) = base_url.strip_prefix("http://") {
            ("http://", r)
        } else {
            return loc.to_string();
        };
        if let Some(without_slashes) = loc.strip_prefix("//") {
            return format!("{scheme}{without_slashes}");
        }
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if loc.starts_with('/') {
            format!("{scheme}{authority}{loc}")
        } else {
            format!("{scheme}{authority}/{loc}")
        }
    }

    let mut current_url = url.trim().to_string();
    for _ in 0..=5 {
        let (host, port, pinned_ip) = parse_and_validate_url(&current_url)?;
        let mut args = vec![
            "-sS".to_string(),
            "-i".to_string(),
            "--proto".to_string(),
            "=http,https".to_string(),
            "--max-time".to_string(),
            "30".to_string(),
            "--compressed".to_string(),
            "-A".to_string(),
            "Mozilla/5.0 (compatible; tupi/0.1)".to_string(),
        ];
        if let Some(ip) = pinned_ip {
            args.push("--resolve".to_string());
            args.push(format!("{host}:{port}:{ip}"));
        }
        args.push(current_url.clone());

        let output = crate::proc::program("curl", &args, cwd, Duration::from_secs(45))?;
        if !output.success {
            let detail = if output.stderr.trim().is_empty() {
                output.stdout.trim()
            } else {
                output.stderr.trim()
            };
            return Err(ToolError::Other(format!(
                "falha ao baixar {current_url}: {}",
                crate::exec::clip(detail, 600, 100)
            )));
        }

        let raw = &output.stdout;
        let (headers, body) = raw
            .split_once("\r\n\r\n")
            .or_else(|| raw.split_once("\n\n"))
            .unwrap_or(("", raw.as_str()));

        let status = headers
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse::<u16>().ok())
            .unwrap_or(200);

        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = headers.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.trim().eq_ignore_ascii_case("location") {
                    Some(value.trim())
                } else {
                    None
                }
            });
            if let Some(loc) = location {
                current_url = resolve_redirect(&current_url, loc);
                continue;
            }
        }

        return Ok(body.to_string());
    }

    Err(ToolError::Other(format!(
        "muitos redirecionamentos ao baixar {url}"
    )))
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

        let cwd = std::path::Path::new(".");
        for blocked in [
            "http://127.0.0.1:8080/secret",
            "http://localhost/admin",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]:8000/",
            "http://10.0.0.1/internal",
            "http://172.16.0.1/internal",
            "http://192.168.1.1/router",
            "http://[::ffff:127.0.0.1]:8080/secret",
            "http://[::ffff:169.254.169.254]/latest/meta-data/",
            "http://100.64.0.1/cgnat",
            "http://127.0.0.1.nip.io/admin",
            "http://localtest.me/admin",
        ] {
            let err = curl(blocked, cwd).unwrap_err();
            assert!(
                matches!(err, ToolError::InvalidArguments(_)),
                "expected {blocked} to be rejected before spawning curl, got {err:?}"
            );
        }
    }
}
