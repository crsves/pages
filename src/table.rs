//! Tables (`TST.TableInfoArchive`). Cell values live in tiles of packed binary
//! cell records; strings and rich text are interned in per-table data lists.

use crate::doc::{Block, Extractor, Span, Style};
use std::collections::{BTreeMap, HashMap};

const RICH_TEXT_PAYLOAD: u32 = 6218;
/// Cells rendered per table; beyond this, trailing rows are summarised.
const MAX_GRID_CELLS: usize = 100_000;

#[derive(Debug)]
pub struct Table {
    pub name: Option<String>,
    pub header_rows: usize,
    pub header_cols: usize,
    pub rows: Vec<Vec<Vec<Span>>>,
    /// Rows left out because the table was too large to render.
    pub omitted_rows: usize,
}

pub fn load(ex: &mut Extractor, info_id: u64, depth: usize) -> Option<Block> {
    let store = ex.store();
    let (_, info) = store.msg(info_id)?;
    let (_, model) = store.msg(info.reference(2)?)?;
    let nrows = usize::try_from(model.varint(6).unwrap_or(0)).unwrap_or(usize::MAX);
    let ncols = usize::try_from(model.varint(7).unwrap_or(0)).unwrap_or(usize::MAX);
    let ds = model.msg(4)?;

    let strings = data_list(ex, ds.reference(4));
    let rich = data_list(ex, ds.reference(17));

    // rowTileTree maps a tile's first row to its tile id.
    let tile_first_row: HashMap<u64, usize> = ds
        .msg(9)
        .map(|t| t.msgs(1).map(|n| (n.varint(2).unwrap_or(0), n.varint(1).unwrap_or(0) as usize)).collect())
        .unwrap_or_default();
    let tiles = ds.msg(3)?;
    let tile_size = tiles.varint(2).unwrap_or(256) as usize;

    // Sparse: only cells that hold something are stored.
    let mut cells: BTreeMap<(usize, usize), Vec<Span>> = BTreeMap::new();
    let mut legacy = false;
    for t in tiles.msgs(1) {
        let tile_id = t.varint(1).unwrap_or(0);
        let Some((_, tile)) = t.reference(2).and_then(|r| store.msg(r)) else { continue };
        let base =
            tile_first_row.get(&tile_id).copied().unwrap_or_else(|| (tile_id as usize).saturating_mul(tile_size));
        for ri in tile.msgs(5) {
            let row = base.saturating_add(ri.varint(1).unwrap_or(0) as usize);
            let (Some(buf), Some(offsets)) = (ri.bytes(6), ri.bytes(7)) else {
                // Pre-2019 tiles only carry the older cell format.
                legacy |= ri.bytes(3).is_some();
                continue;
            };
            if row >= nrows {
                continue;
            }
            let wide = ri.bool(8).unwrap_or(false);
            for (col, cell) in row_cells(buf, offsets, ncols, wide).into_iter().enumerate() {
                if cells.len() >= MAX_GRID_CELLS {
                    break;
                }
                let Some(cell) = cell else { continue };
                let spans = decode_cell(ex, cell, &strings, &rich, depth);
                if !spans.is_empty() {
                    cells.insert((row, col), spans);
                }
            }
        }
    }
    if cells.is_empty() {
        return legacy.then(|| Block::Placeholder("table (saved by an older Pages version; not supported)".into()));
    }

    // Lay out only the used extent, capped in total size.
    let used_rows = cells.keys().map(|&(r, _)| r).max().unwrap_or(0) + 1;
    let used_cols = cells.keys().map(|&(_, c)| c).max().unwrap_or(0) + 1;
    let shown_rows = used_rows.min((MAX_GRID_CELLS / used_cols).max(1));
    let mut rows: Vec<Vec<Vec<Span>>> = vec![vec![Vec::new(); used_cols]; shown_rows];
    for ((r, c), spans) in cells {
        if r < shown_rows {
            rows[r][c] = spans;
        }
    }

    let name = model.string(8).filter(|_| model.bool(22).unwrap_or(false)).filter(|s| !s.trim().is_empty());
    Some(Block::Table(Table {
        name,
        header_rows: model.varint(9).unwrap_or(0) as usize,
        header_cols: model.varint(10).unwrap_or(0) as usize,
        rows,
        omitted_rows: used_rows - shown_rows,
    }))
}

enum Entry {
    Text(String),
    Rich(u64),
}

fn data_list(ex: &Extractor, id: Option<u64>) -> HashMap<u32, Entry> {
    let mut out = HashMap::new();
    let Some((_, list)) = id.and_then(|id| ex.store().msg(id)) else { return out };
    for e in list.msgs(3) {
        let key = e.varint(1).unwrap_or(0) as u32;
        if let Some(s) = e.string(3) {
            out.insert(key, Entry::Text(s));
        } else if let Some(r) = e.reference(9) {
            out.insert(key, Entry::Rich(r));
        }
    }
    out
}

/// Split a row's storage buffer into per-column cell records using the
/// 16-bit offset array (-1 = empty; offsets are in 4-byte units when wide).
fn row_cells<'b>(buf: &'b [u8], offsets: &[u8], ncols: usize, wide: bool) -> Vec<Option<&'b [u8]>> {
    let offs: Vec<i32> = offsets
        .chunks_exact(2)
        .take(ncols)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as i32)
        .map(|o| if o >= 0 && wide { o * 4 } else { o })
        .collect();
    (0..offs.len())
        .map(|i| {
            let start = usize::try_from(offs[i]).ok()?;
            let end = offs[i + 1..].iter().find(|&&o| o >= 0).map_or(buf.len(), |&o| o as usize);
            buf.get(start..end.max(start))
        })
        .collect()
}

