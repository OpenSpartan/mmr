//! Bond Compact Binary v2, read without a schema into a tree of field ids.

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Uint(u64),
    Int(i64),
    F32(f32),
    F64(f64),
    Str(String),
    Struct(Vec<(u16, Value)>),
    List(Vec<Value>),
    Bytes(Vec<u8>),
    Map(Vec<(Value, Value)>),
}

impl Value {
    pub fn field(&self, id: u16) -> Option<&Value> {
        match self {
            Value::Struct(fields) => fields.iter().find(|(i, _)| *i == id).map(|(_, v)| v),
            _ => None,
        }
    }

    /// A `nullable<T>` is a list of zero or one; anything else is itself.
    pub fn maybe(&self) -> Option<&Value> {
        match self {
            Value::List(items) => items.first(),
            other => Some(other),
        }
    }

    pub fn uint(&self) -> Option<u64> {
        match self {
            Value::Uint(v) => Some(*v),
            Value::Int(v) => u64::try_from(*v).ok(),
            Value::Bool(v) => Some(u64::from(*v)),
            _ => None,
        }
    }
}

const DEPTH: u32 = 32;
/// The most elements reserved before reading them: a count is checked only against
/// the bytes left, at every level of nesting.
const ROOM: usize = 64;

struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.p.checked_add(n).ok_or("bond length overflow")?;
        let s = self.b.get(self.p..end).ok_or("bond past end")?;
        self.p = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn var(&mut self) -> Result<u64, String> {
        let (mut v, mut shift) = (0u64, 0u32);
        loop {
            let x = self.u8()?;
            if shift >= 64 {
                return Err("bond varint too long".into());
            }
            v |= u64::from(x & 127) << shift;
            if x & 128 == 0 {
                return Ok(v);
            }
            shift += 7;
        }
    }

    fn zig(&mut self) -> Result<i64, String> {
        let v = self.var()?;
        Ok((v >> 1).cast_signed() ^ -(v & 1).cast_signed())
    }

    fn count(&mut self, n: u64) -> Result<usize, String> {
        // Every element takes at least a byte, so a larger count is corrupt.
        if n > (self.b.len() - self.p) as u64 {
            return Err("bond count past end".into());
        }
        Ok(n as usize)
    }

    fn value(&mut self, t: u8, depth: u32) -> Result<Value, String> {
        if depth > DEPTH {
            return Err("bond too deep".into());
        }
        Ok(match t {
            2 => Value::Bool(self.u8()? != 0),
            3 => Value::Uint(u64::from(self.u8()?)),
            14 => Value::Int(i64::from(self.u8()?.cast_signed())),
            4..=6 => Value::Uint(self.var()?),
            15..=17 => Value::Int(self.zig()?),
            7 => Value::F32(f32::from_le_bytes(self.take(4)?.try_into().unwrap())),
            8 => Value::F64(f64::from_le_bytes(self.take(8)?.try_into().unwrap())),
            9 => {
                let n = self.var()? as usize;
                Value::Str(String::from_utf8_lossy(self.take(n)?).into_owned())
            }
            18 => {
                let n = self.var()? as usize;
                let raw = self.take(n.checked_mul(2).ok_or("bond length overflow")?)?;
                let units: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                Value::Str(String::from_utf16_lossy(&units))
            }
            10 => self.structure(depth + 1)?,
            11 | 12 => {
                let x = self.u8()?;
                let et = x & 31;
                let n = match x >> 5 {
                    0 => self.var()?,
                    k => u64::from(k - 1),
                };
                let n = self.count(n)?;
                if et == 3 || et == 14 {
                    Value::Bytes(self.take(n)?.to_vec())
                } else {
                    let mut out = Vec::with_capacity(n.min(ROOM));
                    for _ in 0..n {
                        out.push(self.value(et, depth + 1)?);
                    }
                    Value::List(out)
                }
            }
            13 => {
                let (kt, vt) = (self.u8()? & 31, self.u8()? & 31);
                let n = self.var()?;
                let n = self.count(n)?;
                let mut out = Vec::with_capacity(n.min(ROOM));
                for _ in 0..n {
                    let k = self.value(kt, depth + 1)?;
                    out.push((k, self.value(vt, depth + 1)?));
                }
                Value::Map(out)
            }
            _ => return Err(format!("bond type {t}")),
        })
    }

    fn structure(&mut self, depth: u32) -> Result<Value, String> {
        let n = self.var()? as usize;
        let end = self.p.checked_add(n).filter(|e| *e <= self.b.len()).ok_or("bond struct past end")?;
        let mut fields = Vec::new();
        loop {
            let h = self.u8()?;
            let t = h & 31;
            match t {
                0 => break,
                1 => continue, // the end of a base class's fields
                _ => {}
            }
            let id = match h >> 5 {
                6 => u16::from(self.u8()?),
                7 => u16::from_le_bytes(self.take(2)?.try_into().unwrap()),
                i => u16::from(i),
            };
            fields.push((id, self.value(t, depth)?));
        }
        if self.p != end {
            return Err("bond struct length".into());
        }
        Ok(Value::Struct(fields))
    }
}

pub fn parse(b: &[u8]) -> Result<Value, String> {
    Reader { b, p: 0 }.structure(0)
}

