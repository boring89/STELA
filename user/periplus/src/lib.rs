// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Periplus's pages (P11, `docs/apps.md` §4): a forgiving HTML reader
//! for a subset, and the layout of a page in rows of a monospaced font.
//! Pure logic, tested on the host.
//!
//! ```text
//!  "<h1>Hi</h1><p>a <a href=x>link</a>"
//!    ──tokens──► Start(h1) Text(Hi) End(h1) Start(p) Text(a ) Start(a, href=x) ...
//!    ──parse───► Page { title, blocks: [Heading 1 "Hi", Para "a " + link 0], links: ["x"] }
//!    ──layout(cols)──► rows of spans (column, text, bold, link)
//! ```
//!
//! Understood: `title`; `h1`–`h6`, `p`, `div` and the like, `br`, `hr`,
//! `pre`, `blockquote`, `ul`/`ol`/`li`, `table`/`tr`/`td`/`th` (cells by
//! tab stops), `b`/`strong`, `i`/`em` (shown as is), `a href`, `img alt`
//! (`[alt]`); `script` and `style` are skipped; entities `&amp; &lt; &gt;
//! &quot; &apos; &nbsp; &#N; &#xN;`. Anything else is left out and its
//! text kept.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// A piece of the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    /// `<name attr=value ...>` (name lowered; `/>` too).
    Start(String, Vec<(String, String)>),
    /// `</name>`.
    End(String),
    /// Text, entities decoded.
    Text(String),
}

/// The character of entity `name` (without `&` and `;`).
fn entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "copy" => '©',
        "mdash" => '—',
        "ndash" => '–',
        _ => {
            let n = name.strip_prefix('#')?;
            let code = match n.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => n.parse().ok()?,
            };
            char::from_u32(code)?
        }
    })
}

/// `text` with its entities decoded (unknown ones stay as they are).
pub fn decode(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find(';').filter(|&e| e <= 10) {
            Some(e) if entity(&after[..e]).is_some() => {
                out.push(entity(&after[..e]).unwrap_or('&'));
                rest = &after[e + 1..];
            }
            _ => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn attributes(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        let name_end = rest
            .find(|c: char| c == '=' || c.is_whitespace())
            .unwrap_or(rest.len());
        let name = rest[..name_end].trim_end_matches('/').to_ascii_lowercase();
        rest = rest[name_end..].trim_start();
        let mut value = String::new();
        if let Some(r) = rest.strip_prefix('=') {
            let r = r.trim_start();
            let (v, after) = match r.chars().next() {
                Some(q @ ('"' | '\'')) => {
                    let body = &r[1..];
                    let end = body.find(q).unwrap_or(body.len());
                    (&body[..end], body.get(end + 1..).unwrap_or(""))
                }
                _ => {
                    let end = r.find(char::is_whitespace).unwrap_or(r.len());
                    (r[..end].trim_end_matches('/'), &r[end..])
                }
            };
            value = decode(v);
            rest = after.trim_start();
        }
        if !name.is_empty() {
            out.push((name, value));
        }
    }
    out
}

/// The tokens of `html`. Comments, `<!...>` and `<?...>` are dropped; the
/// text of `script` and `style` too.
pub fn tokens(html: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut rest = html;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            out.push(Token::Text(decode(rest)));
            break;
        };
        if lt > 0 {
            out.push(Token::Text(decode(&rest[..lt])));
        }
        rest = &rest[lt..];
        if rest.starts_with("<!--") {
            rest = rest.find("-->").map_or("", |e| &rest[e + 3..]);
            continue;
        }
        let Some(gt) = rest.find('>') else {
            out.push(Token::Text(decode(rest)));
            break;
        };
        let inner = &rest[1..gt];
        rest = &rest[gt + 1..];
        if inner.starts_with('!') || inner.starts_with('?') {
            continue;
        }
        if let Some(name) = inner.strip_prefix('/') {
            out.push(Token::End(name.trim().to_ascii_lowercase()));
            continue;
        }
        let name_end = inner
            .find(|c: char| c.is_whitespace() || c == '/')
            .unwrap_or(inner.len());
        let name = inner[..name_end].to_ascii_lowercase();
        if name.is_empty() {
            out.push(Token::Text(String::from("<")));
            continue;
        }
        let attrs = attributes(&inner[name_end..]);
        if name == "script" || name == "style" {
            let close = alloc::format!("</{name}");
            let lower = rest.to_ascii_lowercase();
            rest = match lower.find(&close) {
                Some(e) => rest[e..].find('>').map_or("", |g| &rest[e + g + 1..]),
                None => "",
            };
            continue;
        }
        out.push(Token::Start(name, attrs));
    }
    out
}

