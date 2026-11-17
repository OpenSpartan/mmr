//! AMQP 1.0 from the server's side of a lobby WebSocket, read incrementally: feed it
//! each message's payload as it arrives, and it returns the rosters that complete.
//!
//! Only the server's direction is read. The client's carries the auth tokens.

use crate::{bond, roster};
use std::collections::{BTreeMap, HashMap};

type R<T> = Result<T, &'static str>;

const MAX_FRAME: usize = 1 << 24;
const ATTACH: u8 = 0x12;
const TRANSFER: u8 = 0x14;
const DATA: u8 = 0x75;

#[derive(Debug, PartialEq)]
enum V {
    Null,
    Bool(bool),
    Num(u64),
    Text(String),
    List(Vec<V>),
    Other,
}

fn take<'a>(b: &'a [u8], p: &mut usize, n: usize) -> R<&'a [u8]> {
    let end = p.checked_add(n).ok_or("amqp length")?;
    let s = b.get(*p..end).ok_or("amqp short")?;
    *p = end;
    Ok(s)
}

fn be(s: &[u8]) -> u64 {
    s.iter().fold(0u64, |a, x| a << 8 | u64::from(*x))
}

fn value(b: &[u8], p: &mut usize, depth: u32) -> R<V> {
    if depth > 32 {
        return Err("amqp too deep");
    }
    let c = take(b, p, 1)?[0];
    Ok(match c {
        0x00 => {
            value(b, p, depth + 1)?; // the descriptor
            return value(b, p, depth + 1);
        }
        0x40 => V::Null,
        0x41 => V::Bool(true),
        0x42 => V::Bool(false),
        0x43 | 0x44 => V::Num(0),
        0x45 => V::List(vec![]),
        0x56 => V::Bool(take(b, p, 1)?[0] != 0),
        0x50..=0x55 => V::Num(u64::from(take(b, p, 1)?[0])),
        0x60 | 0x61 => V::Num(be(take(b, p, 2)?)),
        0x70..=0x74 => V::Num(be(take(b, p, 4)?)),
        0x80..=0x84 => V::Num(be(take(b, p, 8)?)),
        0x98 => {
            take(b, p, 16)?;
            V::Other
        }
        0xa0 | 0xa1 | 0xa3 => {
            let n = take(b, p, 1)?[0] as usize;
            V::Text(String::from_utf8_lossy(take(b, p, n)?).into_owned())
        }
        0xb0 | 0xb1 | 0xb3 => {
            let n = be(take(b, p, 4)?) as usize;
            V::Text(String::from_utf8_lossy(take(b, p, n)?).into_owned())
        }
        0xc0 | 0xc1 | 0xd0 | 0xd1 => {
            let (size, count, width) = if c < 0xd0 {
                let s = take(b, p, 2)?;
                (s[0] as usize, s[1] as usize, 1)
            } else {
                let s = take(b, p, 8)?;
                (be(&s[..4]) as usize, be(&s[4..]) as usize, 4)
            };
            let end = p.checked_add(size.checked_sub(width).ok_or("amqp size")?).ok_or("amqp size")?;
            // The elements stay within the list's size, so `p` only moves forward.
            let inner = b.get(..end).ok_or("amqp short")?;
            let mut out = Vec::new();
            for _ in 0..count {
                out.push(value(inner, p, depth + 1)?);
            }
            *p = end;
            V::List(out)
        }
        0xe0 => {
            let n = take(b, p, 1)?[0] as usize;
            take(b, p, n)?;
            V::Other
        }
        0xf0 => {
            let n = be(take(b, p, 4)?) as usize;
            take(b, p, n)?;
            V::Other
        }
        _ => return Err("amqp constructor"),
    })
}

/// The Data sections of a message, joined.
fn data_sections(b: &[u8]) -> R<Vec<u8>> {
    let (mut p, mut out) = (0, Vec::new());
    while p < b.len() {
        let h = take(b, &mut p, 3)?;
        if h[..2] != [0x00, 0x53] {
            return Err("amqp section");
        }
        if h[2] == DATA {
            let n = match take(b, &mut p, 1)?[0] {
                0xa0 => take(b, &mut p, 1)?[0] as usize,
                0xb0 => be(take(b, &mut p, 4)?) as usize,
                _ => return Err("amqp data"),
            };
            out.extend_from_slice(take(b, &mut p, n)?);
        } else {
            value(b, &mut p, 0)?;
        }
    }
    Ok(out)
}

