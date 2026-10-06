//! Minimal schema-less protobuf reader. iWork archives are proto2 messages; we
//! only read the handful of fields we need, addressed by field number.

#[derive(Clone, Copy, Debug)]
pub enum Val<'a> {
    Varint(u64),
    Fixed64,
    Fixed32(u32),
    Bytes(&'a [u8]),
}

#[derive(Clone, Debug, Default)]
pub struct Msg<'a> {
    fields: Vec<(u32, Val<'a>)>,
}

pub fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut result = 0u64;
    let mut shift = 0;
    loop {
        let b = *buf.get(*pos)?;
        *pos += 1;
        if shift < 64 {
            result |= u64::from(b & 0x7f) << shift;
        }
        if b < 0x80 {
            return Some(result);
        }
        shift += 7;
        if shift > 70 {
            return None;
        }
    }
}

impl<'a> Msg<'a> {
    /// Parse a message. Malformed trailing data is dropped rather than failing,
    /// so a partially understood archive still yields what it can.
    pub fn parse(buf: &'a [u8]) -> Msg<'a> {
        let mut fields = Vec::new();
        let mut pos = 0;
        while pos < buf.len() {
            let Some(key) = read_varint(buf, &mut pos) else { break };
            let field = (key >> 3) as u32;
            let val = match key & 7 {
                0 => match read_varint(buf, &mut pos) {
                    Some(v) => Val::Varint(v),
                    None => break,
                },
                1 => {
                    if pos + 8 > buf.len() {
                        break;
                    }
                    pos += 8;
                    Val::Fixed64
                }
                2 => {
                    let Some(len) = read_varint(buf, &mut pos) else { break };
                    let Some(b) = buf.get(pos..pos.saturating_add(len as usize)) else { break };
                    pos += len as usize;
                    Val::Bytes(b)
                }
                5 => {
                    let Some(b) = buf.get(pos..pos + 4) else { break };
                    pos += 4;
                    Val::Fixed32(u32::from_le_bytes(b.try_into().unwrap()))
                }
                _ => break,
            };
            fields.push((field, val));
        }
        Msg { fields }
    }

    fn last(&self, f: u32) -> Option<Val<'a>> {
        self.fields.iter().rev().find(|(n, _)| *n == f).map(|(_, v)| *v)
    }

    pub fn varint(&self, f: u32) -> Option<u64> {
        match self.last(f)? {
            Val::Varint(v) => Some(v),
            _ => None,
        }
    }

    pub fn bool(&self, f: u32) -> Option<bool> {
        self.varint(f).map(|v| v != 0)
    }

    pub fn float(&self, f: u32) -> Option<f32> {
        match self.last(f)? {
            Val::Fixed32(v) => Some(f32::from_bits(v)),
            _ => None,
        }
    }

    pub fn bytes(&self, f: u32) -> Option<&'a [u8]> {
        match self.last(f)? {
            Val::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn string(&self, f: u32) -> Option<String> {
        self.bytes(f).map(|b| String::from_utf8_lossy(b).into_owned())
    }

    pub fn msg(&self, f: u32) -> Option<Msg<'a>> {
        self.bytes(f).map(Msg::parse)
    }

    pub fn msgs(&self, f: u32) -> impl Iterator<Item = Msg<'a>> + '_ {
        self.fields.iter().filter(move |(n, _)| *n == f).filter_map(|(_, v)| match v {
            Val::Bytes(b) => Some(Msg::parse(b)),
            _ => None,
        })
    }

    pub fn strings(&self, f: u32) -> Vec<String> {
        self.fields
            .iter()
            .filter(|(n, _)| *n == f)
            .filter_map(|(_, v)| match v {
                Val::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
                _ => None,
            })
            .collect()
    }

    /// Repeated varint field, accepting both packed and unpacked encodings.
    pub fn varints(&self, f: u32) -> Vec<u64> {
        let mut out = Vec::new();
        for (n, v) in &self.fields {
            if *n != f {
                continue;
            }
            match v {
                Val::Varint(x) => out.push(*x),
                Val::Bytes(b) => {
                    let mut pos = 0;
                    while pos < b.len() {
                        match read_varint(b, &mut pos) {
                            Some(x) => out.push(x),
                            None => break,
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Repeated float field, accepting both packed and unpacked encodings.
    pub fn floats(&self, f: u32) -> Vec<f32> {
        let mut out = Vec::new();
        for (n, v) in &self.fields {
            match v {
                Val::Fixed32(x) if *n == f => out.push(f32::from_bits(*x)),
                Val::Bytes(b) if *n == f => {
                    out.extend(b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())))
                }
                _ => {}
            }
        }
        out
    }

    /// A `TSP.Reference` stored in field `f`, returning the object identifier.
    pub fn reference(&self, f: u32) -> Option<u64> {
        self.msg(f)?.varint(1).filter(|&id| id != 0)
    }
}