/// What a block is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Running text.
    Para,
    /// A heading, 1 to 6.
    Heading(u8),
    /// Preformatted: lines kept, no wrapping.
    Pre,
    /// A list item with its marker (`*` or `3.`).
    Item(String),
    /// A horizontal rule.
    Rule,
}

/// A piece of text with one style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// The text.
    pub text: String,
    /// Bold (`b`, `strong`, headings).
    pub bold: bool,
    /// The link it belongs to (an index into `Page::links`).
    pub link: Option<usize>,
}

/// A block: its kind, indentation (in columns) and text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// What it is.
    pub kind: Kind,
    /// Columns of indentation (lists, quotes).
    pub indent: usize,
    /// Its text.
    pub runs: Vec<Run>,
}

/// A page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Page {
    /// The `title`.
    pub title: String,
    /// The blocks in order.
    pub blocks: Vec<Block>,
    /// The links' targets as written (`href`).
    pub links: Vec<String>,
}

/// What a list is: ordered with its next number, or not.
enum List {
    Bullets,
    Numbers(usize),
}

struct Builder {
    page: Page,
    block: Option<Block>,
    bold: usize,
    link: Option<usize>,
    pre: usize,
    lists: Vec<List>,
    quote: usize,
    in_title: bool,
    cells: usize,
}

impl Builder {
    fn indent(&self) -> usize {
        2 * (self.lists.len() + self.quote)
    }

    fn finish(&mut self) {
        if let Some(mut b) = self.block.take() {
            if b.kind != Kind::Pre {
                // Collapse blanks at the ends.
                if let Some(first) = b.runs.first_mut() {
                    first.text = first.text.trim_start().to_string();
                }
                if let Some(last) = b.runs.last_mut() {
                    last.text = last.text.trim_end().to_string();
                }
                b.runs.retain(|r| !r.text.is_empty());
            }
            if !b.runs.is_empty() || matches!(b.kind, Kind::Rule | Kind::Item(_)) {
                self.page.blocks.push(b);
            }
        }
    }

    fn start(&mut self, kind: Kind) {
        self.finish();
        self.block = Some(Block {
            kind,
            indent: self.indent(),
            runs: Vec::new(),
        });
    }

    /// A cell boundary: a tab, kept as it is.
    fn tab(&mut self) {
        if self.block.is_none() {
            self.start(Kind::Para);
        }
        if let Some(b) = self.block.as_mut() {
            if let Some(last) = b.runs.last_mut() {
                let kept = last.text.trim_end().len();
                last.text.truncate(kept);
            }
            b.runs.push(Run {
                text: String::from("\t"),
                bold: false,
                link: None,
            });
        }
    }