fn decode_cell(
    ex: &mut Extractor,
    cell: &[u8],
    strings: &HashMap<u32, Entry>,
    rich: &HashMap<u32, Entry>,
    depth: usize,
) -> Vec<Span> {
    if cell.len() < 12 || cell[0] != 5 {
        return Vec::new();
    }
    let kind = cell[1];
    let flags = u32::from_le_bytes(cell[8..12].try_into().unwrap());
    let mut pos = 12;
    let mut take = |n: usize| -> Option<&[u8]> {
        let b = cell.get(pos..pos + n);
        pos += n;
        b
    };
    let mut d128 = None;
    let mut double = None;
    let mut seconds = None;
    let mut string_id = None;
    let mut rich_id = None;
    if flags & 0x1 != 0 {
        d128 = take(16).map(|b| <[u8; 16]>::try_from(b).unwrap());
    }
    if flags & 0x2 != 0 {
        double = take(8).map(|b| f64::from_le_bytes(b.try_into().unwrap()));
    }
    if flags & 0x4 != 0 {
        seconds = take(8).map(|b| f64::from_le_bytes(b.try_into().unwrap()));
    }
    if flags & 0x8 != 0 {
        string_id = take(4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    }
    if flags & 0x10 != 0 {
        rich_id = take(4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    }

    let text = |s: String| vec![Span { text: s, style: Style::default() }];
    match kind {
        // text
        3 => match string_id.and_then(|k| strings.get(&k)) {
            Some(Entry::Text(s)) => text(s.clone()),
            _ => Vec::new(),
        },
        // number, currency
        2 | 10 => match (d128, double) {
            (Some(d), _) => text(decimal128(&d)),
            (None, Some(f)) => text(format_float(f)),
            _ => Vec::new(),
        },
        5 => seconds.map(|s| text(format_date(s))).unwrap_or_default(),
        6 => double.map(|d| text(if d > 0.0 { "TRUE" } else { "FALSE" }.into())).unwrap_or_default(),
        7 => double.map(|d| text(format_duration(d))).unwrap_or_default(),
        8 => text("#ERROR".into()),
        // rich text ("automatic") cells
        9 => match rich_id.and_then(|k| rich.get(&k)) {
            Some(Entry::Rich(payload)) => {
                let storage =
                    ex.store().msg(*payload).filter(|(k, _)| *k == RICH_TEXT_PAYLOAD).and_then(|(_, m)| m.reference(1));
                storage.map(|s| ex.storage_spans(s, depth)).unwrap_or_default()
            }
            Some(Entry::Text(s)) => text(s.clone()),
            None => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// IEEE 754-2008 decimal128 (BID, as stored by Numbers/Pages) to an exact string.
pub fn decimal128(b: &[u8; 16]) -> String {
    let exp = ((((b[15] & 0x7f) as i32) << 7) | (b[14] >> 1) as i32) - 0x1820;
    let mut mantissa: u128 = (b[14] & 1) as u128;
    for i in (0..14).rev() {
        mantissa = (mantissa << 8) | b[i] as u128;
    }
    let neg = b[15] & 0x80 != 0;
    let digits = mantissa.to_string();
    let mut s = if exp >= 0 {
        if mantissa == 0 {
            "0".to_string()
        } else {
            format!("{digits}{}", "0".repeat(exp as usize))
        }
    } else {
        let point = -exp as usize;
        let padded = format!("{digits:0>width$}", width = point + 1);
        let (int, frac) = padded.split_at(padded.len() - point);
        let frac = frac.trim_end_matches('0');
        if frac.is_empty() {
            int.to_string()
        } else {
            format!("{int}.{frac}")
        }
    };
    if neg && s != "0" {
        s.insert(0, '-');
    }
    s
}

fn format_float(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{f:.0}")
    } else {
        let s = format!("{f:.10}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Seconds since 2001-01-01T00:00:00Z.
fn format_date(secs: f64) -> String {
    // Clamp to years 0..=9999 so the arithmetic below can't overflow.
    let total = (secs.floor() as i64).clamp(-63_000_000_000, 250_000_000_000) + 978_307_200; // to Unix epoch
    let days = total.div_euclid(86_400);
    let tod = total.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    if tod == 0 {
        format!("{y:04}-{m:02}-{d:02}")
    } else {
        format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", tod / 3600, tod % 3600 / 60)
    }
}

fn format_duration(secs: f64) -> String {
    let s = secs.round() as i64;
    let (h, m, s) = (s / 3600, s % 3600 / 60, s % 60);
    match (h, m) {
        (0, 0) => format!("{s}s"),
        (0, _) => format!("{m}m {s}s"),
        _ => format!("{h}h {m}m {s}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d128(mantissa: u128, exp: i32, neg: bool) -> [u8; 16] {
        let biased = (exp + 0x1820) as u128;
        let v = mantissa | (biased << 113) | ((neg as u128) << 127);
        v.to_le_bytes()
    }

    #[test]
    fn decimals() {
        assert_eq!(decimal128(&d128(1843, 0, false)), "1843");
        assert_eq!(decimal128(&d128(12345, -2, false)), "123.45");
        assert_eq!(decimal128(&d128(5, -3, true)), "-0.005");
        assert_eq!(decimal128(&d128(1500, -2, false)), "15");
        assert_eq!(decimal128(&d128(7, 2, false)), "700");
    }

    #[test]
    fn dates() {
        assert_eq!(format_date(0.0), "2001-01-01");
        assert_eq!(format_date(86_400.0 * 365.0 + 3_600.0 * 13.5), "2002-01-01 13:30");
    }
}
