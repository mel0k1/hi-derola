use anyhow::{bail, Result};
use std::time::Duration;

const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_TIMEOUT: u64 = 120;
const USER_AGENT: &str =
    "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko); compatible; hi-derola/0.1";

fn is_textual(mime: &str) -> bool {
    mime.is_empty()
        || mime.starts_with("text/")
        || mime == "application/json"
        || mime.ends_with("+json")
        || mime == "application/xml"
        || mime.ends_with("+xml")
        || mime.contains("javascript")
}

pub async fn fetch(url: &str, timeout: u64) -> Result<(String, String)> {
    let u = url.trim();
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        bail!("url must use http:// or https://");
    }
    let timeout = timeout.clamp(1, MAX_TIMEOUT);
    let http = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .build()?;
    let resp = tokio::time::timeout(
        Duration::from_secs(timeout),
        http.get(u)
            .header("Accept", "text/html;q=0.9, text/plain;q=0.8, */*;q=0.1")
            .send(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("request timed out ({timeout}s)"))??;
    let status = resp.status();
    if !status.is_success() {
        bail!("{status} for {u}");
    }
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let mime = ctype.split(';').next().unwrap_or("").trim().to_lowercase();
    if mime.starts_with("image/")
        || mime.starts_with("video/")
        || mime.starts_with("audio/")
        || mime == "application/pdf"
    {
        bail!("unsupported content type: {mime}");
    }
    if let Some(n) = resp.content_length() {
        if n as usize > MAX_BYTES {
            bail!("response too large ({} bytes)", n);
        }
    }
    let mut body: Vec<u8> = Vec::new();
    let mut resp = resp;
    while let Some(chunk) = resp.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BYTES {
            bail!("response exceeds {} bytes", MAX_BYTES);
        }
    }
    Ok((ctype, String::from_utf8_lossy(&body).to_string()))
}

pub async fn fetch_markdown(url: &str, format: &str, timeout: u64) -> Result<String> {
    let (ctype, body) = fetch(url, timeout).await?;
    let mime = ctype.split(';').next().unwrap_or("").trim().to_lowercase();
    if !is_textual(&mime) {
        bail!("unsupported content type: {mime}");
    }
    if mime.contains("html") && format != "html" {
        Ok(html_to_md(&body))
    } else {
        Ok(body)
    }
}

/// Web search via the DuckDuckGo HTML endpoint (no API key).
pub async fn websearch(query: &str, timeout: u64) -> Result<String> {
    let timeout = timeout.clamp(1, MAX_TIMEOUT);
    let http = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .build()?;
    let resp = tokio::time::timeout(
        Duration::from_secs(timeout),
        http.post("https://html.duckduckgo.com/html/")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(format!("q={}", urlencode(query)))
            .send(),
    )
    .await??;
    let status = resp.status();
    let html = resp.text().await?;
    if !status.is_success() {
        bail!("search failed: {status}");
    }
    let results = parse_results(&html, 8);
    if results.is_empty() {
        return Ok("no results found".into());
    }
    let mut out = format!("Search results for \"{query}\":\n");
    for (i, r) in results.iter().enumerate() {
        out.push_str(&format!(
            "\n{}. {}\n   {}\n   {}\n",
            i + 1,
            r.title,
            r.url,
            r.snippet
        ));
    }
    Ok(out)
}

pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

