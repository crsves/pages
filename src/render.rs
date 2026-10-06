//! Lays the block model out as wrapped, ANSI-styled terminal text.

use crate::doc::{Block, Document, ListLabel, ParaKind, Span, Style};
use crate::table::Table;
use unicode_width::UnicodeWidthChar;

const MARGIN: usize = 2;

// 256-colour palette
const ACCENT: u8 = 212;
const H2: u8 = 75;
const H3: u8 = 114;
const MUTED: u8 = 245;
// Mid grey: visible on light, dark, and translucent backgrounds alike.
const FAINT: u8 = 244;
const LINK: u8 = 75;
const QUOTE: u8 = 141;

pub struct Options {
    pub width: usize,
    pub color: bool,
    /// Emit OSC 8 hyperlinks; otherwise URLs are printed after the link text.
    pub hyperlinks: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Look {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    dim: bool,
    fg: Option<u8>,
    link: Option<String>,
}

impl Look {
    fn fg(c: u8) -> Look {
        Look { fg: Some(c), ..Look::default() }
    }

    fn from_style(s: &Style) -> Look {
        Look {
            bold: s.bold,
            italic: s.italic,
            underline: s.underline || s.link.is_some(),
            strike: s.strike,
            dim: false,
            fg: s.link.as_ref().map(|_| LINK),
            link: s.link.clone(),
        }
    }
}

type Run = (String, Look);

pub fn render(doc: &Document, opts: &Options) -> String {
    let r = Renderer { opts, width: opts.width.saturating_sub(MARGIN * 2).max(20) };
    let mut lines: Vec<String> = vec![String::new()];
    let mut prev: Option<&Block> = None;
    for block in &doc.blocks {
        if let Some(p) = prev {
            if !tight(p, block) {
                lines.push(String::new());
            }
        }
        lines.extend(r.block(block, r.width));
        prev = Some(block);
    }
    if !doc.footnotes.is_empty() {
        lines.push(String::new());
        lines.push(r.paint(&[("─".repeat(12), Look::fg(FAINT))]));
        for (i, note) in doc.footnotes.iter().enumerate() {
            let label = format!("[{}] ", i + 1);
            let runs = r.runs(note, &Look { dim: true, ..Look::default() });
            lines.extend(r.wrap_with_prefix(&runs, r.width, (label.clone(), Look::fg(MUTED)), " ".repeat(label.len())));
        }
    }
    lines.push(String::new());
    let pad = " ".repeat(MARGIN);
    let mut out = String::new();
    for l in lines {
        if !l.is_empty() {
            out.push_str(&pad);
        }
        out.push_str(&l);
        out.push('\n');
    }
    out
}

/// Consecutive list items and a title followed by its subtitle sit together.
fn tight(a: &Block, b: &Block) -> bool {
    matches!(
        (a, b),
        (Block::Para { list: Some(_), .. }, Block::Para { list: Some(_), .. })
            | (Block::Para { kind: ParaKind::Title, .. }, Block::Para { kind: ParaKind::Subtitle, .. })
    )
}

struct Renderer<'a> {
    opts: &'a Options,
    width: usize,
}