    fn text(&mut self, text: &str) {
        if self.in_title {
            self.page.title.push_str(text);
            return;
        }
        let text = if self.pre > 0 {
            text.to_string()
        } else {
            // Runs of blanks become one space.
            let mut s = String::new();
            let mut blank = false;
            for c in text.chars() {
                if c.is_whitespace() {
                    if !blank {
                        s.push(' ');
                    }
                    blank = true;
                } else {
                    s.push(c);
                    blank = false;
                }
            }
            s
        };
        if text.is_empty() {
            return;
        }
        if self.block.is_none() {
            if text.trim().is_empty() {
                return;
            }
            self.start(if self.pre > 0 { Kind::Pre } else { Kind::Para });
        }
        let (bold, link) = (self.bold > 0, self.link);
        let Some(b) = self.block.as_mut() else {
            return;
        };
        let heading = matches!(b.kind, Kind::Heading(_));
        // No double space between runs.
        let text = if b.kind != Kind::Pre
            && text.starts_with(' ')
            && b.runs.last().is_none_or(|r| r.text.ends_with(' '))
        {
            text[1..].to_string()
        } else {
            text
        };
        match b.runs.last_mut() {
            Some(r) if r.bold == (bold || heading) && r.link == link => r.text.push_str(&text),
            _ => b.runs.push(Run {
                text,
                bold: bold || heading,
                link,
            }),
        }
    }
}

/// The page `html` describes.
pub fn parse(html: &str) -> Page {
    let mut b = Builder {
        page: Page::default(),
        block: None,
        bold: 0,
        link: None,
        pre: 0,
        lists: Vec::new(),
        quote: 0,
        in_title: false,
        cells: 0,
    };
    for t in tokens(html) {
        match t {
            Token::Text(s) => b.text(&s),
            Token::Start(name, attrs) => match name.as_str() {
                "title" => b.in_title = true,
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    b.start(Kind::Heading(name.as_bytes()[1] - b'0'));
                }
                "p" | "div" | "section" | "article" | "header" | "footer" | "nav" | "main"
                | "address" | "figure" | "dl" | "dt" | "dd" => {
                    b.start(Kind::Para);
                }
                "blockquote" => {
                    b.quote += 1;
                    b.start(Kind::Para);
                }
                "pre" => {
                    b.pre += 1;
                    b.start(Kind::Pre);
                }
                "br" => {
                    let kind = b.block.as_ref().map_or(Kind::Para, |bl| match bl.kind {
                        Kind::Pre => Kind::Pre,
                        Kind::Heading(n) => Kind::Heading(n),
                        _ => Kind::Para,
                    });
                    if b.pre > 0 {
                        b.text("\n");
                    } else {
                        b.start(kind);
                    }
                }
                "hr" => {
                    b.start(Kind::Rule);
                    b.finish();
                }
                "ul" => {
                    b.finish();
                    b.lists.push(List::Bullets);
                }
                "ol" => {
                    b.finish();
                    b.lists.push(List::Numbers(1));
                }
                "li" => {
                    let marker = match b.lists.last_mut() {
                        Some(List::Numbers(n)) => {
                            let m = alloc::format!("{n}.");
                            *n += 1;
                            m
                        }
                        _ => String::from("*"),
                    };
                    b.finish();
                    b.block = Some(Block {
                        kind: Kind::Item(marker),
                        indent: 2 * (b.lists.len().max(1) - 1 + b.quote),
                        runs: Vec::new(),
                    });
                }
                "tr" => {
                    b.start(Kind::Para);
                    b.cells = 0;
                }
                "td" | "th" => {
                    if b.cells > 0 {
                        b.tab();
                    }
                    b.cells += 1;
                    if name == "th" {
                        b.bold += 1;
                    }
                }
                "b" | "strong" => b.bold += 1,
                "a" => {
                    if let Some((_, href)) = attrs.iter().find(|(n, _)| n == "href") {
                        b.page.links.push(href.clone());
                        b.link = Some(b.page.links.len() - 1);
                    }
                }
                "img" => {
                    let alt = attrs
                        .iter()
                        .find(|(n, _)| n == "alt")
                        .map_or("image", |(_, v)| v.as_str());
                    let text = alloc::format!("[{alt}]");
                    b.text(&text);
                }
                _ => {}
            },
            Token::End(name) => match name.as_str() {
                "title" => b.in_title = false,
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "div" | "section" | "article"
                | "header" | "footer" | "nav" | "main" | "li" | "tr" | "table" | "address"
                | "figure" | "dl" | "dt" | "dd" => b.finish(),
                "blockquote" => {
                    b.finish();
                    b.quote = b.quote.saturating_sub(1);
                }
                "pre" => {
                    b.finish();
                    b.pre = b.pre.saturating_sub(1);
                }
                "ul" | "ol" => {
                    b.finish();
                    b.lists.pop();
                }
                "th" => b.bold = b.bold.saturating_sub(1),
                "b" | "strong" => b.bold = b.bold.saturating_sub(1),
                "a" => b.link = None,
                _ => {}
            },
        }
    }
    b.finish();
    b.page.title = b
        .page
        .title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    b.page
}

