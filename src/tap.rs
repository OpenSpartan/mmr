//! Reads a copy of one decrypted connection's HTTP/1.1 as it passes through the
//! proxy. The proxy forwards every byte unchanged whatever happens here, so traffic
//! the tap can't parse only stops the copy.
//!
//! On the client's side it reads only the framing (method, target, body length,
//! upgrade), to pair each response with its request. The client's headers carry the
//! sign-in tokens, and their values are never kept. On the server's side it keeps the
//! lobby WebSocket's messages and the bodies of Xbox session documents.

use crate::feed::{Item, Kind};
use flate2::read::{GzDecoder, ZlibDecoder};
use std::collections::VecDeque;
use std::io::Read;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

const MAX_HEAD: usize = 64 << 10;
const MAX_BODY: usize = 16 << 20;
const MAX_FRAME: usize = 16 << 20;

/// Which connection this is, by the host the client asked for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Host {
    Lobby,
    SessionDirectory,
}

impl Host {
    /// One of `cert::HOSTS`, or `None` for every other host.
    pub fn of(host: &str) -> Option<Host> {
        match host.to_ascii_lowercase().as_str() {
            h if h == crate::cert::HOSTS[0] => Some(Host::Lobby),
            h if h == crate::cert::HOSTS[1] => Some(Host::SessionDirectory),
            _ => None,
        }
    }
}

/// What the proxy and its taps pass on to the console.
#[derive(Debug)]
pub enum Out {
    Item(Item),
    /// Something the copy couldn't follow. The traffic itself still flows.
    Problem(String),
    /// A decrypted connection opened or closed. Closed has the same id as the
    /// connection's items, which all arrive before it.
    Opened(Host),
    Closed(Host, u64),
}

#[derive(Debug, Clone)]
struct Request {
    head: bool,
    upgrade: bool,
    document: bool,
}

type Queue = Arc<Mutex<VecDeque<Request>>>;

pub(crate) fn find(h: &[u8], needle: &[u8]) -> Option<usize> {
    h.windows(needle.len()).position(|w| w == needle)
}

/// The value of the header `name`, matched without case. Only framing headers are asked for.
fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.split("\r\n").skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

fn chunked(head: &str) -> bool {
    header(head, "transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
}

/// A body being skipped or kept.
#[derive(Debug)]
enum Body {
    Length(usize),
    Chunked(Chunk),
    UntilClose,
}

#[derive(Debug)]
enum Chunk {
    Size,
    Data(usize),
    DataEnd,
    Trailer,
}

/// The body a message head announces, or `None` when it has none. A response with
/// neither length nor chunks runs until the server closes; a request has no body.
fn body_of(head: &str, response: bool) -> Result<Option<Body>, ()> {
    if chunked(head) {
        return Ok(Some(Body::Chunked(Chunk::Size)));
    }
    match header(head, "content-length") {
        Some(v) => match v.parse::<usize>().map_err(|_| ())? {
            0 => Ok(None),
            n => Ok(Some(Body::Length(n))),
        },
        None if response => Ok(Some(Body::UntilClose)),
        None => Ok(None),
    }
}

/// What's kept of a body: nothing, a document's bytes, or a document left out as too big.
#[derive(Debug)]
enum Keep {
    Not,
    Bytes(Vec<u8>),
    Over,
}

const OVER: &str = "an Xbox document over 16 MB, left out";

/// Steps through a body. Returns how many bytes it used and whether the body ended.
fn body_step(body: &mut Body, b: &[u8], keep: &mut Keep) -> Result<(usize, bool), ()> {
    let mut keep_bytes = |s: &[u8]| {
        if let Keep::Bytes(k) = keep {
            if k.len() + s.len() > MAX_BODY {
                *keep = Keep::Over;
            } else {
                k.extend_from_slice(s);
            }
        }
    };
    match body {
        Body::Length(n) => {
            let take = (*n).min(b.len());
            keep_bytes(&b[..take]);
            *n -= take;
            Ok((take, *n == 0))
        }
        Body::UntilClose => {
            keep_bytes(b);
            Ok((b.len(), false))
        }
        Body::Chunked(c) => {
            let mut p = 0;
            loop {
                match c {
                    Chunk::Size => {
                        let Some(e) = find(&b[p..], b"\r\n") else {
                            return if b.len() - p > 1024 { Err(()) } else { Ok((p, false)) };
                        };
                        let line = std::str::from_utf8(&b[p..p + e]).map_err(|_| ())?;
                        let n =
                            usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16).map_err(|_| ())?;
                        p += e + 2;
                        *c = if n == 0 { Chunk::Trailer } else { Chunk::Data(n) };
                    }
                    Chunk::Data(n) => {
                        let take = (*n).min(b.len() - p);
                        if take == 0 {
                            return Ok((p, false));
                        }
                        keep_bytes(&b[p..p + take]);
                        p += take;
                        *n -= take;
                        if *n == 0 {
                            *c = Chunk::DataEnd;
                        }
                    }
                    Chunk::DataEnd => {
                        if b.len() - p < 2 {
                            return Ok((p, false));
                        }
                        if &b[p..p + 2] != b"\r\n" {
                            return Err(());
                        }
                        p += 2;
                        *c = Chunk::Size;
                    }
                    Chunk::Trailer => {
                        let Some(e) = find(&b[p..], b"\r\n") else {
                            return if b.len() - p > MAX_HEAD { Err(()) } else { Ok((p, false)) };
                        };
                        p += e + 2;
                        if e == 0 {
                            return Ok((p, true));
                        }
                    }
                }
            }
        }
    }
}