impl Renderer<'_> {
    fn block(&self, block: &Block, width: usize) -> Vec<String> {
        match block {
            Block::Para { kind, list, spans } => self.para(*kind, list.as_ref(), spans, width),
            Block::Table(t) => self.table(t, width),
            Block::Placeholder(what) => {
                let look = Look { dim: true, italic: true, ..Look::default() };
                self.wrap_with_prefix(&[(what.clone(), look.clone())], width, ("▣ ".into(), look), "  ".into())
            }
            Block::Aside(inner) => {
                let mut lines = Vec::new();
                let mut prev: Option<&Block> = None;
                for b in inner {
                    if prev.is_some_and(|p| !tight(p, b)) {
                        lines.push(String::new());
                    }
                    lines.extend(self.block(b, width.saturating_sub(2).max(10)));
                    prev = Some(b);
                }
                let bar = self.paint(&[("▎".into(), Look::fg(FAINT))]);
                lines.into_iter().map(|l| if l.is_empty() { bar.clone() } else { format!("{bar} {l}") }).collect()
            }
        }
    }

    fn para(&self, kind: ParaKind, list: Option<&ListLabel>, spans: &[Span], width: usize) -> Vec<String> {
        let base = match kind {
            ParaKind::Title => Look { bold: true, ..Look::fg(ACCENT) },
            ParaKind::Subtitle => Look { italic: true, ..Look::fg(MUTED) },
            ParaKind::Heading(1) => Look { bold: true, ..Look::fg(ACCENT) },
            ParaKind::Heading(2) => Look { bold: true, ..Look::fg(H2) },
            ParaKind::Heading(3) => Look { bold: true, ..Look::fg(H3) },
            ParaKind::Heading(_) => Look { bold: true, ..Look::default() },
            ParaKind::Quote => Look { italic: true, ..Look::default() },
            ParaKind::Body => Look::default(),
        };
        let runs = self.runs(spans, &base);
        match (kind, list) {
            (_, Some(l)) => {
                let indent = " ".repeat(l.level * 3);
                let label = if l.label.is_empty() { String::new() } else { format!("{} ", l.label) };
                let first = format!("{indent}{label}");
                let rest = " ".repeat(str_width(&first));
                self.wrap_with_prefix(&runs, width, (first, Look::fg(ACCENT)), rest)
            }
            (ParaKind::Quote, None) => {
                let bar = "│ ".to_string();
                self.wrap_with_prefix(&runs, width, (bar.clone(), Look::fg(QUOTE)), String::new())
                    .into_iter()
                    .enumerate()
                    .map(
                        |(i, l)| {
                            if i == 0 {
                                l
                            } else {
                                format!("{}{l}", self.paint(&[(bar.clone(), Look::fg(QUOTE))]))
                            }
                        },
                    )
                    .collect()
            }
            (ParaKind::Title | ParaKind::Heading(1), None) => {
                let mut lines = self.wrap(&runs, width);
                let text_w = lines.iter().map(|l| runs_width(l)).max().unwrap_or(0);
                let (ch, look) =
                    if kind == ParaKind::Title { ("━", Look::fg(ACCENT)) } else { ("─", Look::fg(FAINT)) };
                let rule_w = if kind == ParaKind::Title { text_w } else { width };
                let mut out: Vec<String> = lines.drain(..).map(|l| self.paint(&l)).collect();
                out.push(self.paint(&[(ch.repeat(rule_w.max(1)), look)]));
                out
            }
            _ => self.wrap(&runs, width).iter().map(|l| self.paint(l)).collect(),
        }
    }

    fn runs(&self, spans: &[Span], base: &Look) -> Vec<Run> {
        let mut out = Vec::new();
        for s in spans {
            let own = Look::from_style(&s.style);
            let look = Look {
                bold: base.bold || own.bold,
                italic: base.italic || own.italic,
                underline: own.underline,
                strike: own.strike,
                dim: base.dim,
                fg: own.fg.or(base.fg),
                link: if self.opts.hyperlinks { own.link.clone() } else { None },
            };
            let text = match s.style.script {
                1 => to_script(&s.text, SUPERSCRIPT),
                2 => to_script(&s.text, SUBSCRIPT),
                _ => s.text.clone(),
            };
            out.push((text.clone(), look));
            if let Some(url) = &s.style.link {
                if !self.opts.hyperlinks && !same_link(url, &s.text) {
                    out.push((format!(" ({url})"), Look { dim: true, ..Look::default() }));
                }
            }
        }
        out
    }

    fn wrap_with_prefix(&self, runs: &[Run], width: usize, first: (String, Look), rest: String) -> Vec<String> {
        let pw = str_width(&first.0);
        let lines = self.wrap(runs, width.saturating_sub(pw).max(8));
        lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let prefix = if i == 0 { self.paint(std::slice::from_ref(&first)) } else { rest.clone() };
                format!("{prefix}{}", self.paint(l))
            })
            .collect()
    }

    /// Greedy word wrap over styled runs. Explicit newlines force breaks;
    /// words wider than the line are split by character.
    fn wrap(&self, runs: &[Run], width: usize) -> Vec<Vec<Run>> {
        let mut chars: Vec<(char, usize)> = Vec::new();
        for (i, (t, _)) in runs.iter().enumerate() {
            chars.extend(t.chars().map(|c| (c, i)));
        }
        let mut lines: Vec<Vec<(char, usize)>> = vec![Vec::new()];
        let mut cur = 0usize;
        let mut pending: Vec<(char, usize)> = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i].0;
            if c == '\n' {
                lines.push(Vec::new());
                cur = 0;
                pending.clear();
                i += 1;
                continue;
            }
            if c == ' ' || c == '\u{3000}' {
                let j = chars[i..]
                    .iter()
                    .position(|(c, _)| !(*c == ' ' || *c == '\u{3000}'))
                    .map_or(chars.len(), |p| i + p);
                if cur > 0 {
                    pending = chars[i..j].to_vec();
                }
                i = j;
                continue;
            }
            let j = chars[i..]
                .iter()
                .position(|(c, _)| matches!(c, ' ' | '\n' | '\u{3000}'))
                .map_or(chars.len(), |p| i + p);
            let word = &chars[i..j];
            let ww: usize = word.iter().map(|(c, _)| cw(*c)).sum();
            let pw: usize = pending.iter().map(|(c, _)| cw(*c)).sum();
            if cur > 0 && cur + pw + ww > width {
                lines.push(Vec::new());
                cur = 0;
            } else if cur > 0 {
                lines.last_mut().unwrap().extend(pending.iter().copied());
                cur += pw;
            }
            pending.clear();
            for &(c, r) in word {
                let w = cw(c);
                if cur + w > width && cur > 0 {
                    lines.push(Vec::new());
                    cur = 0;
                }
                lines.last_mut().unwrap().push((c, r));
                cur += w;
            }
            i = j;
        }
        lines
            .into_iter()
            .map(|l| {
                let mut out: Vec<Run> = Vec::new();
                for (c, r) in l {
                    match out.last_mut() {
                        Some((t, look)) if *look == runs[r].1 => t.push(c),
                        _ => out.push((c.to_string(), runs[r].1.clone())),
                    }
                }
                out
            })
            .collect()
    }

    fn paint(&self, runs: &[Run]) -> String {
        let mut s = String::new();
        for (text, look) in runs {
            if !self.opts.color || *look == Look::default() {
                s.push_str(text);
                continue;
            }
            let mut codes: Vec<String> = Vec::new();
            if look.bold {
                codes.push("1".into());
            }
            if look.dim {
                codes.push("2".into());
            }
            if look.italic {
                codes.push("3".into());
            }
            if look.underline {
                codes.push("4".into());
            }
            if look.strike {
                codes.push("9".into());
            }
            if let Some(c) = look.fg {
                codes.push(format!("38;5;{c}"));
            }
            if let Some(url) = &look.link {
                s.push_str(&format!("\x1b]8;;{url}\x1b\\"));
            }
            if !codes.is_empty() {
                s.push_str(&format!("\x1b[{}m{text}\x1b[0m", codes.join(";")));
            } else {
                s.push_str(text);
            }
            if look.link.is_some() {
                s.push_str("\x1b]8;;\x1b\\");
            }
        }
        s
    }

    fn table(&self, t: &Table, width: usize) -> Vec<String> {
        let ncols = t.rows.iter().map(Vec::len).max().unwrap_or(0);
        if ncols == 0 {
            return Vec::new();
        }
        let cells: Vec<Vec<Vec<Run>>> = t
            .rows
            .iter()
            .enumerate()
            .map(|(ri, row)| {
                (0..ncols)
                    .map(|ci| {
                        let base = if ri < t.header_rows {
                            Look { bold: true, ..Look::fg(ACCENT) }
                        } else if ci < t.header_cols {
                            Look { bold: true, ..Look::default() }
                        } else {
                            Look::default()
                        };
                        row.get(ci).map(|spans| self.runs(spans, &base)).unwrap_or_default()
                    })
                    .collect()
            })
            .collect();

        // Natural column widths, then shrink the widest until the table fits.
        let mut widths: Vec<usize> =
            (0..ncols).map(|c| cells.iter().map(|r| natural_width(&r[c])).max().unwrap_or(0).max(1)).collect();
        let avail = width.saturating_sub(3 * ncols + 1);
        let floor: Vec<usize> = widths.iter().map(|&w| w.min(8)).collect();
        while widths.iter().sum::<usize>() > avail {
            let (i, &w) = widths.iter().enumerate().max_by_key(|(_, w)| **w).unwrap();
            if w <= floor[i] {
                break;
            }
            widths[i] -= 1;
        }

        let rendered: Vec<Vec<Vec<Vec<Run>>>> =
            cells.iter().map(|r| r.iter().zip(&widths).map(|(c, &w)| self.wrap(c, w)).collect()).collect();
        let multiline = rendered.iter().any(|r| r.iter().any(|c| c.len() > 1));

        let border = Look::fg(FAINT);
        let rule = |l: &str, m: &str, r: &str| {
            let mut s = l.to_string();
            for (i, w) in widths.iter().enumerate() {
                s.push_str(&"─".repeat(w + 2));
                s.push_str(if i + 1 == ncols { r } else { m });
            }
            self.paint(&[(s, border.clone())])
        };
        let bar = self.paint(&[("│".into(), border.clone())]);

        let mut out = Vec::new();
        if let Some(name) = &t.name {
            out.push(self.paint(&[(name.clone(), Look { bold: true, ..Look::default() })]));
        }
        out.push(rule("╭", "┬", "╮"));
        for (ri, row) in rendered.iter().enumerate() {
            if ri > 0 && (multiline || ri == t.header_rows) {
                out.push(rule("├", "┼", "┤"));
            }
            let height = row.iter().map(Vec::len).max().unwrap_or(1).max(1);
            for li in 0..height {
                let mut line = bar.clone();
                for (ci, cell) in row.iter().enumerate() {
                    let content = cell.get(li).map(|l| (self.paint(l), runs_width(l))).unwrap_or_default();
                    line.push(' ');
                    line.push_str(&content.0);
                    line.push_str(&" ".repeat(widths[ci].saturating_sub(content.1) + 1));
                    line.push_str(&bar);
                }
                out.push(line);
            }
        }
        out.push(rule("╰", "┴", "╯"));
        if t.omitted_rows > 0 {
            let note = format!("… {} more rows", t.omitted_rows);
            out.push(self.paint(&[(note, Look { dim: true, italic: true, ..Look::default() })]));
        }
        out
    }
}