/// A plain text document as a page: one preformatted block.
pub fn plain(text: &str) -> Page {
    Page {
        title: String::new(),
        blocks: alloc::vec![Block {
            kind: Kind::Pre,
            indent: 0,
            runs: alloc::vec![Run {
                text: text.to_string(),
                bold: false,
                link: None,
            }],
        }],
        links: Vec::new(),
    }
}

/// A piece of a laid-out row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    /// The column it starts in.
    pub col: usize,
    /// Its text (no tabs, no newlines).
    pub text: String,
    /// Bold.
    pub bold: bool,
    /// Its link.
    pub link: Option<usize>,
}

/// A row: its spans, left to right.
pub type Row = Vec<Span>;

/// Columns between tab stops (table cells).
pub const TAB: usize = 16;

/// Columns `text` takes (a CJK character two, P11 K39).
pub fn cells(text: &str) -> usize {
    toolbox::cjk::width(text)
}

/// The longest start of `text` that fits in `n` columns.
fn take_cells(text: &str, n: usize) -> String {
    let mut used = 0;
    let mut out = String::new();
    for c in text.chars() {
        let w = toolbox::cjk::cells(c);
        if used + w > n {
            break;
        }
        used += w;
        out.push(c);
    }
    out
}

struct Rows {
    rows: Vec<Row>,
    row: Row,
    col: usize,
}

impl Rows {
    fn push(&mut self, text: &str, bold: bool, link: Option<usize>) {
        if text.is_empty() {
            return;
        }
        match self.row.last_mut() {
            Some(s) if s.bold == bold && s.link == link && s.col + cells(&s.text) == self.col => {
                s.text.push_str(text);
            }
            _ => self.row.push(Span {
                col: self.col,
                text: text.to_string(),
                bold,
                link,
            }),
        }
        self.col += cells(text);
    }

    fn newline(&mut self) {
        if let Some(last) = self.row.last_mut() {
            let kept = last.text.trim_end().len();
            last.text.truncate(kept);
        }
        self.row.retain(|s| !s.text.is_empty());
        self.rows.push(core::mem::take(&mut self.row));
        self.col = 0;
    }
}

