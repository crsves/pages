//! Reading the iWork container: a zip (or package directory) holding
//! `Index/*.iwa` files, each a stream of Snappy-compressed chunks that decode to
//! length-prefixed `TSP.ArchiveInfo` headers followed by message payloads.

use crate::proto::{read_varint, Msg};
use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::Path;

/// Upper bound on decompressed archive data for one document. Real documents
/// are a few MB; this only stops crafted files from exhausting memory.
const MAX_TOTAL_BYTES: usize = 512 << 20;
/// IWA chunks hold at most 64 KiB uncompressed in practice.
const MAX_CHUNK_BYTES: usize = 16 << 20;

pub struct Object {
    pub kind: u32,
    pub data: Vec<u8>,
}

#[derive(Default)]
pub struct Store {
    objects: HashMap<u64, Object>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        let mut store = Store::default();
        let mut budget = MAX_TOTAL_BYTES;
        for (name, raw) in read_container(path)? {
            let data = decompress(&raw, budget).with_context(|| format!("decompressing {name}"))?;
            budget -= data.len();
            store.ingest(&data).with_context(|| format!("decoding {name}"))?;
        }
        if store.objects.is_empty() {
            bail!("no iWork archives found; is this a Pages document?");
        }
        Ok(store)
    }

    /// Build a store from already-decompressed IWA streams, keeping whatever
    /// parses. Used to exercise the decoder with corrupted input.
    pub fn from_streams<'s>(streams: impl IntoIterator<Item = &'s [u8]>) -> Store {
        let mut store = Store::default();
        for data in streams {
            let _ = store.ingest(data);
        }
        store
    }

    pub fn get(&self, id: u64) -> Option<&Object> {
        self.objects.get(&id)
    }

    /// Parse an object's payload, optionally requiring a specific type.
    pub fn msg(&self, id: u64) -> Option<(u32, Msg<'_>)> {
        self.objects.get(&id).map(|o| (o.kind, Msg::parse(&o.data)))
    }

    pub fn find_kind(&self, kind: u32) -> impl Iterator<Item = (u64, &Object)> {
        self.objects.iter().filter(move |(_, o)| o.kind == kind).map(|(id, o)| (*id, o))
    }

    fn ingest(&mut self, data: &[u8]) -> Result<()> {
        let mut pos = 0;
        let take = |pos: &mut usize, len: u64| -> Result<&[u8]> {
            let end = usize::try_from(len).ok().and_then(|l| pos.checked_add(l)).filter(|&e| e <= data.len());
            let end = end.ok_or_else(|| anyhow!("truncated archive"))?;
            let b = &data[*pos..end];
            *pos = end;
            Ok(b)
        };
        while pos < data.len() {
            let len = read_varint(data, &mut pos).ok_or_else(|| anyhow!("truncated header"))?;
            let info = Msg::parse(take(&mut pos, len)?);
            let id = info.varint(1).unwrap_or(0);
            // The first MessageInfo is the object itself; later ones are diffs.
            for (i, mi) in info.msgs(2).enumerate() {
                let payload = take(&mut pos, mi.varint(3).unwrap_or(0))?;
                if i == 0 {
                    let kind = mi.varint(1).unwrap_or(0) as u32;
                    self.objects.entry(id).or_insert(Object { kind, data: payload.to_vec() });
                }
            }
        }
        Ok(())
    }
}

/// IWA framing: repeated [0x00, len:u24le, snappy-raw block].
pub fn decompress(raw: &[u8], limit: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut pos = 0;
    let mut dec = snap::raw::Decoder::new();
    while pos + 4 <= raw.len() {
        if raw[pos] != 0 {
            bail!("unexpected chunk type {:#x}", raw[pos]);
        }
        let len = u32::from_le_bytes([raw[pos + 1], raw[pos + 2], raw[pos + 3], 0]) as usize;
        pos += 4;
        let chunk = raw.get(pos..pos + len).ok_or_else(|| anyhow!("truncated chunk"))?;
        let size = snap::raw::decompress_len(chunk)?;
        if size > MAX_CHUNK_BYTES || out.len() + size > limit {
            bail!("document is too large");
        }
        out.extend(dec.decompress_vec(chunk)?);
        pos += len;
    }
    Ok(out)
}

fn read_file(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|e| {
        // ENEEDAUTH: macOS won't materialise an evicted iCloud ("dataless") file here.
        if e.raw_os_error() == Some(81) {
            anyhow!("file is in iCloud and not downloaded; open or download it in Finder, then retry")
        } else {
            anyhow!(e)
        }
    })
}

fn read_container(path: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    if path.is_dir() {
        let index_zip = path.join("Index.zip");
        if index_zip.exists() {
            return read_zip(read_file(&index_zip)?);
        }
        if path.join("index.xml").exists() || path.join("index.xml.gz").exists() {
            bail!("this is a Pages '09 (XML) document; only Pages 5+ files are supported");
        }
        let mut out = Vec::new();
        walk(&path.join("Index"), "Index", &mut out)?;
        return Ok(out);
    }
    read_zip(read_file(path)?)
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let name = format!("{prefix}/{}", entry.file_name().to_string_lossy());
        if entry.file_type()?.is_dir() {
            walk(&entry.path(), &name, out)?;
        } else if name.ends_with(".iwa") {
            if entry.metadata()?.len() > MAX_TOTAL_BYTES as u64 {
                bail!("document is too large");
            }
            out.push((name, read_file(&entry.path())?));
        }
    }
    Ok(())
}

fn read_zip(bytes: Vec<u8>) -> Result<Vec<(String, Vec<u8>)>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| anyhow!("not a Pages document"))?;
    let mut out = Vec::new();
    let mut legacy = false;
    let mut budget = MAX_TOTAL_BYTES;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i)?;
        let name = f.name().to_string();
        if name == "index.xml" || name == "index.xml.gz" {
            legacy = true;
        }
        if name.ends_with(".iwa") {
            let mut buf = Vec::new();
            (&mut f).take(budget as u64 + 1).read_to_end(&mut buf)?;
            budget = budget.checked_sub(buf.len()).ok_or_else(|| anyhow!("document is too large"))?;
            out.push((name, buf));
        }
    }
    if out.is_empty() && legacy {
        bail!("this is a Pages '09 (XML) document; only Pages 5+ files are supported");
    }
    Ok(out)
}