pub fn parse_results(html: &str, max: usize) -> Vec<SearchHit> {
    let link_re =
        regex::Regex::new(r#"<a[^>]*class="result__a"[^>]*href="([^"]+)"[^>]*>([\s\S]*?)</a>"#)
            .unwrap();
    let link_re2 =
        regex::Regex::new(r#"<a[^>]*href="([^"]+)"[^>]*class="result__a"[^>]*>([\s\S]*?)</a>"#)
            .unwrap();
    let snippet_re =
        regex::Regex::new(r#"<a[^>]*class="result__snippet"[^>]*>([\s\S]*?)</a>"#).unwrap();

    let mut links: Vec<(String, String)> = Vec::new();
    for re in [&link_re, &link_re2] {
        for cap in re.captures_iter(html) {
            links.push((cap[1].to_string(), cap[2].to_string()));
        }
        if !links.is_empty() {
            break;
        }
    }
    let snippets: Vec<String> = snippet_re
        .captures_iter(html)
        .map(|c| strip_tags(&c[1]))
        .collect();

    let mut out = Vec::new();
    for (i, (href, title)) in links.into_iter().enumerate() {
        if out.len() >= max {
            break;
        }
        let url = resolve_ddg_href(&href);
        if url.starts_with("http") && !url.contains("duckduckgo.com") {
            out.push(SearchHit {
                title: strip_tags(&title),
                url,
                snippet: snippets.get(i).cloned().unwrap_or_default(),
            });
        }
    }
    out
}

fn resolve_ddg_href(href: &str) -> String {
    if let Some(pos) = href.find("uddg=") {
        let raw = &href[pos + 5..];
        let raw = raw.split('&').next().unwrap_or(raw);
        urldecode(raw)
    } else {
        href.to_string()
    }
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(b);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for ch in html.chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let out = entities(&out);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i..].starts_with('&') {
            let rest = &s[i..];
            if let Some(e) = rest.find(';').filter(|e| *e <= 10) {
                let ent = &rest[1..e];
                let rep = match ent {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some(' '),
                    "copy" => Some('\u{a9}'),
                    "reg" => Some('\u{ae}'),
                    "mdash" => Some('\u{2014}'),
                    "ndash" => Some('\u{2013}'),
                    "hellip" => Some('\u{2026}'),
                    "middot" => Some('\u{b7}'),
                    "laquo" => Some('\u{ab}'),
                    "raquo" => Some('\u{bb}'),
                    _ => None,
                };
                let parsed = rep.or_else(|| {
                    ent.strip_prefix('#').and_then(|num| {
                        if let Some(hex) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
                            u32::from_str_radix(hex, 16).ok()
                        } else {
                            num.parse::<u32>().ok()
                        }
                        .and_then(char::from_u32)
                    })
                });
                if let Some(c) = parsed {
                    out.push(c);
                    i += e + 1;
                    continue;
                }
            }
        }
        let c = s[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

struct Md {
    out: String,
    link: Option<(String, String)>,
    pre: bool,
}

impl Md {
    fn block(&mut self) {
        if self.out.is_empty() || self.out.ends_with("\n\n") {
            return;
        }
        while !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        if !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
    }

    fn text(&mut self, t: &str) {
        if self.pre {
            self.out.push_str(t);
            return;
        }
        let t = t.replace('\n', " ");
        if t.trim().is_empty() {
            if t.contains(' ')
                && !self.out.is_empty()
                && !self.out.ends_with(' ')
                && !self.out.ends_with('\n')
            {
                self.out.push(' ');
            }
            return;
        }
        if let Some((_, buf)) = self.link.as_mut() {
            buf.push_str(&t);
        } else {
            self.out.push_str(&t);
        }
    }
}

const SKIP: &[&str] = &[
    "script", "style", "noscript", "iframe", "svg", "template", "object", "embed", "head",
];

fn mark(md: &mut Md, s: &str) {
    match md.link.as_mut() {
        Some((_, buf)) => buf.push_str(s),
        None => md.out.push_str(s),
    }
}
const BLOCK: &[&str] = &[
    "p",
    "div",
    "section",
    "article",
    "header",
    "footer",
    "main",
    "nav",
    "aside",
    "figure",
    "figcaption",
    "form",
    "table",
    "thead",
    "tbody",
    "ul",
    "ol",
    "dl",
    "blockquote",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "tr",
];

fn tag_name(raw: &str) -> String {
    raw.trim_start_matches('/')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

fn attr(attrs: &str, key: &str) -> String {
    let low = attrs.to_lowercase();
    for pat in [format!("{key}=\""), format!("{key}='")] {
        if let Some(p) = low.find(&pat) {
            let quote = pat.chars().last().unwrap();
            let rest = &attrs[p + pat.len()..];
            if let Some(e) = rest.find(quote) {
                return rest[..e].to_string();
            }
        }
    }
    String::new()
}

pub fn html_to_md(html: &str) -> String {
    let mut md = Md {
        out: String::new(),
        link: None,
        pre: false,
    };
    let mut i = 0;
    while i < html.len() {
        if html[i..].starts_with("<!--") {
            match html[i..].find("-->") {
                Some(p) => i += p + 3,
                None => break,
            }
            continue;
        }
        if !html[i..].starts_with('<') {
            let next = html[i..].find('<').map(|p| i + p).unwrap_or(html.len());
            let text = entities(&html[i..next]);
            md.text(&text);
            i = next;
            continue;
        }
        let Some(end) = html[i..].find('>') else {
            break;
        };
        let raw = &html[i + 1..i + end];
        i += end + 1;
        let closing = raw.starts_with('/');
        let name = tag_name(raw);
        if name.is_empty() {
            continue;
        }
        if md.pre {
            if closing && name == "pre" {
                md.pre = false;
                if !md.out.ends_with('\n') {
                    md.out.push('\n');
                }
                md.out.push_str("```\n");
                md.block();
            }
            continue;
        }
        if !closing && SKIP.contains(&name.as_str()) {
            let close = format!("</{}", name);
            let mut search = i;
            let mut depth = 1usize;
            loop {
                match html[search..].find('<') {
                    Some(p) => {
                        let rest = &html[search + p..];
                        if rest.len() > close.len()
                            && rest[1..].to_lowercase().starts_with(&close[1..])
                        {
                            depth -= 1;
                        } else if rest[1..].to_lowercase().starts_with(&format!("{} ", name))
                            || rest[1..].to_lowercase().starts_with(&format!("{}>", name))
                        {
                            depth += 1;
                        }
                        search += p + 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    None => {
                        search = html.len();
                        break;
                    }
                }
            }
            i = search;
            continue;
        }
        if closing {
            match name.as_str() {
                "a" => {
                    if let Some((href, buf)) = md.link.take() {
                        let buf = buf.trim();
                        if href.is_empty() || href.starts_with('#') || buf.is_empty() {
                            md.out.push_str(buf);
                        } else {
                            md.out.push_str(&format!("[{buf}]({href})"));
                        }
                    }
                }
                "b" | "strong" => mark(&mut md, "**"),
                "i" | "em" => mark(&mut md, "*"),
                "code" | "kbd" | "samp" => mark(&mut md, "`"),
                "li" | "dt" | "dd" => {
                    if !md.out.ends_with('\n') {
                        md.out.push('\n');
                    }
                }
                _ if BLOCK.contains(&name.as_str()) => md.block(),
                _ => {}
            }
            continue;
        }
        let attrs = &raw[name.len()..];
        match name.as_str() {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let lvl = name[1..].parse::<usize>().unwrap_or(1);
                md.block();
                md.out.push_str(&"#".repeat(lvl));
                md.out.push(' ');
            }
            "br" => md.out.push_str("  \n"),
            "hr" => {
                md.block();
                md.out.push_str("---\n\n");
            }
            "li" | "dt" | "dd" => {
                md.block();
                md.out.push_str("- ");
            }
            "pre" => {
                md.block();
                md.out.push_str("```\n");
                md.pre = true;
            }
            "code" | "kbd" | "samp" => mark(&mut md, "`"),
            "b" | "strong" => mark(&mut md, "**"),
            "i" | "em" => mark(&mut md, "*"),
            "a" => {
                md.link = Some((entities(&attr(attrs, "href")), String::new()));
            }
            "img" => {
                let src = entities(&attr(attrs, "src"));
                let alt = entities(&attr(attrs, "alt"));
                if !src.is_empty() {
                    md.out.push_str(&format!("![{alt}]({src})"));
                }
            }
            _ if BLOCK.contains(&name.as_str()) => md.block(),
            _ => {}
        }
    }
    let out = md
        .out
        .lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = out.trim().to_string();
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_parse() {
        let html = r##"<div class="result">
<a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fdocs&amp;rut=abc">Rust <b>Book</b> &amp; Guide</a>
<a class="result__snippet" href="#">The <b>best</b> guide to rust</a>
<a rel="nofollow" class="result__a" href="https://direct.example.org">Direct link</a>
<a class="result__snippet">second snippet</a>
</div>"##;
        let hits = parse_results(html, 8);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "https://example.com/docs");
        assert_eq!(hits[0].title, "Rust Book & Guide");
        assert_eq!(hits[0].snippet, "The best guide to rust");
        assert_eq!(hits[1].url, "https://direct.example.org");
        assert_eq!(hits[1].snippet, "second snippet");
        assert!(parse_results("<p>nothing</p>", 8).is_empty());
    }

    #[test]
    fn url_tools() {
        assert_eq!(urlencode("a b&c"), "a%20b%26c");
        assert_eq!(urldecode("a%20b+c%26d"), "a b c&d");
        assert_eq!(
            resolve_ddg_href("//x/l/?uddg=https%3A//a.io&rut=1"),
            "https://a.io"
        );
    }

    #[test]
    fn html_basics() {
        let md = html_to_md(
            "<html><head><title>t</title><style>x{}</style></head>\
             <body><h1>Title</h1><p>hello <b>world</b> &amp; friends</p>\
             <ul><li>one</li><li>two</li></ul>\
             <p>a <a href=\"https://x.y\">link</a> end</p>\
             <script>alert(1)</script>\
             <pre><code>let x = 1;</code></pre>\
             <img src=\"p.png\" alt=\"pic\">\
             </body></html>",
        );
        assert!(md.contains("# Title"), "{md}");
        assert!(md.contains("hello **world** & friends"), "{md}");
        assert!(md.contains("- one"), "{md}");
        assert!(md.contains("- two"), "{md}");
        assert!(md.contains("[link](https://x.y)"), "{md}");
        assert!(!md.contains("alert"), "{md}");
        assert!(!md.contains("x{}"), "{md}");
        assert!(md.contains("```"), "{md}");
        assert!(md.contains("let x = 1;"), "{md}");
        assert!(md.contains("![pic](p.png)"), "{md}");
        assert!(!md.contains("<"), "{md}");
    }

    #[test]
    fn nested_links_and_entities() {
        let md =
            html_to_md("<p><a href='/a'><b>bold</b> link</a></p><p>x &#65; &copy; &mdash;</p>");
        assert!(md.contains("[**bold** link](/a)"), "{md}");
        assert!(md.contains("x A \u{a9} \u{2014}"), "{md}");
    }

    #[test]
    fn bare_link_keeps_text() {
        let md = html_to_md("<p><a href=\"#top\">top</a></p>");
        assert!(md.contains("top"), "{md}");
        assert!(!md.contains("[]"), "{md}");
    }

    #[test]
    fn url_check() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            assert!(fetch("ftp://x", 5).await.is_err());
            assert!(fetch("example.com", 5).await.is_err());
        });
    }
}