/// Undoes the body's content encoding, or says why it can't.
fn decoded(body: Vec<u8>, encoding: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    // A byte past the limit tells a document that's over it from one that fits.
    let cap = MAX_BODY as u64 + 1;
    let read = match encoding {
        "" | "identity" => return Ok(body),
        "gzip" | "x-gzip" => GzDecoder::new(&body[..]).take(cap).read_to_end(&mut out),
        "deflate" => ZlibDecoder::new(&body[..]).take(cap).read_to_end(&mut out),
        _ => return Err(format!("an Xbox document mmr can't decode ({encoding})")),
    };
    match read {
        Err(_) => Err(format!("an Xbox document mmr can't decode ({encoding})")),
        Ok(_) if out.len() > MAX_BODY => Err(OVER.into()),
        Ok(_) => Ok(out),
    }
}

/// The bytes a WebSocket frame used, and its payload if it's a data frame.
type Frame = (usize, Option<Vec<u8>>);

/// One WebSocket frame from the start of `b`, once it's all there.
pub(crate) fn frame(b: &[u8]) -> Result<Option<Frame>, String> {
    if b.len() < 2 {
        return Ok(None);
    }
    let opcode = b[0] & 15;
    let masked = b[1] & 128 != 0;
    let mut n = u64::from(b[1] & 127);
    let mut p = 2;
    if n == 126 {
        let Some(s) = b.get(2..4) else { return Ok(None) };
        n = u64::from(u16::from_be_bytes([s[0], s[1]]));
        p = 4;
    } else if n == 127 {
        let Some(s) = b.get(2..10) else { return Ok(None) };
        n = u64::from_be_bytes(s.try_into().unwrap());
        p = 10;
    }
    if n > MAX_FRAME as u64 {
        return Err("a WebSocket frame over 16 MB".into());
    }
    let mut key = [0u8; 4];
    if masked {
        let Some(s) = b.get(p..p + 4) else { return Ok(None) };
        key.copy_from_slice(s);
        p += 4;
    }
    let end = p + n as usize;
    let Some(d) = b.get(p..end) else { return Ok(None) };
    // Control frames are left out.
    let payload = (opcode < 8 && !d.is_empty()).then(|| {
        if masked {
            d.iter().enumerate().map(|(i, x)| x ^ key[i % 4]).collect()
        } else {
            d.to_vec()
        }
    });
    Ok(Some((end, payload)))
}

/// A tap on one connection: feed it the client's bytes before they go upstream, and
/// the server's bytes as they come back.
pub fn new(host: Host, conn: u64, out: Sender<Out>) -> (ClientSide, ServerSide) {
    let queue = Queue::default();
    (
        ClientSide { host, queue: queue.clone(), buf: Vec::new(), state: ClientState::Head },
        ServerSide { host, conn, queue, out, buf: Vec::new(), state: ServerState::Head },
    )
}

/// Reads one whole response to a session document request from a capture, the same way
/// as a live one, and gives what the tap would pass on: the document, or a problem.
pub(crate) fn document(method: &str, response: &[u8]) -> Vec<Out> {
    let (tx, rx) = std::sync::mpsc::channel();
    let (c, mut s) = new(Host::SessionDirectory, 0, tx);
    let request = Request { head: method.eq_ignore_ascii_case("HEAD"), upgrade: false, document: true };
    c.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push_back(request);
    s.feed(response);
    s.close();
    rx.try_iter().collect()
}

