//! A Fiddler capture (.saz) as feed items: each lobby WebSocket's server messages
//! and each Xbox session document, in session order. Frames and responses are read
//! with the tap's own parsing, so a capture decodes the same way as live traffic.

use crate::feed::{Item, Kind};
use crate::tap::{self, Out};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek};
use std::path::Path;
use std::str::FromStr;
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

/// The most bytes read from one entry, in case it inflates without end.
const MAX_ENTRY: u64 = 256 << 20;
/// The most bytes read for a request's first line.
const MAX_LINE: u64 = 64 << 10;

fn find(h: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    tap::find(h.get(from..)?, needle).map(|i| i + from)
}

/// The number in `s[at..at + len]`, which must be all digits.
fn digits<T: FromStr>(s: &str, at: usize, len: usize) -> Option<T> {
    let d = s.get(at..at.checked_add(len)?)?;
    d.bytes().all(|c| c.is_ascii_digit()).then(|| d.parse().ok())?
}

/// A time like `2025-03-01T21:04:05.1234567-08:00`: `Z` or an offset, and any
/// number of fractional digits.
fn timestamp(s: &str) -> Option<OffsetDateTime> {
    let b = s.as_bytes();
    let at = |i: usize, c: u8| b.get(i) == Some(&c);
    if !(at(4, b'-') && at(7, b'-') && at(10, b'T') && at(13, b':') && at(16, b':')) {
        return None;
    }
    let month = Month::try_from(digits::<u8>(s, 5, 2)?).ok()?;
    let date = Date::from_calendar_date(digits(s, 0, 4)?, month, digits(s, 8, 2)?).ok()?;
    let (mut p, mut nanos) = (19, 0);
    if at(19, b'.') {
        let n = b[20..].iter().take_while(|c| c.is_ascii_digit()).count();
        let kept = n.min(9);
        nanos = digits::<u32>(s, 20, kept)? * 10u32.pow(9 - kept as u32);
        p = 20 + n;
    }
    let time = Time::from_hms_nano(digits(s, 11, 2)?, digits(s, 14, 2)?, digits(s, 17, 2)?, nanos).ok()?;
    let offset = match s.get(p..)? {
        "Z" => UtcOffset::UTC,
        o if o.len() == 6 && at(p + 3, b':') => {
            let sign = match b[p] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            UtcOffset::from_hms(sign * digits::<i8>(s, p + 1, 2)?, sign * digits::<i8>(s, p + 4, 2)?, 0).ok()?
        }
        _ => return None,
    };
    PrimitiveDateTime::new(date, time).assume_offset(offset).checked_to_offset(UtcOffset::UTC)
}

/// Fiddler's local time as UTC, in the form `clock::iso` writes for the proxy. A
/// time that doesn't parse is kept as it is.
fn utc_of(s: &str) -> String {
    timestamp(s).map_or_else(|| s.to_string(), crate::clock::iso)
}

/// A Fiddler WebSocket log: blocks of header lines, then the bytes of one frame.
/// Returns the server's frames as (time, frame bytes).
fn server_records(w: &[u8]) -> Vec<(String, &[u8])> {
    let (mut p, mut out) = (0, Vec::new());
    loop {
        let req = find(w, b"Request-Length: ", p);
        let res = find(w, b"Response-Length: ", p);
        let (at, inbound) = match (req, res) {
            (Some(a), Some(b)) if a < b => (a, false),
            (_, Some(b)) => (b, true),
            (Some(a), None) => (a, false),
            (None, None) => break,
        };
        let Some(head_end) = find(w, b"\r\n\r\n", at) else { break };
        let head = String::from_utf8_lossy(&w[at..head_end]);
        let mut len = None;
        let mut time = String::new();
        for line in head.split("\r\n") {
            if let Some((k, v)) = line.split_once(": ") {
                match k {
                    "Request-Length" | "Response-Length" => len = v.trim().parse::<usize>().ok(),
                    "DoneRead" => time = utc_of(v.trim()),
                    _ => {}
                }
            }
        }
        let Some(n) = len else { break };
        let start = head_end + 4;
        let Some(frame) = start.checked_add(n).and_then(|end| w.get(start..end)) else { break };
        if inbound {
            out.push((time, frame));
        }
        p = start + n;
    }
    out
}

/// The data payloads in one record's WebSocket bytes. Control frames are left out.
fn payloads(b: &[u8]) -> Vec<u8> {
    let (mut out, mut p) = (Vec::new(), 0);
    while let Ok(Some((used, d))) = tap::frame(&b[p..]) {
        p += used;
        out.extend(d.unwrap_or_default());
    }
    out
}

