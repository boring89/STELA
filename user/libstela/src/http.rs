// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! HTTP/1.0 GET with what a browser needs (P11, `docs/apps.md` §4): the
//! headers, redirects (301, 302, 303, 307, 308; at most 5), chunked
//! bodies and the content type; and joining a link to the page it is on.
//! No HTTPS (that needs TLS). The parsing is pure and tested on the host.
//!
//! ```text
//!  get(url) ──► net: connect, "GET path HTTP/1.0", read until closed
//!            ──► parse_head: status, headers, where the body starts
//!            ──► 3xx + Location → join(url, location), again
//!            ──► Transfer-Encoding: chunked → dechunk
//! ```

use crate::Error;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Most redirects followed.
pub const MAX_REDIRECTS: usize = 5;

/// A response.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Response {
    /// The status code (200...).
    pub status: u16,
    /// The headers, names lowered.
    pub headers: Vec<(String, String)>,
    /// The body (de-chunked).
    pub body: Vec<u8>,
    /// The URL it came from (after redirects).
    pub url: String,
}

impl Response {
    /// The value of header `name` (any case).
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Whether the body is HTML: the `Content-Type`, or (without one) a
    /// body that starts like HTML.
    pub fn is_html(&self) -> bool {
        match self.header("content-type") {
            Some(t) => t.to_ascii_lowercase().starts_with("text/html"),
            None => looks_like_html(&self.body),
        }
    }
}

/// Whether `body` starts (after blanks) with `<!doctype` or `<html`.
pub fn looks_like_html(body: &[u8]) -> bool {
    let start: Vec<u8> = body
        .iter()
        .skip_while(|b| b.is_ascii_whitespace())
        .take(9)
        .map(u8::to_ascii_lowercase)
        .collect();
    start.starts_with(b"<!doctype") || start.starts_with(b"<html")
}

/// A response's head: the status, the headers (names lowered) and where
/// the body starts.
pub type Head = (u16, Vec<(String, String)>, usize);

/// The head of a raw response.
pub fn parse_head(raw: &[u8]) -> Option<Head> {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = core::str::from_utf8(&raw[..end]).ok()?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next()?;
    let mut parts = status_line.split_whitespace();
    if !parts.next()?.starts_with("HTTP/1.") {
        return None;
    }
    let status = parts.next()?.parse().ok()?;
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    Some((status, headers, end + 4))
}

/// A chunked body (RFC 9112 §7.1) put together; `None` if it is broken.
pub fn dechunk(mut data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = data.windows(2).position(|w| w == b"\r\n")?;
        let size_text = core::str::from_utf8(&data[..line_end]).ok()?;
        let size_text = size_text.split(';').next()?.trim();
        let size = usize::from_str_radix(size_text, 16).ok()?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(data.get(..size)?);
        data = data.get(size + 2..)?;
    }
}

/// `link` as seen from the page at `base`: absolute URLs (`http://`,
/// `stela:`, `file:`, `Volume:/`) stay; `/path` starts at the host (or
/// the volume); anything else is next to `base`'s last `/`. `.` and `..`
/// are resolved; `#fragment`s are dropped.
pub fn join(base: &str, link: &str) -> String {
    let link = link.split('#').next().unwrap_or("").trim();
    if link.is_empty() {
        return base.split('#').next().unwrap_or(base).to_string();
    }
    if is_absolute(link) {
        return normalize(link);
    }
    // Where the path starts in `base`: after "http://host" or "Volume:".
    let path_at = if let Some(rest) = base.strip_prefix("http://") {
        7 + rest.find('/').unwrap_or(rest.len())
    } else if let Some(colon) = base.find(':') {
        colon + 1
    } else {
        0
    };
    let (origin, path) = base.split_at(path_at);
    let path = path.split(['?', '#']).next().unwrap_or("");
    let joined = if link.starts_with('/') {
        format!("{origin}{link}")
    } else {
        let dir = match path.rfind('/') {
            Some(i) => &path[..=i],
            None => "/",
        };
        format!("{origin}{dir}{link}")
    };
    normalize(&joined)
}

fn is_absolute(link: &str) -> bool {
    match link.find(':') {
        Some(i) => {
            let scheme = &link[..i];
            !scheme.is_empty() && !scheme.contains('/') && !scheme.contains('?')
        }
        None => false,
    }
}