enum ClientState {
    Head,
    Body(Body),
    Stopped,
}

pub struct ClientSide {
    host: Host,
    queue: Queue,
    buf: Vec<u8>,
    state: ClientState,
}

impl ClientSide {
    pub fn feed(&mut self, b: &[u8]) {
        if matches!(self.state, ClientState::Stopped) {
            return;
        }
        self.buf.extend_from_slice(b);
        let mut p = 0;
        while p < self.buf.len() {
            match &mut self.state {
                ClientState::Head => {
                    let Some(e) = find(&self.buf[p..], b"\r\n\r\n") else {
                        if self.buf.len() - p > MAX_HEAD {
                            self.state = ClientState::Stopped;
                        }
                        break;
                    };
                    let head = String::from_utf8_lossy(&self.buf[p..p + e]);
                    let mut line = head.split("\r\n").next().unwrap_or("").split(' ');
                    let (method, target) = (line.next().unwrap_or(""), line.next().unwrap_or(""));
                    let upgrade = header(&head, "upgrade").is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
                    let request = Request {
                        head: method.eq_ignore_ascii_case("HEAD"),
                        upgrade,
                        document: self.host == Host::SessionDirectory && target.contains("sessionTemplates"),
                    };
                    let body = body_of(&head, false);
                    drop(head);
                    p += e + 4;
                    self.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push_back(request);
                    self.state = match body {
                        // After an upgrade, the client sends WebSocket frames, which aren't read.
                        _ if upgrade => ClientState::Stopped,
                        Ok(Some(b)) => ClientState::Body(b),
                        Ok(None) => ClientState::Head,
                        Err(()) => ClientState::Stopped,
                    };
                }
                ClientState::Body(body) => match body_step(body, &self.buf[p..], &mut Keep::Not) {
                    Ok((used, done)) => {
                        p += used;
                        if done {
                            self.state = ClientState::Head;
                        } else if used == 0 {
                            break;
                        }
                    }
                    Err(()) => self.state = ClientState::Stopped,
                },
                ClientState::Stopped => break,
            }
        }
        if matches!(self.state, ClientState::Stopped) {
            self.buf = Vec::new();
        } else {
            self.buf.drain(..p);
        }
    }
}

enum ServerState {
    Head,
    Body { body: Body, keep: Keep, encoding: String },
    WebSocket,
    Stopped,
}

pub struct ServerSide {
    host: Host,
    conn: u64,
    queue: Queue,
    out: Sender<Out>,
    buf: Vec<u8>,
    state: ServerState,
}

impl ServerSide {
    fn problem(&self, what: String) {
        let _ = self.out.send(Out::Problem(what));
    }

    fn stop(&mut self, why: &str) {
        self.problem(format!("stopped reading a connection: {why}"));
        self.state = ServerState::Stopped;
    }

    fn send(&self, kind: Kind, payload: Vec<u8>) {
        let _ = self.out.send(Out::Item(Item { kind, conn: self.conn, at: crate::clock::now(), payload }));
    }

    /// A response body ended. Sends it if it's a document being kept.
    fn finish(&self, keep: Keep, encoding: &str) {
        match keep {
            Keep::Not => {}
            Keep::Over => self.problem(OVER.into()),
            Keep::Bytes(body) => match decoded(body, encoding) {
                Ok(d) => self.send(Kind::Document, d),
                Err(why) => self.problem(why),
            },
        }
    }

    /// One response head. Returns how the bytes after it are read.
    fn head(&self, head: &str) -> Result<ServerState, String> {
        let code = head.split(' ').nth(1).and_then(|c| c.parse::<u16>().ok()).ok_or("a response without a status")?;
        if (100..200).contains(&code) && code != 101 {
            return Ok(ServerState::Head);
        }
        let request = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .ok_or("a response without a request")?;
        if code == 101 {
            if !request.upgrade || self.host != Host::Lobby {
                return Ok(ServerState::Stopped);
            }
            if header(head, "sec-websocket-extensions").is_some_and(|v| v.to_ascii_lowercase().contains("deflate")) {
                return Err("the lobby WebSocket is compressed".into());
            }
            return Ok(ServerState::WebSocket);
        }
        if request.head || code == 204 || code == 304 {
            return Ok(ServerState::Head);
        }
        let keep = if request.document && (200..300).contains(&code) { Keep::Bytes(Vec::new()) } else { Keep::Not };
        let encoding = header(head, "content-encoding").unwrap_or("").to_ascii_lowercase();
        match body_of(head, true) {
            Ok(Some(body)) => Ok(ServerState::Body { body, keep, encoding }),
            Ok(None) => {
                self.finish(keep, &encoding);
                Ok(ServerState::Head)
            }
            Err(()) => Err("a response length that can't be read".into()),
        }
    }