/// A session document's body, as the proxy would keep it from the same response.
fn http_body(method: &str, raw: &[u8]) -> Option<Vec<u8>> {
    tap::document(method, raw).into_iter().find_map(|o| match o {
        Out::Item(i) => Some(i.payload),
        _ => None,
    })
}

/// An entry's bytes, up to `MAX_ENTRY`.
fn entry<R: Read + Seek>(z: &mut zip::ZipArchive<R>, name: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    z.by_name(name).ok()?.take(MAX_ENTRY).read_to_end(&mut out).ok()?;
    Some(out)
}

/// An entry's first line, without its line break.
fn first_line<R: Read + Seek>(z: &mut zip::ZipArchive<R>, name: &str) -> Option<String> {
    let mut line = Vec::new();
    BufReader::new(z.by_name(name).ok()?.take(MAX_LINE)).read_until(b'\n', &mut line).ok()?;
    let line = line.strip_suffix(b"\n").unwrap_or(&line);
    Some(String::from_utf8_lossy(line.strip_suffix(b"\r").unwrap_or(line)).into_owned())
}

pub fn read(path: &Path) -> Result<Vec<Item>, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let z = zip::ZipArchive::new(file).map_err(|e| format!("{}: not a capture: {e}", path.display()))?;
    Ok(items(z))
}

