//! Turns the object graph of a Pages document into a simple block model.
//!
//! Text lives in `TSWP.StorageArchive` objects: one string plus a set of
//! attribute tables keyed by UTF-16 offset (paragraph style, character style,
//! list style, list level, attachments, links, ...).

use crate::iwa::Store;
use crate::proto::Msg;
use crate::table;
use std::collections::HashMap;

// Object type ids from the iWork type registry.
const DOCUMENT: u32 = 10000;
const STORAGE: u32 = 2001;
const STORAGE_ALT: u32 = 2005;
const DRAWABLE_ATTACHMENT: u32 = 2003;
const FOOTNOTE_REFERENCE: u32 = 2008;
const SHAPE_INFO: u32 = 2011;
const EQUATION: u32 = 2015;
const HYPERLINK: u32 = 2032;
const TABLE_INFO: u32 = 6000;
const IMAGE: u32 = 3005;
const MOVIE: u32 = 3007;
const GROUP: u32 = 3008;
const CHART: u32 = 5021;

const ATTACHMENT_CHAR: char = '\u{FFFC}';
const MAX_DEPTH: usize = 16;
const MAX_LIST_LEVEL: u64 = 8;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// 1 = superscript, 2 = subscript.
    pub script: u8,
    pub link: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParaKind {
    Title,
    Subtitle,
    Heading(u8),
    Body,
    Quote,
}

#[derive(Clone, Debug)]
pub struct ListLabel {
    pub level: usize,
    /// Bullet or number text; empty for an indented paragraph without a label.
    pub label: String,
}

#[derive(Debug)]
pub enum Block {
    Para {
        kind: ParaKind,
        list: Option<ListLabel>,
        spans: Vec<Span>,
    },
    Table(table::Table),
    Placeholder(String),
    /// Text box or other shape with text, rendered set off from the body.
    Aside(Vec<Block>),
}

#[derive(Debug, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
    pub footnotes: Vec<Vec<Span>>,
}

pub fn load(store: &Store) -> Document {
    let mut ex = Extractor { store, style_cache: HashMap::new(), footnotes: Vec::new(), body_size: None };
    let mut blocks = Vec::new();
    if let Some(body) = ex.body_storage() {
        blocks = ex.storage_blocks(body, 0);
    }
    // Page-layout documents keep their text in free-floating text boxes.
    if !blocks.iter().any(has_text) {
        let mut shapes: Vec<u64> = store.find_kind(SHAPE_INFO).map(|(id, _)| id).collect();
        shapes.sort_unstable();
        for id in shapes {
            blocks.extend(ex.drawable_blocks(id, 0));
        }
    }
    Document { blocks, footnotes: ex.footnotes }
}

fn has_text(b: &Block) -> bool {
    match b {
        Block::Para { spans, .. } => spans.iter().any(|s| !s.text.trim().is_empty()),
        Block::Aside(inner) => inner.iter().any(has_text),
        Block::Table(_) | Block::Placeholder(_) => true,
    }
}

/// Run-length attribute table: entry applies from its offset to the next one.
struct AttrTable<T> {
    entries: Vec<(u32, T)>,
}

impl<T: Clone> AttrTable<T> {
    fn covering(&self, idx: u32) -> Option<&T> {
        let i = self.entries.partition_point(|(at, _)| *at <= idx);
        if i == 0 {
            None
        } else {
            Some(&self.entries[i - 1].1)
        }
    }

    fn exact(&self, idx: u32) -> Option<&T> {
        self.entries.binary_search_by_key(&idx, |(at, _)| *at).ok().map(|i| &self.entries[i].1)
    }
}

fn object_table(storage: &Msg, field: u32) -> AttrTable<Option<u64>> {
    let mut entries: Vec<_> = storage
        .msg(field)
        .map(|t| t.msgs(1).map(|e| (e.varint(1).unwrap_or(0) as u32, e.reference(2))).collect())
        .unwrap_or_default();
    entries.sort_by_key(|(at, _)| *at);
    AttrTable { entries }
}

fn data_table(storage: &Msg, field: u32) -> AttrTable<u64> {
    let mut entries: Vec<_> = storage
        .msg(field)
        .map(|t| t.msgs(1).map(|e| (e.varint(1).unwrap_or(0) as u32, e.varint(2).unwrap_or(0))).collect())
        .unwrap_or_default();
    entries.sort_by_key(|(at, _)| *at);
    AttrTable { entries }
}