    pub fn feed(&mut self, b: &[u8]) {
        if matches!(self.state, ServerState::Stopped) {
            return;
        }
        self.buf.extend_from_slice(b);
        let mut p = 0;
        while p < self.buf.len() {
            match &mut self.state {
                ServerState::Head => {
                    let Some(e) = find(&self.buf[p..], b"\r\n\r\n") else {
                        if self.buf.len() - p > MAX_HEAD {
                            self.stop("a response head over 64 KB");
                        }
                        break;
                    };
                    let head = String::from_utf8_lossy(&self.buf[p..p + e]);
                    let next = self.head(&head);
                    drop(head);
                    p += e + 4;
                    match next {
                        Ok(s) => self.state = s,
                        Err(why) => self.stop(&why),
                    }
                }
                ServerState::Body { body, keep, .. } => match body_step(body, &self.buf[p..], keep) {
                    Ok((used, done)) => {
                        p += used;
                        if done {
                            if let ServerState::Body { keep, encoding, .. } =
                                std::mem::replace(&mut self.state, ServerState::Head)
                            {
                                self.finish(keep, &encoding);
                            }
                        } else if used == 0 {
                            break;
                        }
                    }
                    Err(()) => self.stop("a chunked body that can't be read"),
                },
                ServerState::WebSocket => match frame(&self.buf[p..]) {
                    Ok(Some((used, payload))) => {
                        p += used;
                        if let Some(d) = payload {
                            self.send(Kind::WebSocket, d);
                        }
                    }
                    Ok(None) => break,
                    Err(why) => self.stop(&why),
                },
                ServerState::Stopped => break,
            }
        }
        if matches!(self.state, ServerState::Stopped) {
            self.buf = Vec::new();
        } else {
            self.buf.drain(..p);
        }
    }