fn items<R: Read + Seek>(mut z: zip::ZipArchive<R>) -> Vec<Item> {
    // A name that can't be decoded isn't one of Fiddler's.
    let names: std::collections::HashSet<String> =
        z.file_names().filter_map(Result::ok).map(std::borrow::Cow::into_owned).collect();
    let mut ids: Vec<&str> = names.iter().filter_map(|n| n.strip_prefix("raw/")?.strip_suffix("_c.txt")).collect();
    ids.sort_by_key(|i| i.parse::<u64>().unwrap_or(u64::MAX));
    let mut items = Vec::new();
    for id in ids {
        let Some(line) = first_line(&mut z, &format!("raw/{id}_c.txt")) else { continue };
        let mut parts = line.split(' ');
        let (method, url) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        if method == "CONNECT" {
            continue;
        }
        let conn = id.parse::<u64>().unwrap_or(0);
        let ws = format!("raw/{id}_w.txt");
        if url.contains("lobby-hi") && names.contains(&ws) {
            let Some(w) = entry(&mut z, &ws) else { continue };
            for (at, frame) in server_records(&w) {
                items.push(Item { kind: Kind::WebSocket, conn, at, payload: payloads(frame) });
            }
        } else if url.contains("sessiondirectory.xboxlive.com") && url.contains("sessionTemplates") {
            let Some(s) = entry(&mut z, &format!("raw/{id}_s.txt")) else { continue };
            if let Some(payload) = http_body(method, &s) {
                items.push(Item { kind: Kind::Document, conn, at: String::new(), payload });
            }
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_keep_only_the_servers_side() {
        let mut w = b"Request-Length: 2\r\nID: 1\r\nDoneRead: a\r\n\r\nxx\r\n".to_vec();
        w.extend(b"Response-Length: 3\r\nID: 2\r\nDoneRead: b\r\n\r\nabc\r\n");
        let got = server_records(&w);
        assert_eq!(got, vec![("b".to_string(), &b"abc"[..])]);
    }

    #[test]
    fn a_length_inside_a_payload_is_not_a_record() {
        let mut w = b"Response-Length: 18\r\nDoneRead: a\r\n\r\nRequest-Length: 9\r".to_vec();
        w.extend(b"\n\r\nResponse-Length: 1\r\nDoneRead: b\r\n\r\nz");
        assert_eq!(server_records(&w).len(), 2);
    }

    #[test]
    fn payloads_unmask_and_skip_control_frames() {
        let masked = [0x82, 0x82, 1, 2, 3, 4, b'a' ^ 1, b'b' ^ 2];
        let ping = [0x89, 0x01, b'p'];
        assert_eq!(payloads(&[&ping[..], &masked[..]].concat()), b"ab");
    }

    #[test]
    fn chunked_gzip_is_undone() {
        use flate2::{write::GzEncoder, Compression};
        use std::io::Write;
        let mut e = GzEncoder::new(Vec::new(), Compression::default());
        e.write_all(b"{}").unwrap();
        let z = e.finish().unwrap();
        let mut raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Encoding: gzip\r\n\r\n".to_vec();
        raw.extend(format!("{:x}\r\n", z.len()).bytes());
        raw.extend(&z);
        raw.extend(b"\r\n0\r\n\r\n");
        assert_eq!(http_body("GET", &raw).unwrap(), b"{}");
    }

    #[test]
    fn a_record_length_that_overflows_ends_the_records() {
        assert!(server_records(b"Response-Length: 18446744073709551615\r\n\r\nabc").is_empty());
    }

    #[test]
    fn a_chunk_size_that_overflows_is_no_body() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffffffffffffffff\r\nabc\r\n0\r\n\r\n";
        assert_eq!(http_body("GET", raw), None);
    }

    #[test]
    fn fiddlers_times_become_utc_as_the_proxy_writes_them() {
        let w = b"Response-Length: 1\r\nDoneRead: 2025-03-01T21:04:05.1234567-08:00\r\n\r\nz";
        assert_eq!(server_records(w)[0].0, "2025-03-02T05:04:05.123Z");
        for (fiddler, utc) in [
            ("2025-03-01T21:04:05Z", "2025-03-01T21:04:05.000Z"),
            ("2025-03-01T21:04:05.5+05:30", "2025-03-01T15:34:05.500Z"),
            ("2025-12-31T23:59:59.9999999-00:30", "2026-01-01T00:29:59.999Z"),
            ("2025-03-01T21:04:05.123456789123+00:00", "2025-03-01T21:04:05.123Z"),
        ] {
            assert_eq!(utc_of(fiddler), utc, "{fiddler}");
        }
        for odd in [
            "",
            "b",
            "2025-03-01T21:04:05",
            "2025-02-30T21:04:05Z",
            "2025-03-01T21:04:05.-08:00",
            "2025-03-01 21:04:05Z",
        ] {
            assert_eq!(utc_of(odd), odd);
        }
        assert_eq!(utc_of("2025-03-01T21:04:05+1:00"), "2025-03-01T21:04:05+1:00");
    }

    /// A capture in memory, its entries deflated.
    fn capture(entries: &[(&str, Vec<u8>)]) -> zip::ZipArchive<std::io::Cursor<Vec<u8>>> {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, data) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(data).unwrap();
        }
        zip::ZipArchive::new(w.finish().unwrap()).unwrap()
    }

    #[test]
    fn a_capture_reads_as_the_proxy_reads_live_traffic() {
        use flate2::{write::ZlibEncoder, Compression};
        use std::io::Write;
        let docs = "https://sessiondirectory.xboxlive.com/serviceconfigs/x/sessionTemplates/t/sessions/s";
        let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
        e.write_all(b"{\"a\":1}").unwrap();
        let z = e.finish().unwrap();
        let mut deflated =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding:chunked\r\nContent-Encoding: deflate\r\n\r\n".to_vec();
        deflated.extend(format!("{:x}\r\n", z.len()).bytes());
        deflated.extend(&z);
        deflated.extend(b"\r\n0\r\n\r\n");
        // A request body far bigger than its first line, which is all that's read.
        let mut put = format!("PUT {docs} HTTP/1.1\r\nContent-Length: {}\r\n\r\n", 4 << 20).into_bytes();
        put.resize(put.len() + (4 << 20), b' ');
        let mut ws = b"Request-Length: 7\r\nDoneRead: 2025-03-01T21:04:05.1-08:00\r\n\r\n\x82\x81\0\0\0\0x".to_vec();
        ws.extend(b"Response-Length: 5\r\nDoneRead: 2025-03-01T21:04:06.5-08:00\r\n\r\n\x82\x03one");
        let get = |url: &str| format!("GET {url} HTTP/1.1\r\nHost: x\r\n\r\n").into_bytes();
        let z = capture(&[
            ("raw/1_c.txt", b"CONNECT sessiondirectory.xboxlive.com:443 HTTP/1.1\r\n\r\n".to_vec()),
            ("raw/2_c.txt", put),
            ("raw/2_s.txt", deflated),
            ("raw/3_c.txt", get("https://lobby-hi.svc.halowaypoint.com/hi/lobby")),
            ("raw/3_w.txt", ws),
            ("raw/4_c.txt", get(docs)),
            ("raw/4_s.txt", b"HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\n\r\n{}".to_vec()),
            ("raw/10_c.txt", get(docs)),
            (
                "raw/10_s.txt",
                b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\n{\"b\":2}".to_vec(),
            ),
        ]);
        let (got, largest) = crate::bond::counting::largest(|| items(z));
        let item = |kind, conn, at: &str, payload: &[u8]| Item { kind, conn, at: at.into(), payload: payload.to_vec() };
        assert_eq!(
            got,
            vec![
                item(Kind::Document, 2, "", b"{\"a\":1}"),
                item(Kind::WebSocket, 3, "2025-03-02T05:04:06.500Z", b"one"),
                item(Kind::Document, 10, "", b"{\"b\":2}"),
            ]
        );
        assert!(largest < 1 << 20, "allocated {largest} bytes");
    }
}