/// Whether link text already shows its URL, ignoring scheme, `www.`, and a
/// trailing slash (`github.com/x` vs `https://github.com/x/`).
fn same_link(url: &str, text: &str) -> bool {
    fn norm(s: &str) -> &str {
        let s = s.trim();
        let s = s.split_once("://").map_or(s, |(_, rest)| rest);
        let s = s.strip_prefix("mailto:").unwrap_or(s);
        let s = s.strip_prefix("www.").unwrap_or(s);
        s.strip_suffix('/').unwrap_or(s)
    }
    norm(url).eq_ignore_ascii_case(norm(text))
}

fn cw(c: char) -> usize {
    c.width().unwrap_or(0)
}

fn str_width(s: &str) -> usize {
    s.chars().map(cw).sum()
}

fn runs_width(runs: &[Run]) -> usize {
    runs.iter().map(|(t, _)| str_width(t)).sum()
}

fn natural_width(runs: &[Run]) -> usize {
    let text: String = runs.iter().map(|(t, _)| t.as_str()).collect();
    text.split('\n').map(str_width).max().unwrap_or(0)
}

const SUPERSCRIPT: (&str, &str) = ("0123456789+-=()n[]", "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ⁿ[]");
const SUBSCRIPT: (&str, &str) = ("0123456789+-=()", "₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎");