    /// The server closed the connection: a body read until close ends here.
    pub fn close(&mut self) {
        if let ServerState::Body { body: Body::UntilClose, .. } = self.state {
            if let ServerState::Body { keep, encoding, .. } = std::mem::replace(&mut self.state, ServerState::Stopped) {
                self.finish(keep, &encoding);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// One byte at a time, then all at once: the same payloads either way.
    fn both_ways(host: Host, client: &[u8], server: &[u8]) -> Vec<Vec<u8>> {
        let mut runs = Vec::new();
        for size in [1, client.len().max(server.len())] {
            let (tx, rx) = mpsc::channel();
            let (mut c, mut s) = new(host, 7, tx);
            client.chunks(size).for_each(|b| c.feed(b));
            server.chunks(size).for_each(|b| s.feed(b));
            s.close();
            let got: Vec<Vec<u8>> = rx
                .try_iter()
                .map(|o| match o {
                    Out::Item(i) => i.payload,
                    other => panic!("{other:?}"),
                })
                .collect();
            runs.push(got);
        }
        assert_eq!(runs[0], runs[1]);
        runs.pop().unwrap()
    }

    #[test]
    fn the_lobby_websocket_gives_the_servers_messages() {
        let client = b"GET /hi HTTP/1.1\r\nAuthorization: secret\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n\x82\x83abcdxyz";
        let mut server = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\r\n".to_vec();
        server.extend([0x82, 3, b'o', b'n', b'e']);
        server.extend([0x89, 1, b'p']);
        server.extend([0x82, 126, 0, 200]);
        server.extend([b'z'; 200]);
        assert_eq!(both_ways(Host::Lobby, client, &server), vec![b"one".to_vec(), vec![b'z'; 200]]);
    }

    #[test]
    fn only_session_documents_are_kept() {
        use flate2::{write::GzEncoder, Compression};
        use std::io::Write;
        let mut e = GzEncoder::new(Vec::new(), Compression::default());
        e.write_all(b"{\"members\":{}}").unwrap();
        let z = e.finish().unwrap();
        let mut client =
            b"PUT /serviceconfigs/x/sessionTemplates/t/sessions/s HTTP/1.1\r\nContent-Length: 4\r\n\r\nbody".to_vec();
        client.extend(b"GET /handles HTTP/1.1\r\n\r\n");
        client.extend(b"HEAD /serviceconfigs/x/sessionTemplates/t/sessions/s HTTP/1.1\r\n\r\n");
        client.extend(b"GET /serviceconfigs/x/sessionTemplates/t/sessions/s HTTP/1.1\r\n\r\n");
        let mut server = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Encoding: gzip\r\n\r\n".to_vec();
        server.extend(format!("{:x}\r\n", z.len()).bytes());
        server.extend(&z);
        server.extend(b"\r\n0\r\n\r\n");
        server.extend(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n[]");
        server.extend(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n");
        server.extend(b"HTTP/1.1 200 OK\r\n\r\n{\"a\":1}");
        let got = both_ways(Host::SessionDirectory, &client, &server);
        assert_eq!(got, vec![b"{\"members\":{}}".to_vec(), b"{\"a\":1}".to_vec()]);
    }

    /// Everything the tap passes on, from each side's bytes given whole.
    fn outs(host: Host, client: &[u8], server: &[u8]) -> Vec<Out> {
        let (tx, rx) = mpsc::channel();
        let (mut c, mut s) = new(host, 7, tx);
        c.feed(client);
        s.feed(server);
        s.close();
        rx.try_iter().collect()
    }

    const GET: &[u8] = b"GET /serviceconfigs/x/sessionTemplates/t/sessions/s HTTP/1.1\r\n\r\n";

    #[test]
    fn a_document_over_the_limit_is_a_problem_and_the_next_still_comes() {
        let mut server = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1).into_bytes();
        server.resize(server.len() + MAX_BODY + 1, b' ');
        server.extend(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
        let got = outs(Host::SessionDirectory, &GET.repeat(2), &server);
        assert!(matches!(&got[..], [Out::Problem(p), Out::Item(i)] if p.contains("over 16 MB") && i.payload == b"{}"));
    }

    #[test]
    fn a_document_that_inflates_past_the_limit_is_a_problem() {
        use flate2::{write::GzEncoder, Compression};
        use std::io::Write;
        let mut e = GzEncoder::new(Vec::new(), Compression::fast());
        e.write_all(&vec![b' '; MAX_BODY + 1]).unwrap();
        let z = e.finish().unwrap();
        let mut server =
            format!("HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n", z.len()).into_bytes();
        server.extend(z);
        let got = outs(Host::SessionDirectory, GET, &server);
        assert!(matches!(&got[..], [Out::Problem(p)] if p.contains("over 16 MB")), "{got:?}");
    }

    #[test]
    fn chunk_extensions_are_skipped() {
        let server = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: Chunked\r\n\r\n5;a=1\r\n{\"a\":\r\n2 ; b=\"x\"\r\n1}\r\n0;c\r\n\r\n";
        assert_eq!(both_ways(Host::SessionDirectory, GET, server), vec![b"{\"a\":1}".to_vec()]);
    }

    #[test]
    fn a_frame_with_a_64_bit_length_is_read() {
        let client = b"GET /hi HTTP/1.1\r\nUpgrade: websocket\r\n\r\n";
        let mut server = b"HTTP/1.1 101 Switching Protocols\r\n\r\n".to_vec();
        server.extend([0x82, 127, 0, 0, 0, 0, 0, 1, 0, 0]);
        server.resize(server.len() + (1 << 16), b'z');
        assert_eq!(both_ways(Host::Lobby, client, &server), vec![vec![b'z'; 1 << 16]]);
    }

    #[test]
    fn a_frame_over_the_limit_stops_the_copy_with_a_problem() {
        let client = b"GET /hi HTTP/1.1\r\nUpgrade: websocket\r\n\r\n";
        let mut server = b"HTTP/1.1 101 Switching Protocols\r\n\r\n".to_vec();
        server.extend([0x82, 127, 0, 0, 0, 0, 1, 0, 0, 1]);
        let got = outs(Host::Lobby, client, &server);
        assert!(matches!(&got[..], [Out::Problem(p)] if p.contains("over 16 MB")), "{got:?}");
    }

    #[test]
    fn a_response_without_a_request_stops_the_copy() {
        let (tx, rx) = mpsc::channel();
        let (_c, mut s) = new(Host::SessionDirectory, 1, tx);
        s.feed(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        assert!(matches!(rx.try_recv(), Ok(Out::Problem(_))));
        s.feed(b"HTTP/1.1 200 OK\r\n\r\n");
        assert!(rx.try_recv().is_err());
    }
}