/// Resolves `.` and `..` in the path part of `url`.
fn normalize(url: &str) -> String {
    let path_at = if let Some(rest) = url.strip_prefix("http://") {
        7 + rest.find('/').unwrap_or(rest.len())
    } else if let Some(colon) = url.find(':') {
        colon + 1
    } else {
        0
    };
    let (origin, rest) = url.split_at(path_at);
    let (path, query) = match rest.find('?') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    if !path.starts_with('/') {
        return url.to_string();
    }
    let mut parts: Vec<&str> = Vec::new();
    for p in path.split('/').skip(1) {
        match p {
            "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    // A trailing "." or ".." leaves a folder.
    if path.ends_with("/.") || path.ends_with("/..") {
        parts.push("");
    }
    format!("{origin}/{}{query}", parts.join("/"))
}

/// `GET url`, following redirects; the body de-chunked. Only `http://`.
pub fn get(url: &str, timeout_ns: u64) -> Result<Response, Error> {
    let mut url = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let raw = crate::net::http_get_raw(&url, timeout_ns)?;
        let (status, headers, at) = parse_head(&raw).ok_or(Error::IoError)?;
        let mut r = Response {
            status,
            headers,
            body: Vec::new(),
            url: url.clone(),
        };
        if matches!(status, 301 | 302 | 303 | 307 | 308)
            && let Some(to) = r.header("location")
        {
            url = join(&url, to);
            continue;
        }
        let body = &raw[at..];
        r.body = if r
            .header("transfer-encoding")
            .is_some_and(|t| t.eq_ignore_ascii_case("chunked"))
        {
            dechunk(body).ok_or(Error::IoError)?
        } else {
            body.to_vec()
        };
        return Ok(r);
    }
    Err(Error::IoError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heads() {
        let raw = b"HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Type: text/html; charset=utf-8\r\n\r\nbody";
        let (status, headers, at) = parse_head(raw).expect("a head");
        assert_eq!(status, 302);
        assert_eq!(&raw[at..], b"body");
        let r = Response {
            status,
            headers,
            ..Response::default()
        };
        assert_eq!(r.header("LOCATION"), Some("/next"));
        assert!(r.is_html());
        assert!(parse_head(b"garbage\r\n\r\n").is_none());
        assert!(parse_head(b"HTTP/1.0 200 OK\r\n").is_none());
    }

    #[test]
    fn chunks() {
        assert_eq!(
            dechunk(b"5\r\nhello\r\n7;x=1\r\n, world\r\n0\r\n\r\n").as_deref(),
            Some(&b"hello, world"[..])
        );
        assert!(dechunk(b"5\r\nhel").is_none());
        assert!(dechunk(b"zz\r\n").is_none());
    }

    #[test]
    fn html_sniffing() {
        let mut r = Response {
            body: b"  <!DOCTYPE html><p>".to_vec(),
            ..Response::default()
        };
        assert!(r.is_html());
        r.body = b"plain".to_vec();
        assert!(!r.is_html());
        r.headers = alloc::vec![(String::from("content-type"), String::from("text/plain"))];
        r.body = b"<html>".to_vec();
        assert!(!r.is_html());
    }

    #[test]
    fn links() {
        let base = "http://10.0.2.2:8642/docs/index.html";
        assert_eq!(
            join(base, "page2.html"),
            "http://10.0.2.2:8642/docs/page2.html"
        );
        assert_eq!(join(base, "/top.html"), "http://10.0.2.2:8642/top.html");
        assert_eq!(join(base, "../up.html#x"), "http://10.0.2.2:8642/up.html");
        assert_eq!(
            join(base, "./a/./b.html"),
            "http://10.0.2.2:8642/docs/a/b.html"
        );
        assert_eq!(join(base, "http://example.com/"), "http://example.com/");
        assert_eq!(join("http://h", "x"), "http://h/x");
        assert_eq!(join("Data:/web/a.html", "b.html"), "Data:/web/b.html");
        assert_eq!(join("Data:/web/a.html", "/c.html"), "Data:/c.html");
        assert_eq!(join("stela:home", "Data:/x.txt"), "Data:/x.txt");
        assert_eq!(join(base, "?q=1"), "http://10.0.2.2:8642/docs/?q=1");
        assert_eq!(join(base, ""), base);
    }
}