/// Map to Unicode super/subscript characters when every character has one.
fn to_script(s: &str, (from, to): (&str, &str)) -> String {
    let map: Vec<(char, char)> = from.chars().zip(to.chars()).collect();
    let mapped: Option<String> = s.chars().map(|c| map.iter().find(|(f, _)| *f == c).map(|(_, t)| *t)).collect();
    mapped.unwrap_or_else(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(width: usize) -> Options {
        Options { width, color: false, hyperlinks: false }
    }

    #[test]
    fn wraps_words() {
        let r = Renderer { opts: &plain(80), width: 10 };
        let runs = vec![("hello big wide world".to_string(), Look::default())];
        let lines: Vec<String> = r.wrap(&runs, 10).iter().map(|l| r.paint(l)).collect();
        assert_eq!(lines, ["hello big", "wide world"]);
    }

    #[test]
    fn splits_long_words_and_keeps_styles_together() {
        let r = Renderer { opts: &plain(80), width: 4 };
        let runs =
            vec![("abcdefgh".to_string(), Look::default()), (" x".to_string(), Look { bold: true, ..Look::default() })];
        let lines: Vec<String> = r.wrap(&runs, 4).iter().map(|l| r.paint(l)).collect();
        assert_eq!(lines, ["abcd", "efgh", "x"]);
    }

    #[test]
    fn wide_chars() {
        let r = Renderer { opts: &plain(80), width: 4 };
        let runs = vec![("日本語です".to_string(), Look::default())];
        let lines: Vec<String> = r.wrap(&runs, 4).iter().map(|l| r.paint(l)).collect();
        assert_eq!(lines, ["日本", "語で", "す"]);
    }

    #[test]
    fn link_text_matching() {
        assert!(same_link("https://github.com/crsves/pages", "github.com/crsves/pages"));
        assert!(same_link("https://www.example.com/", "example.com"));
        assert!(same_link("mailto:a@b.c", "a@b.c"));
        assert!(!same_link("https://example.com/docs", "the docs"));
    }

    #[test]
    fn scripts() {
        assert_eq!(to_script("2", SUBSCRIPT), "₂");
        assert_eq!(to_script("[3]", SUPERSCRIPT), "[³]");
        assert_eq!(to_script("th", SUPERSCRIPT), "th");
    }
}