pub struct Extractor<'a> {
    store: &'a Store,
    style_cache: HashMap<(Option<u64>, Option<u64>), Style>,
    footnotes: Vec<Vec<Span>>,
    body_size: Option<f32>,
}

struct ListState {
    counters: [u32; 10],
    style: Option<u64>,
}

impl<'a> Extractor<'a> {
    fn body_storage(&self) -> Option<u64> {
        let (id, _) = self.store.find_kind(DOCUMENT).next()?;
        let (_, doc) = self.store.msg(id)?;
        let body = doc.reference(4)?;
        matches!(self.store.get(body)?.kind, STORAGE | STORAGE_ALT).then_some(body)
    }

    /// Style objects from `id` up through its parents (variation -> named style -> ...).
    fn chain(&self, id: Option<u64>) -> Vec<Msg<'a>> {
        let mut out = Vec::new();
        let mut cur = id;
        while let Some(id) = cur {
            if out.len() >= MAX_DEPTH {
                break;
            }
            let Some((_, m)) = self.store.msg(id) else { break };
            cur = m.msg(1).and_then(|s| s.reference(3));
            out.push(m);
        }
        out
    }

    fn style_names(&self, chain: &[Msg]) -> Vec<(String, String)> {
        chain
            .iter()
            .filter_map(|m| m.msg(1))
            .map(|s| (s.string(1).unwrap_or_default(), s.string(2).unwrap_or_default()))
            .collect()
    }

    /// First value of a character property found walking the character style
    /// chain, then the paragraph style chain.
    fn char_prop<T>(&self, chains: &[&[Msg]], f: impl Fn(&Msg) -> Option<T>) -> Option<T> {
        chains.iter().flat_map(|c| c.iter()).find_map(|style| style.msg(11).and_then(|cp| f(&cp)))
    }

    fn resolve_style(&mut self, para_style: Option<u64>, char_style: Option<u64>) -> Style {
        if let Some(s) = self.style_cache.get(&(para_style, char_style)) {
            return s.clone();
        }
        let pc = self.chain(para_style);
        let cc = self.chain(char_style);
        let chains: [&[Msg]; 2] = [&cc, &pc];
        let font = self.char_prop(&chains, |cp| cp.string(5)).unwrap_or_default();
        let style = Style {
            bold: self
                .char_prop(&chains, |cp| cp.bool(1))
                .unwrap_or_else(|| ["Bold", "Black", "Heavy", "Semibold"].iter().any(|w| font.contains(w))),
            italic: self
                .char_prop(&chains, |cp| cp.bool(2))
                .unwrap_or_else(|| font.contains("Italic") || font.contains("Oblique")),
            underline: self.char_prop(&chains, |cp| cp.varint(11)).unwrap_or(0) != 0,
            strike: self.char_prop(&chains, |cp| cp.varint(12)).unwrap_or(0) != 0,
            script: self.char_prop(&chains, |cp| cp.varint(10)).unwrap_or(0) as u8,
            link: None,
        };
        self.style_cache.insert((para_style, char_style), style.clone());
        style
    }

    fn para_kind(&self, para_style: Option<u64>) -> ParaKind {
        let chain = self.chain(para_style);
        for (name, ident) in self.style_names(&chain) {
            let lname = name.to_lowercase();
            if ident.ends_with("paragraphstyle-Title") || lname == "title" {
                return ParaKind::Title;
            }
            if ident.ends_with("paragraphstyle-Subtitle") || lname == "subtitle" {
                return ParaKind::Subtitle;
            }
            if let Some(n) = ident.split("paragraphstyle-Heading ").nth(1).and_then(|n| n.trim().parse().ok()) {
                return ParaKind::Heading(n);
            }
            if let Some(rest) = lname.strip_prefix("heading") {
                let rest = rest.trim();
                if rest.is_empty() {
                    return ParaKind::Heading(1);
                }
                if let Ok(n) = rest.parse() {
                    return ParaKind::Heading(n);
                }
            }
            if lname.contains("quote") {
                return ParaKind::Quote;
            }
        }
        ParaKind::Body
    }

    /// Guess a heading for body paragraphs formatted by hand (big, bold, short).
    fn size_heading(
        &mut self,
        para_style: Option<u64>,
        char_style: Option<u64>,
        bold: bool,
        len: usize,
    ) -> Option<ParaKind> {
        if len == 0 || len > 100 {
            return None;
        }
        let pc = self.chain(para_style);
        let cc = self.chain(char_style);
        let size = self.char_prop(&[&cc, &pc], |cp| cp.float(3))?;
        let base = self.body_size();
        if size >= base * 1.6 {
            Some(ParaKind::Heading(1))
        } else if size >= base * 1.3 && bold {
            Some(ParaKind::Heading(2))
        } else {
            None
        }
    }

    fn body_size(&mut self) -> f32 {
        if let Some(size) = self.body_size {
            return size;
        }
        let size = self.find_body_size();
        self.body_size = Some(size);
        size
    }

    fn find_body_size(&self) -> f32 {
        // The stylesheet's "Body" style sets the document's base size.
        for (id, o) in self.store.find_kind(2022) {
            let m = Msg::parse(&o.data);
            if m.msg(1).and_then(|s| s.string(2)).is_some_and(|i| i.contains("paragraphstyle-Body")) {
                if let Some(sz) = self.char_prop(&[&self.chain(Some(id))], |cp| cp.float(3)) {
                    return sz;
                }
            }
        }
        12.0
    }

    fn list_label(
        &self,
        list_style: Option<u64>,
        level: usize,
        start: Option<u32>,
        state: &mut ListState,
    ) -> Option<ListLabel> {
        let id = list_style?;
        let (_, ls) = self.store.msg(id)?;
        let at = |v: &[u64]| v.get(level).or(v.last()).copied();
        let label_type = at(&ls.varints(11)).unwrap_or(0);
        let lvl = level.min(state.counters.len() - 1);
        for c in &mut state.counters[lvl + 1..] {
            *c = 0;
        }
        let label = match label_type {
            // 1 = image bullet, 2 = text bullet
            1 => "•".to_string(),
            2 => {
                let strings = ls.strings(16);
                let s = strings.get(level).or(strings.last()).map(|s| s.trim().to_string()).unwrap_or_default();
                if s.is_empty() {
                    "•".to_string()
                } else {
                    s
                }
            }
            3 => {
                if state.style != Some(id) && start.is_none() {
                    state.counters = [0; 10];
                }
                state.style = Some(id);
                state.counters[lvl] = match start {
                    Some(n) => n,
                    None => state.counters[lvl].saturating_add(1),
                };
                let kind = at(&ls.varints(15)).unwrap_or(0);
                let tiered = ls.varints(25).get(level).copied().unwrap_or(0) != 0;
                if tiered {
                    let parts: Vec<String> =
                        state.counters[..=lvl].iter().map(|&n| format_number(n.max(1), 0)).collect();
                    format!("{}.", parts.join("."))
                } else {
                    decorate_number(state.counters[lvl], kind)
                }
            }
            _ if level > 0 => String::new(),
            _ => return None,
        };
        // Imported (e.g. Word) list styles may offset every level instead of
        // using the level itself; treat each ~18pt of base indent as a level.
        let offset = ls.floats(13).first().map_or(0, |&i| (i / 18.0).round().clamp(0.0, 8.0) as usize);
        Some(ListLabel { level: level + offset, label })
    }

    fn storage_blocks(&mut self, storage_id: u64, depth: usize) -> Vec<Block> {
        let store = self.store;
        let Some((_, st)) = store.msg(storage_id) else { return Vec::new() };
        if depth > MAX_DEPTH {
            return Vec::new();
        }
        let text: String = st.strings(3).concat();
        let para_styles = object_table(&st, 5);
        let para_data = data_table(&st, 6);
        let list_styles = object_table(&st, 7);
        let char_styles = object_table(&st, 8);
        let attachments = object_table(&st, 9);
        let smartfields = object_table(&st, 11);
        let para_starts = data_table(&st, 14);
        let footnotes = object_table(&st, 16);

        // Paragraph style entries without an object continue the previous style.
        let mut para_style_resolved: Vec<(u32, Option<u64>)> = Vec::new();
        let mut last = None;
        for (at, obj) in &para_styles.entries {
            if obj.is_some() {
                last = *obj;
            }
            para_style_resolved.push((*at, last));
        }
        let para_styles = AttrTable { entries: para_style_resolved };

        let mut blocks = Vec::new();
        let mut list_state = ListState { counters: [0; 10], style: None };
        let mut chars = text.chars().peekable();
        let mut idx: u32 = 0; // UTF-16 offset

        while chars.peek().is_some() {
            let start = idx;
            let pstyle = para_styles.covering(start).copied().flatten();
            let mut spans: Vec<Span> = Vec::new();
            let mut extra: Vec<Block> = Vec::new();
            let mut first_char_style = None;

            for c in chars.by_ref() {
                let at = idx;
                idx += c.len_utf16() as u32;
                if matches!(c, '\n' | '\u{2029}' | '\u{000C}' | '\u{0004}' | '\u{000E}' | '\u{0005}') {
                    break;
                }
                let cstyle = char_styles.covering(at).copied().flatten();
                if first_char_style.is_none() && !c.is_whitespace() {
                    first_char_style = Some(cstyle);
                }
                let mut style = self.resolve_style(pstyle, cstyle);
                if let Some(Some(f)) = smartfields.covering(at) {
                    if let Some((HYPERLINK, m)) = store.msg(*f) {
                        style.link = m.string(2);
                    }
                }
                let piece: String = match c {
                    ATTACHMENT_CHAR => {
                        if let Some(Some(f)) = footnotes.exact(at) {
                            self.footnote_mark(*f, depth)
                        } else if let Some(Some(a)) = attachments.exact(at) {
                            self.attachment(*a, depth, &mut extra)
                        } else {
                            String::new()
                        }
                    }
                    '\u{2028}' | '\u{000B}' => "\n".into(),
                    '\t' => "    ".into(),
                    '\u{00AD}' => String::new(),
                    c if c.is_control() => String::new(),
                    c => c.to_string(),
                };
                if piece.is_empty() {
                    continue;
                }
                if c == ATTACHMENT_CHAR && footnotes.exact(at).is_some() {
                    style.script = 1;
                }
                match spans.last_mut() {
                    Some(last) if last.style == style => last.text.push_str(&piece),
                    _ => spans.push(Span { text: piece, style }),
                }
            }

            let has_text = spans.iter().any(|s| !s.text.trim().is_empty());
            if has_text {
                let level = para_data.covering(start).copied().unwrap_or(0).min(MAX_LIST_LEVEL) as usize;
                let lstyle = list_styles.covering(start).copied().flatten().or_else(|| self.para_list_style(pstyle));
                let explicit = para_starts.exact(start).copied().filter(|&n| n > 0).map(|n| n as u32);
                let list = self.list_label(lstyle, level, explicit, &mut list_state);
                let mut kind = self.para_kind(pstyle);
                if kind == ParaKind::Body && list.is_none() {
                    let len: usize = spans.iter().map(|s| s.text.chars().count()).sum();
                    let all_bold = spans.iter().filter(|s| !s.text.trim().is_empty()).all(|s| s.style.bold);
                    if let Some(k) = self.size_heading(pstyle, first_char_style.flatten(), all_bold, len) {
                        kind = k;
                    }
                }
                trim_spans(&mut spans);
                blocks.push(Block::Para { kind, list, spans });
            }
            blocks.append(&mut extra);
        }
        blocks
    }

    fn para_list_style(&self, para_style: Option<u64>) -> Option<u64> {
        self.chain(para_style).iter().find_map(|s| s.msg(12).and_then(|pp| pp.reference(40)))
    }

    fn footnote_mark(&mut self, id: u64, depth: usize) -> String {
        let Some((kind, m)) = self.store.msg(id) else { return String::new() };
        if kind != FOOTNOTE_REFERENCE {
            return String::new();
        }
        let n = self.footnotes.len() + 1;
        let mark = m.string(3).filter(|s| !s.is_empty()).unwrap_or_else(|| n.to_string());
        let body = m.reference(2).map(|s| self.storage_blocks(s, depth + 1)).unwrap_or_default();
        let mut spans = Vec::new();
        for b in body {
            if let Block::Para { spans: s, .. } = b {
                if !spans.is_empty() {
                    spans.push(Span { text: " ".into(), style: Style::default() });
                }
                spans.extend(s);
            }
        }
        self.footnotes.push(spans);
        format!("[{mark}]")
    }

    /// Inline attachments either produce inline text (returned) or blocks that
    /// follow the paragraph (pushed to `extra`).
    fn attachment(&mut self, id: u64, depth: usize, extra: &mut Vec<Block>) -> String {
        let Some((kind, m)) = self.store.msg(id) else { return String::new() };
        match kind {
            DRAWABLE_ATTACHMENT => {
                if let Some(d) = m.reference(1) {
                    extra.extend(self.drawable_blocks(d, depth + 1));
                }
                String::new()
            }
            // Textual attachments: page number, page count, footnote mark.
            2004 | 2007 | 2009 | 2010 => m.string(1).unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn drawable_blocks(&mut self, id: u64, depth: usize) -> Vec<Block> {
        if depth > MAX_DEPTH {
            return Vec::new();
        }
        let Some((kind, m)) = self.store.msg(id) else { return Vec::new() };
        match kind {
            TABLE_INFO => table::load(self, id, depth).into_iter().collect(),
            IMAGE => {
                // ImageArchive.super -> DrawableArchive.accessibility_description
                let desc = m.msg(1).and_then(|d| d.string(8)).filter(|s| !s.trim().is_empty());
                vec![Block::Placeholder(match desc {
                    Some(d) => format!("image: {d}"),
                    None => "image".into(),
                })]
            }
            MOVIE => vec![Block::Placeholder("video".into())],
            CHART => vec![Block::Placeholder("chart".into())],
            EQUATION => vec![Block::Placeholder("equation".into())],
            GROUP => {
                let children: Vec<u64> = m.msgs(2).filter_map(|r| r.varint(1)).collect();
                children.into_iter().flat_map(|c| self.drawable_blocks(c, depth + 1)).collect()
            }
            SHAPE_INFO => {
                let Some(storage) = m.reference(4).or_else(|| m.reference(2)) else { return Vec::new() };
                let inner = self.storage_blocks(storage, depth + 1);
                if inner.iter().any(has_text) {
                    vec![Block::Aside(inner)]
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }

    /// Flattened text of a storage, used for table cells. Paragraphs are
    /// separated by newlines; list labels are kept.
    pub fn storage_spans(&mut self, storage_id: u64, depth: usize) -> Vec<Span> {
        let mut out: Vec<Span> = Vec::new();
        for b in self.storage_blocks(storage_id, depth + 1) {
            if let Block::Para { list, spans, .. } = b {
                if !out.is_empty() {
                    out.push(Span { text: "\n".into(), style: Style::default() });
                }
                if let Some(l) = list.filter(|l| !l.label.is_empty()) {
                    out.push(Span { text: format!("{} ", l.label), style: Style::default() });
                }
                out.extend(spans);
            }
        }
        out
    }

    pub fn store(&self) -> &'a Store {
        self.store
    }
}

fn trim_spans(spans: &mut Vec<Span>) {
    if let Some(first) = spans.first_mut() {
        first.text = first.text.trim_start_matches([' ', '\u{a0}']).to_string();
    }
    if let Some(last) = spans.last_mut() {
        last.text = last.text.trim_end().to_string();
    }
    spans.retain(|s| !s.text.is_empty());
}

fn decorate_number(n: u32, kind: u64) -> String {
    if kind == 48 && (1..=20).contains(&n) {
        return char::from_u32(0x2460 + n - 1).unwrap().to_string();
    }
    let (base, punct) = if kind < 15 { (kind / 3, kind % 3) } else { (0, (kind.saturating_sub(15)) % 3) };
    let s = format_number(n, base);
    match punct {
        1 => format!("({s})"),
        2 => format!("{s})"),
        _ => format!("{s}."),
    }
}

/// base: 0 decimal, 1 upper roman, 2 lower roman, 3 upper alpha, 4 lower alpha.
fn format_number(n: u32, base: u64) -> String {
    match base {
        1 => roman(n),
        2 => roman(n).to_lowercase(),
        3 => alpha(n),
        4 => alpha(n).to_lowercase(),
        _ => n.to_string(),
    }
}

fn roman(mut n: u32) -> String {
    if n == 0 || n >= 4000 {
        return n.to_string();
    }
    let table = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut s = String::new();
    for (v, r) in table {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

fn alpha(n: u32) -> String {
    // 1 -> A, 26 -> Z, 27 -> AA (as Pages does)
    if n == 0 || n > 26 * 8 {
        return n.to_string();
    }
    let letter = (b'A' + ((n - 1) % 26) as u8) as char;
    letter.to_string().repeat(((n - 1) / 26 + 1) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_formats() {
        assert_eq!(decorate_number(3, 0), "3.");
        assert_eq!(decorate_number(4, 3), "IV.");
        assert_eq!(decorate_number(9, 7), "(ix)");
        assert_eq!(decorate_number(2, 11), "B)");
        assert_eq!(decorate_number(28, 12), "bb.");
        assert_eq!(decorate_number(2, 48), "②");
    }
}