/// The leading fields of a struct, as far as `b` holds them. A byte list cut short
/// keeps the bytes there are, and reading stops at the first field that doesn't fit.
pub fn head(b: &[u8]) -> Vec<(u16, Value)> {
    let mut r = Reader { b, p: 0 };
    let mut fields = Vec::new();
    if r.var().is_err() {
        return fields;
    }
    while let Ok(h) = r.u8() {
        let t = h & 31;
        match t {
            0 => break,
            1 => continue,
            _ => {}
        }
        let id = match h >> 5 {
            6 => r.u8().map(u16::from),
            7 => r.take(2).map(|s| u16::from_le_bytes([s[0], s[1]])),
            i => Ok(u16::from(i)),
        };
        let Ok(id) = id else { break };
        let at = r.p;
        match r.value(t, 1) {
            Ok(v) => fields.push((id, v)),
            Err(_) => {
                r.p = at;
                if let (11 | 12, Ok(x)) = (t, r.u8()) {
                    let counted = x >> 5 != 0 || r.var().is_ok();
                    if matches!(x & 31, 3 | 14) && counted {
                        fields.push((id, Value::Bytes(r.b[r.p..].to_vec())));
                    }
                }
                break;
            }
        }
    }
    fields
}

/// For tests: the largest single allocation a closure makes on its own thread.
#[cfg(test)]
pub(crate) mod counting {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static LARGEST: Cell<usize> = const { Cell::new(0) };
    }

    struct Counting;

    // SAFETY: every call passes its arguments to System unchanged, and only records a
    // size in a thread-local Cell, which never allocates.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            LARGEST.with(|m| m.set(m.get().max(l.size())));
            // SAFETY: the caller's layout, passed on unchanged.
            unsafe { System.alloc(l) }
        }

        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            // SAFETY: p came from System with this layout, as every block here does.
            unsafe { System.dealloc(p, l) }
        }

        unsafe fn realloc(&self, p: *mut u8, l: Layout, size: usize) -> *mut u8 {
            LARGEST.with(|m| m.set(m.get().max(size)));
            // SAFETY: p came from System with this layout, as every block here does.
            unsafe { System.realloc(p, l, size) }
        }
    }

    #[global_allocator]
    static COUNTING: Counting = Counting;

    pub fn largest<T>(f: impl FnOnce() -> T) -> (T, usize) {
        LARGEST.with(|m| m.set(0));
        let r = f();
        (r, LARGEST.with(Cell::get))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(mut v: usize, out: &mut Vec<u8>) {
        while v >= 128 {
            out.push((v & 127) as u8 | 128);
            v >>= 7;
        }
        out.push(v as u8);
    }

    /// Structs nested `levels` deep in field 1, the innermost holding a bool.
    fn nested(levels: usize) -> Vec<u8> {
        let mut s = vec![3, 0x22, 1, 0];
        for _ in 0..levels {
            let mut outer = Vec::new();
            var(s.len() + 2, &mut outer);
            outer.push(0x2a);
            outer.extend(s);
            outer.push(0);
            s = outer;
        }
        s
    }

    #[test]
    fn struct_with_scalars() {
        // Length 9: field 1 uint64 300, field 3 int32 -1, and field 40 bool, its id in
        // the next byte. Nullables are left to the roster tests.
        let b = [9u8, 0x26, 0xac, 0x02, 0x70, 0x01, 0xc2, 40, 1, 0];
        let v = parse(&b).unwrap();
        assert_eq!(v.field(1), Some(&Value::Uint(300)));
        assert_eq!(v.field(3), Some(&Value::Int(-1)));
        assert_eq!(v.field(40), Some(&Value::Bool(true)));
    }

    #[test]
    fn the_head_of_a_cut_document_keeps_its_leading_fields() {
        // field 1 uint 5; field 2 a list of 300 int8, of which only 3 bytes came.
        let b = [100u8, 0x25, 5, 0x4b, 0x0e, 0xac, 0x02, 7, 8, 9];
        assert!(parse(&b).is_err());
        let got = head(&b);
        assert_eq!(got, vec![(1, Value::Uint(5)), (2, Value::Bytes(vec![7, 8, 9]))]);
    }

    #[test]
    fn a_struct_that_overruns_is_an_error() {
        assert!(parse(&[200, 0]).is_err());
    }

    #[test]
    fn a_huge_list_count_is_an_error_not_an_allocation() {
        assert!(parse(&[8, 0x2b, 0x05, 0xff, 0xff, 0xff, 0xff, 0x0f, 0]).is_err());
    }

    #[test]
    fn a_count_reserves_room_for_only_a_few_elements() {
        // Field 1 is a list, then a map, of 100000 structs, the first of which overruns.
        // The padding lets the count pass the check against the bytes left.
        let n = 100_000;
        for container in [vec![0x2b, 0x0a], vec![0x2d, 0x0a, 0x0a]] {
            let mut b = vec![0];
            b.extend(container);
            var(n, &mut b);
            b.extend([0xff, 0xff, 0xff, 0xff, 0x0f]);
            b.resize(b.len() + n, 0);
            let (got, largest) = counting::largest(|| parse(&b));
            assert_eq!(got, Err("bond struct past end".to_string()));
            assert!(largest < 64 << 10, "reserved {largest} bytes");
        }
    }

    #[test]
    fn a_varint_of_eleven_bytes_is_an_error() {
        let ten = [[0xff; 9].as_slice(), &[0x01]].concat();
        assert_eq!(Reader { b: &ten, p: 0 }.var(), Ok(u64::MAX));
        let eleven = [[0xff; 10].as_slice(), &[0x01]].concat();
        assert_eq!(Reader { b: &eleven, p: 0 }.var(), Err("bond varint too long".to_string()));
    }

    #[test]
    fn a_string_that_runs_past_the_end_is_an_error() {
        assert_eq!(parse(&[4, 0x29, 5, b'a', 0]), Err("bond past end".to_string()));
    }

    #[test]
    fn structs_nested_past_the_limit_are_too_deep() {
        assert!(parse(&nested(32)).is_ok());
        assert_eq!(parse(&nested(33)), Err("bond too deep".to_string()));
    }
}