/// The page laid out `cols` wide: a blank row between blocks, words
/// wrapped (a word wider than a row is cut), `pre` kept as it is (cut at
/// the edge), tabs to the next stop.
pub fn layout(page: &Page, cols: usize) -> Vec<Row> {
    let cols = cols.max(8);
    let mut r = Rows {
        rows: Vec::new(),
        row: Vec::new(),
        col: 0,
    };
    for (i, block) in page.blocks.iter().enumerate() {
        let tight = matches!(block.kind, Kind::Item(_))
            && i > 0
            && matches!(page.blocks[i - 1].kind, Kind::Item(_));
        if i > 0 && !tight {
            r.newline();
        }
        let start = block.indent.min(cols / 2);
        r.col = start;
        let mut left = start;
        match &block.kind {
            Kind::Rule => {
                let line: String = core::iter::repeat_n('-', cols - start).collect();
                r.push(&line, false, None);
                r.newline();
                continue;
            }
            Kind::Item(marker) => {
                r.push(marker, false, None);
                r.col += 1;
                left = start + marker.chars().count() + 1;
            }
            _ => {}
        }
        if block.kind == Kind::Pre {
            for run in &block.runs {
                let mut first = true;
                for line in run.text.split('\n') {
                    if !first {
                        r.newline();
                        r.col = left;
                    }
                    first = false;
                    let room = cols.saturating_sub(r.col);
                    let text = take_cells(&line.replace('\t', "    "), room);
                    r.push(&text, run.bold, run.link);
                }
            }
            r.newline();
            continue;
        }
        for run in &block.runs {
            // Words with the blank after them; tabs are their own pieces.
            let mut word = String::new();
            let mut pieces: Vec<String> = Vec::new();
            for c in run.text.chars() {
                if c == '\t' {
                    if !word.is_empty() {
                        pieces.push(core::mem::take(&mut word));
                    }
                    pieces.push(String::from("\t"));
                } else {
                    word.push(c);
                    if c == ' ' {
                        pieces.push(core::mem::take(&mut word));
                    }
                }
            }
            if !word.is_empty() {
                pieces.push(word);
            }
            for piece in pieces {
                if piece == "\t" {
                    let next = (r.col / TAB + 1) * TAB;
                    if next >= cols {
                        r.newline();
                        r.col = left;
                    } else {
                        r.col = next;
                    }
                    continue;
                }
                let width = cells(piece.trim_end());
                if r.col + width > cols && r.col > left {
                    r.newline();
                    r.col = left;
                }
                let mut piece = piece.as_str();
                if r.col == left {
                    piece = piece.trim_start();
                }
                // Too wide for any row: cut.
                let mut rest = String::from(piece);
                while cells(rest.trim_end()) > cols - left {
                    let cut = take_cells(&rest, cols - r.col);
                    if cut.is_empty() {
                        break;
                    }
                    r.push(&cut, run.bold, run.link);
                    rest = String::from(&rest[cut.len()..]);
                    r.newline();
                    r.col = left;
                }
                r.push(&rest, run.bold, run.link);
            }
        }
        r.newline();
    }
    if !r.row.is_empty() {
        r.newline();
    }
    r.rows
}

/// The link under column `col` of `row`, if any.
pub fn link_at(row: &Row, col: usize) -> Option<usize> {
    row.iter()
        .find(|s| s.col <= col && col < s.col + cells(&s.text))
        .and_then(|s| s.link)
}