fn session_of(addr: &str) -> Option<String> {
    let at = addr.find("/hi/gamesession/")? + "/hi/gamesession/".len();
    let id = addr.get(at..at + 36)?;
    let ok = id
        .bytes()
        .enumerate()
        .all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { c == b'-' } else { c.is_ascii_hexdigit() });
    ok.then(|| id.to_ascii_lowercase())
}

fn owner_of(addr: &str) -> Option<u64> {
    let rest = &addr[addr.find("xuid(")? + 5..];
    rest[..rest.find(')')?].parse().ok()
}

/// One link: its game session, or `None` for a link that isn't a game session's.
struct Link {
    session: Option<String>,
    owner: Option<u64>,
}

#[derive(Default)]
struct Conn {
    buf: Vec<u8>,
    links: HashMap<(u16, u32), Link>,
    /// Each link's delivery so far, or `None` for one dropped as too big.
    pending: HashMap<(u16, u32), Option<Vec<u8>>>,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub session: String,
    pub owner: Option<u64>,
    pub at: String,
    pub roster: roster::Roster,
}

#[derive(Default)]
pub struct Decoder {
    conns: HashMap<u64, Conn>,
    pub problems: BTreeMap<String, u64>,
}

impl Decoder {
    fn note(problems: &mut BTreeMap<String, u64>, what: &str) {
        *problems.entry(what.to_string()).or_default() += 1;
    }

    /// One WebSocket message's payload from the server on connection `conn`.
    pub fn feed(&mut self, conn: u64, at: &str, payload: &[u8]) -> Vec<Message> {
        let c = self.conns.entry(conn).or_default();
        c.buf.extend_from_slice(payload);
        let (mut p, mut out) = (0, Vec::new());
        let mut buf = std::mem::take(&mut c.buf);
        while p + 8 <= buf.len() {
            if &buf[p..p + 4] == b"AMQP" {
                p += 8;
                continue;
            }
            let n = u32::from_be_bytes(buf[p..p + 4].try_into().unwrap()) as usize;
            let doff = buf[p + 4] as usize * 4;
            if !(8..=MAX_FRAME).contains(&n) || doff < 8 || doff > n {
                // Not on a frame boundary: start again at the next message.
                Self::note(&mut self.problems, "lost frame alignment");
                p = buf.len();
                break;
            }
            if p + n > buf.len() {
                break;
            }
            let channel = u16::from_be_bytes([buf[p + 6], buf[p + 7]]);
            Self::frame(c, &mut self.problems, channel, &buf[p + doff..p + n], at, &mut out);
            p += n;
        }
        buf.drain(..p);
        c.buf = buf;
        out
    }

    /// Connection `conn` closed: forgets its links and what it buffered.
    pub fn close(&mut self, conn: u64) {
        self.conns.remove(&conn);
    }

    fn frame(
        c: &mut Conn,
        problems: &mut BTreeMap<String, u64>,
        channel: u16,
        f: &[u8],
        at: &str,
        out: &mut Vec<Message>,
    ) {
        if f.len() < 3 || f[..2] != [0x00, 0x53] || (f[2] != ATTACH && f[2] != TRANSFER) {
            return;
        }
        let mut q = 3;
        let Ok(V::List(v)) = value(f, &mut q, 0) else {
            return Self::note(problems, "performative unreadable");
        };
        let Some(V::Num(handle)) = v.get(usize::from(f[2] == ATTACH)) else {
            return Self::note(problems, "performative without a handle");
        };
        let Ok(handle) = u32::try_from(*handle) else {
            return Self::note(problems, "a handle over 32 bits");
        };
        let key = (channel, handle);
        if f[2] == ATTACH {
            let addr = v.iter().skip(5).take(2).find_map(|x| match x {
                V::List(l) => match l.first() {
                    Some(V::Text(t)) if t.contains("gamesession") => Some(t.clone()),
                    _ => None,
                },
                _ => None,
            });
            let link =
                Link { session: addr.as_deref().and_then(session_of), owner: addr.as_deref().and_then(owner_of) };
            c.links.insert(key, link);
            c.pending.remove(&key);
            return;
        }
        // Transfer fields: more is 5, resume 8, aborted 9.
        let more = matches!(v.get(5), Some(V::Bool(true)));
        let aborted = matches!(v.get(9), Some(V::Bool(true)));
        if aborted {
            c.pending.remove(&key);
            return;
        }
        let link = c.links.get(&key);
        if matches!(link, Some(Link { session: None, .. })) {
            return; // another service's link
        }
        let slot = c.pending.entry(key).or_insert_with(|| Some(Vec::new()));
        if let Some(d) = slot {
            if d.len() + (f.len() - q) > MAX_FRAME {
                // The rest of its transfers are skipped, up to the last.
                *slot = None;
                Self::note(problems, "a delivery over 16 MB");
            } else {
                d.extend_from_slice(&f[q..]);
            }
        }
        if more {
            return;
        }
        if let Some(body) = c.pending.remove(&key).flatten() {
            Self::message(link, problems, &body, at, out);
        }
    }