/// The first row where link `link` shows.
pub fn row_of_link(rows: &[Row], link: usize) -> Option<usize> {
    rows.iter()
        .position(|row| row.iter().any(|s| s.link == Some(link)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_and_tokens() {
        assert_eq!(
            decode("a &amp; b &lt;c&gt; &#65;&#x42; &bogus; &"),
            "a & b <c> AB &bogus; &"
        );
        let t = tokens(
            "<!DOCTYPE html><P CLASS='x'>Hi<!-- no --><br/><a href=\"p2.html\">there</a></p>",
        );
        assert_eq!(
            t,
            [
                Token::Start("p".into(), alloc::vec![("class".into(), "x".into())]),
                Token::Text("Hi".into()),
                Token::Start("br".into(), alloc::vec![]),
                Token::Start("a".into(), alloc::vec![("href".into(), "p2.html".into())]),
                Token::Text("there".into()),
                Token::End("a".into()),
                Token::End("p".into()),
            ]
        );
        assert_eq!(
            tokens("a<script>if (a < b) {}</script><style>p{}</style>b"),
            [Token::Text("a".into()), Token::Text("b".into())]
        );
        let t: String = tokens("x < y")
            .into_iter()
            .map(|t| match t {
                Token::Text(s) => s,
                _ => String::new(),
            })
            .collect();
        assert_eq!(t, "x < y");
    }

    #[test]
    fn a_page() {
        let p = parse(
            "<html><head><title> My  page </title></head><body>\
             <h1>Hello</h1><p>Some <b>bold</b> and <a href=\"two.html\">a link</a>.</p>\
             <ul><li>one</li><li>two<ol><li>deep</li></ol></li></ul>\
             <pre>  keep\n   this</pre><hr><img alt=\"logo\">\
             <table><tr><th>A</th><td>B</td></tr></table></body></html>",
        );
        assert_eq!(p.title, "My page");
        assert_eq!(p.links, ["two.html"]);
        assert_eq!(p.blocks[0].kind, Kind::Heading(1));
        assert!(p.blocks[0].runs[0].bold);
        let para = &p.blocks[1];
        let texts: Vec<_> = para
            .runs
            .iter()
            .map(|r| (r.text.as_str(), r.bold, r.link))
            .collect();
        assert_eq!(
            texts,
            [
                ("Some ", false, None),
                ("bold", true, None),
                (" and ", false, None),
                ("a link", false, Some(0)),
                (".", false, None)
            ]
        );
        assert_eq!(p.blocks[2].kind, Kind::Item("*".into()));
        assert_eq!(p.blocks[3].kind, Kind::Item("*".into()));
        assert_eq!(p.blocks[4].kind, Kind::Item("1.".into()));
        assert_eq!(p.blocks[4].indent, 2);
        assert_eq!(p.blocks[5].kind, Kind::Pre);
        assert_eq!(p.blocks[5].runs[0].text, "  keep\n   this");
        assert_eq!(p.blocks[6].kind, Kind::Rule);
        assert_eq!(p.blocks[7].runs[0].text, "[logo]");
        let cells: String = p.blocks[8].runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(cells, "A\tB");
    }

    #[test]
    fn layout_wraps_and_links() {
        let p =
            parse("<p>The quick brown fox jumps over the <a href=x>lazy dog</a>.</p><p>end</p>");
        let rows = layout(&p, 16);
        let text = |row: &Row| -> String {
            let mut s = String::new();
            for sp in row {
                while s.chars().count() < sp.col {
                    s.push(' ');
                }
                s.push_str(&sp.text);
            }
            s
        };
        let lines: Vec<String> = rows.iter().map(text).collect();
        assert_eq!(
            lines,
            [
                "The quick brown",
                "fox jumps over",
                "the lazy dog.",
                "",
                "end"
            ]
        );
        assert_eq!(link_at(&rows[2], 5), Some(0));
        assert_eq!(link_at(&rows[2], 1), None);
        assert_eq!(row_of_link(&rows, 0), Some(2));
    }

    #[test]
    fn layout_lists_pre_tabs() {
        let p = parse(
            "<ul><li>a</li><li>b</li></ul><pre>x\ty\nlong long long line</pre><table><tr><td>1</td><td>2</td></tr></table>",
        );
        let rows = layout(&p, 12);
        assert_eq!(rows[0][0].text, "*");
        assert_eq!((rows[0][1].col, rows[0][1].text.as_str()), (2, "a"));
        assert_eq!(rows[1][1].text, "b");
        // The pre block: tabs as spaces, cut at the edge.
        assert_eq!(rows[3][0].text, "x    y");
        assert_eq!(rows[4][0].text, "long long lo");
        // Cells at the tab stops (16 > 12: the next cell wraps).
        assert_eq!(rows[6][0].text, "1");
        assert_eq!(rows[7][0].text, "2");
        // Plain text.
        let rows = layout(&plain("a\nb"), 10);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn long_words_are_cut() {
        let rows = layout(&parse("<p>abcdefghijklmnopqrstuvwxyz</p>"), 10);
        let texts: Vec<_> = rows.iter().map(|r| r[0].text.clone()).collect();
        assert_eq!(texts, ["abcdefghij", "klmnopqrst", "uvwxyz"]);
    }
}