    fn message(
        link: Option<&Link>,
        problems: &mut BTreeMap<String, u64>,
        body: &[u8],
        at: &str,
        out: &mut Vec<Message>,
    ) {
        let parsed = data_sections(body).map_err(str::to_string).and_then(|d| bond::parse(&d));
        let root = match (parsed, link) {
            (Ok(v), _) => v,
            (Err(_), Some(_)) => return Self::note(problems, "game session message unreadable"),
            (Err(_), None) => return,
        };
        let mut warnings: Vec<&'static str> = Vec::new();
        let roster = roster::read(&root, &mut |w| warnings.push(w));
        for w in warnings {
            Self::note(problems, w);
        }
        let roster = match roster {
            Ok(Some(r)) => r,
            Ok(None) => return,
            Err(e) => return Self::note(problems, e),
        };
        let (session, owner) = match link {
            Some(l) => (l.session.clone().unwrap_or_default(), l.owner),
            None => {
                // The attach came before mmr was watching. The match id still places it.
                if roster.match_id.is_none() {
                    Self::note(problems, "roster with no match id on a link whose attach was missed");
                }
                (String::new(), None)
            }
        };
        out.push(Message { session, owner, at: at.to_string(), roster });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0a1b2c3d-0000-4000-8000-0123456789ab";

    fn frame(channel: u16, body: &[u8]) -> Vec<u8> {
        let mut f = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        f.extend([2, 0]);
        f.extend(channel.to_be_bytes());
        f.extend(body);
        f
    }

    fn string(s: &str) -> Vec<u8> {
        let mut v = vec![0xa1, s.len() as u8];
        v.extend(s.as_bytes());
        v
    }

    fn list(items: &[Vec<u8>]) -> Vec<u8> {
        let inner: Vec<u8> = items.concat();
        let mut v = vec![0xc0, (inner.len() + 1) as u8, items.len() as u8];
        v.extend(inner);
        v
    }

    fn attach(channel: u16, handle: u8) -> Vec<u8> {
        let addr = format!("/hi/gamesession/{ID}?AuthModel=PerLinkAuth&Players=xuid(9100000000000003)");
        let source = [vec![0x00, 0x53, 0x28], list(&[string(&addr)])].concat();
        let perf = list(&[string("link"), vec![0x52, handle], vec![0x42], vec![0x40], vec![0x40], source, vec![0x40]]);
        frame(channel, &[vec![0x00, 0x53, ATTACH], perf].concat())
    }

    /// A transfer whose eleven fields are null except the handle and those in `set`.
    fn transfer_with(channel: u16, handle: Vec<u8>, set: &[(usize, u8)], data: &[u8]) -> Vec<u8> {
        let mut fields = vec![vec![0x40]; 11];
        fields[0] = handle;
        for (i, v) in set {
            fields[*i] = vec![*v];
        }
        frame(channel, &[vec![0x00, 0x53, TRANSFER], list(&fields), data.to_vec()].concat())
    }

    fn transfer(channel: u16, handle: u8, more: bool, data: &[u8]) -> Vec<u8> {
        transfer_with(channel, vec![0x52, handle], &[(5, if more { 0x41 } else { 0x42 })], data)
    }

    /// A Data section holding a game session message: { 10: [ { 8: { XUID: { 4: mmr } } } ] }.
    fn state(mmr: f64) -> Vec<u8> {
        let mut entry = vec![0x88];
        entry.extend(mmr.to_le_bytes());
        entry.push(0);
        let mut e = vec![entry.len() as u8];
        e.extend(entry);
        let xuid: u64 = 0x0009_0000_1234_5678;
        let mut key = Vec::new();
        let mut x = xuid;
        loop {
            let b = (x & 127) as u8;
            x >>= 7;
            if x == 0 {
                key.push(b);
                break;
            }
            key.push(b | 128);
        }
        let mut map = vec![0x0d | (6 << 5), 8, 6, 10, 1];
        map.extend(key);
        map.extend(e);
        map.push(0);
        let mut ms = vec![map.len() as u8];
        ms.extend(map);
        let mut root = vec![(6 << 5) | 11, 10, (2 << 5) | 10];
        root.extend(ms);
        root.push(0);
        let mut out = vec![root.len() as u8];
        out.extend(root);
        let mut d = vec![0x00, 0x53, DATA, 0xa0, out.len() as u8];
        d.extend(out);
        d
    }

    #[test]
    fn a_roster_split_across_websocket_messages() {
        let mut dec = Decoder::default();
        assert!(dec.feed(1, "t0", &attach(0, 3)).is_empty());
        let t = transfer(0, 3, false, &state(1.5));
        assert!(dec.feed(1, "t1", &t[..10]).is_empty());
        let got = dec.feed(1, "t2", &t[10..]);
        assert_eq!(got.len(), 1, "{:?}", dec.problems);
        assert_eq!(got[0].session, ID);
        assert_eq!(got[0].owner, Some(9100000000000003));
        assert_eq!(got[0].at, "t2");
        assert_eq!(got[0].roster.entries[0].mmr, Some(1.5));
    }

    #[test]
    fn a_delivery_split_across_transfers() {
        let mut dec = Decoder::default();
        dec.feed(1, "t", &attach(0, 3));
        let d = state(2.0);
        assert!(dec.feed(1, "t", &transfer(0, 3, true, &d[..9])).is_empty());
        assert_eq!(dec.feed(1, "t", &transfer(0, 3, false, &d[9..])).len(), 1);
    }

    #[test]
    fn the_same_handle_on_another_channel_is_another_link() {
        let mut dec = Decoder::default();
        dec.feed(1, "t", &attach(0, 3));
        let other = frame(1, &[vec![0x00, 0x53, ATTACH], list(&[string("x"), vec![0x52, 3]])].concat());
        dec.feed(1, "t", &other);
        assert!(dec.feed(1, "t", &transfer(1, 3, false, &state(1.0))).is_empty());
        assert_eq!(dec.feed(1, "t", &transfer(0, 3, false, &state(1.0))).len(), 1);
    }

    #[test]
    fn connections_do_not_share_links() {
        let mut dec = Decoder::default();
        dec.feed(1, "t", &attach(0, 3));
        let got = dec.feed(2, "t", &transfer(0, 3, false, &state(1.0)));
        assert_eq!(got[0].session, "");
        assert_eq!(dec.problems["roster with no match id on a link whose attach was missed"], 1);
    }

    #[test]
    fn garbage_resynchronizes_at_the_next_message() {
        let mut dec = Decoder::default();
        dec.feed(1, "t", &attach(0, 3));
        dec.feed(1, "t", &[0xff; 40]);
        assert_eq!(dec.problems["lost frame alignment"], 1);
        assert_eq!(dec.feed(1, "t", &transfer(0, 3, false, &state(1.0))).len(), 1);
    }

    #[test]
    fn a_list_reads_its_elements_only_from_within_its_size() {
        // Each declares room for one byte of elements, then holds a string of five.
        let short = [0xc0, 0x02, 0x01, 0xa1, 0x03, b'a', b'b', b'c'];
        assert_eq!(value(&short, &mut 0, 0), Err("amqp short"));
        let long = [[0xd0, 0, 0, 0, 5, 0, 0, 0, 1].as_slice(), &[0xa1, 0x03, b'a', b'b', b'c']].concat();
        assert_eq!(value(&long, &mut 0, 0), Err("amqp short"));
        // Padding after the elements is skipped.
        let mut p = 0;
        assert_eq!(value(&[0xc0, 0x03, 0x01, 0x41, 0x00, 0x40], &mut p, 0), Ok(V::List(vec![V::Bool(true)])));
        assert_eq!(p, 5);
    }

    #[test]
    fn nesting_and_sizes_that_cant_be_are_errors() {
        assert_eq!(value(&[0; 40], &mut 0, 0), Err("amqp too deep"));
        assert_eq!(value(&[0xc0, 0x00, 0x00], &mut 0, 0), Err("amqp size"));
    }

    #[test]
    fn an_abort_drops_the_delivery_and_a_resume_does_not() {
        let mut dec = Decoder::default();
        dec.feed(1, "t", &attach(0, 3));
        let d = state(2.0);
        dec.feed(1, "t", &transfer(0, 3, true, &d[..9]));
        assert!(dec.feed(1, "t", &transfer_with(0, vec![0x52, 3], &[(9, 0x41)], &[])).is_empty());
        assert!(dec.problems.is_empty(), "{:?}", dec.problems);
        assert_eq!(dec.feed(1, "t", &transfer(0, 3, false, &d)).len(), 1);
        dec.feed(1, "t", &transfer(0, 3, true, &d[..9]));
        assert_eq!(dec.feed(1, "t", &transfer_with(0, vec![0x52, 3], &[(8, 0x41)], &d[9..])).len(), 1);
        assert!(dec.problems.is_empty(), "{:?}", dec.problems);
    }

    #[test]
    fn a_delivery_over_the_frame_limit_is_dropped() {
        let mut dec = Decoder::default();
        dec.feed(1, "t", &attach(0, 3));
        // The second goes over; the third is the rest of the same delivery.
        let half = vec![0; MAX_FRAME / 2 + 1];
        for more in [true, true, false] {
            assert!(dec.feed(1, "t", &transfer(0, 3, more, &half)).is_empty());
        }
        assert_eq!(dec.problems.keys().collect::<Vec<_>>(), ["a delivery over 16 MB"]);
        assert!(dec.conns[&1].pending.is_empty());
        assert_eq!(dec.feed(1, "t", &transfer(0, 3, false, &state(1.0))).len(), 1);
    }

    #[test]
    fn a_handle_over_32_bits_is_not_another_link() {
        let mut dec = Decoder::default();
        dec.feed(1, "t", &attach(0, 0));
        let wide = [[0x80].as_slice(), &(1u64 << 32).to_be_bytes()].concat();
        assert!(dec.feed(1, "t", &transfer_with(0, wide, &[], &state(1.0))).is_empty());
        assert_eq!(dec.problems["a handle over 32 bits"], 1);
    }

    #[test]
    fn a_frame_in_pieces_is_not_copied_for_each_piece() {
        let mut dec = Decoder::default();
        let t = transfer(0, 3, false, &vec![0; 1 << 20]);
        dec.feed(1, "t", &t[..t.len() - 1]);
        let (_, largest) = bond::counting::largest(|| dec.feed(1, "t", &[]));
        assert!(largest < 1 << 10, "allocated {largest} bytes");
    }

    #[test]
    fn a_protocol_header_and_heartbeats_are_skipped() {
        let mut dec = Decoder::default();
        let heartbeat = [0, 0, 0, 8, 2, 0, 0, 0];
        let stream = [b"AMQP\x00\x01\x00\x00".as_slice(), &heartbeat, &attach(0, 3), &heartbeat].concat();
        assert!(dec.feed(1, "t", &stream).is_empty());
        assert_eq!(dec.feed(1, "t", &transfer(0, 3, false, &state(1.0)))[0].session, ID);
        assert!(dec.problems.is_empty(), "{:?}", dec.problems);
    }

    #[test]
    fn a_closed_connection_is_forgotten() {
        let mut dec = Decoder::default();
        dec.feed(1, "t", &attach(0, 3));
        dec.feed(2, "t", &attach(0, 3));
        dec.close(1);
        assert_eq!(dec.conns.keys().collect::<Vec<_>>(), [&2]);
        assert_eq!(dec.feed(1, "t", &transfer(0, 3, false, &state(1.0)))[0].session, "");
    }

    #[test]
    fn a_data_offset_past_the_frame_loses_alignment() {
        let mut dec = Decoder::default();
        dec.feed(1, "t", &[0, 0, 0, 8, 3, 0, 0, 0]);
        assert_eq!(dec.problems["lost frame alignment"], 1);
    }
}
