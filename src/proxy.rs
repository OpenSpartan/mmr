//! The local HTTPS proxy behind `mmr watch`, on 127.0.0.1 only.
//!
//! Programs that use the system proxy ask it for tunnels with CONNECT. A tunnel to
//! one of `cert::HOSTS` on port 443 is decrypted: the proxy checks the real server's
//! certificate against the Windows roots (never an mmr root), answers the client with
//! a certificate for that host signed by the local root, and relays every byte both
//! ways unchanged while a tap reads a copy. Every other tunnel is passed through
//! without being read.

use crate::cert::{Root, MARK};
use crate::tap::{self, ClientSide, Host, Out, ServerSide};
use rustls::pki_types::ServerName;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

pub const DEFAULT_PORT: u16 = 47812;
const MAX_CONNECT: usize = 16 << 10;
/// How long each side gets for its CONNECT or its TLS handshake.
const PATIENCE: Duration = Duration::from_secs(30);

pub struct Proxy {
    port: u16,
    lobby: TlsAcceptor,
    sessions: TlsAcceptor,
    upstream: TlsConnector,
    next_conn: AtomicU64,
    out: Sender<Out>,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Answers clients for `host` with a server certificate made for this run.
fn acceptor(root: &Root, host: &str) -> Result<TlsAcceptor, String> {
    let (cert, key) = root.leaf(host)?;
    // No ALPN: the client speaks HTTP/1.1, as it does through Fiddler.
    let config = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(err)?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .map_err(err)?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// Connects to the real servers, trusting the Windows roots except mmr's own.
fn connector() -> Result<TlsConnector, String> {
    let found = rustls_native_certs::load_native_certs();
    let mut roots = rustls::RootCertStore::empty();
    for c in found.certs {
        if !c.windows(MARK.len()).any(|w| w == MARK.as_bytes()) {
            let _ = roots.add(c);
        }
    }
    if roots.is_empty() {
        return Err("no root certificates found in Windows to check the real servers with".into());
    }
    let mut config = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(err)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsConnector::from(Arc::new(config)))
}

fn find(h: &[u8], needle: &[u8]) -> Option<usize> {
    h.windows(needle.len()).position(|w| w == needle)
}

/// `host:port` or `[v6]:port`.
fn authority(target: &str) -> Option<(String, u16)> {
    let (h, p) = target.rsplit_once(':')?;
    let h = h.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(h);
    Some((h.to_string(), p.parse().ok()?))
}

/// The client's request head, and any bytes it sent after it.
async fn read_head(s: &mut TcpStream) -> io::Result<(String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(e) = find(&buf, b"\r\n\r\n") {
            let rest = buf.split_off(e + 4);
            return Ok((String::from_utf8_lossy(&buf[..e]).into_owned(), rest));
        }
        if buf.len() > MAX_CONNECT {
            return Err(io::ErrorKind::InvalidData.into());
        }
    }
}

async fn answer(s: &mut TcpStream, status: &str) -> io::Result<()> {
    s.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await
}

const ESTABLISHED: &[u8] = b"HTTP/1.1 200 Connection Established\r\n\r\n";

/// Copies `from` to `to`, giving the tap each piece. The tap reads the client's bytes
/// before they go on, so it knows each request before its response arrives.
async fn pump<R, W>(mut from: R, mut to: W, mut tap: impl FnMut(&[u8]), tap_first: bool) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; 32 << 10];
    loop {
        let n = from.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        if tap_first {
            tap(&buf[..n]);
        }
        to.write_all(&buf[..n]).await?;
        if !tap_first {
            tap(&buf[..n]);
        }
    }
    to.shutdown().await
}

/// Relays a decrypted connection both ways until both sides are done, or either
/// side fails.
async fn relay<C, U>(client: C, server: U, (mut tc, mut ts): (ClientSide, ServerSide))
where
    C: AsyncRead + AsyncWrite + Unpin,
    U: AsyncRead + AsyncWrite + Unpin,
{
    let (cr, cw) = tokio::io::split(client);
    let (sr, sw) = tokio::io::split(server);
    let up = pump(cr, sw, |b| tc.feed(b), true);
    let down = async {
        let r = pump(sr, cw, |b| ts.feed(b), false).await;
        ts.close();
        r
    };
    tokio::pin!(up, down);
    tokio::select! {
        r = &mut up => if r.is_ok() { let _ = down.await; },
        r = &mut down => if r.is_ok() { let _ = up.await; },
    }
}

impl Proxy {
    pub fn new(root: &Root, port: u16, out: Sender<Out>) -> Result<Proxy, String> {
        Ok(Proxy {
            port,
            lobby: acceptor(root, crate::cert::HOSTS[0])?,
            sessions: acceptor(root, crate::cert::HOSTS[1])?,
            upstream: connector()?,
            next_conn: AtomicU64::new(1),
            out,
        })
    }

    fn problem(&self, what: String) {
        let _ = self.out.send(Out::Problem(what));
    }

    pub async fn serve(self: Arc<Self>, listener: TcpListener) {
        loop {
            let Ok((s, _)) = listener.accept().await else {
                // Accept can fail when sockets run out: wait for some to free up rather than spin.
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };
            let me = self.clone();
            tokio::spawn(async move {
                let _ = me.connection(s).await;
            });
        }
    }

    async fn connection(&self, mut client: TcpStream) -> io::Result<()> {
        client.set_nodelay(true)?;
        let (head, rest) = match tokio::time::timeout(PATIENCE, read_head(&mut client)).await {
            Ok(r) => r?,
            Err(_) => return Ok(()),
        };
        let mut line = head.split("\r\n").next().unwrap_or("").split(' ');
        let (method, target) = (line.next().unwrap_or(""), line.next().unwrap_or(""));
        if !method.eq_ignore_ascii_case("CONNECT") {
            return answer(&mut client, "405 Method Not Allowed").await;
        }
        let Some((host, port)) = authority(target) else {
            return answer(&mut client, "400 Bad Request").await;
        };
        let local = host == "127.0.0.1" || host == "::1" || host.eq_ignore_ascii_case("localhost");
        if local && port == self.port {
            return answer(&mut client, "403 Forbidden").await;
        }
        let Ok(mut upstream) = TcpStream::connect((host.as_str(), port)).await else {
            return answer(&mut client, "502 Bad Gateway").await;
        };
        // Another name for this machine can also lead back here. A v4 address and its
        // v6-mapped form count as the same.
        let (to, me) = (upstream.peer_addr()?, client.local_addr()?);
        if to.port() == me.port() && to.ip().to_canonical() == me.ip().to_canonical() {
            return answer(&mut client, "403 Forbidden").await;
        }
        upstream.set_nodelay(true)?;
        // Clients wait for the answer before their TLS hello. One that doesn't is
        // passed through rather than read.
        let Some(tapped) = Host::of(&host).filter(|_| port == 443 && rest.is_empty()) else {
            client.write_all(ESTABLISHED).await?;
            upstream.write_all(&rest).await?;
            tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
            return Ok(());
        };
        let name = ServerName::try_from(host.clone()).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let server = match tokio::time::timeout(PATIENCE, self.upstream.connect(name, upstream)).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                self.problem(format!("{host} didn't pass the certificate check, so it wasn't connected: {e}"));
                return answer(&mut client, "502 Bad Gateway").await;
            }
            Err(_) => {
                let s = PATIENCE.as_secs();
                self.problem(format!("{host} didn't finish its TLS handshake in {s} seconds, so it wasn't connected"));
                return answer(&mut client, "504 Gateway Timeout").await;
            }
        };
        client.write_all(ESTABLISHED).await?;
        let acceptor = if tapped == Host::Lobby { &self.lobby } else { &self.sessions };
        let client = match tokio::time::timeout(PATIENCE, acceptor.accept(client)).await {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
                self.problem(format!("a program rejected mmr's certificate for {host} ({e}): check `mmr cert`"));
                return Ok(());
            }
            Err(_) => return Ok(()), // a program that never finished its hello
        };
        let conn = self.next_conn.fetch_add(1, Ordering::Relaxed);
        let _ = self.out.send(Out::Opened(tapped));
        relay(client, server, tap::new(tapped, conn, self.out.clone())).await;
        // The tap sent its items from this task, so they all arrive before Closed.
        let _ = self.out.send(Out::Closed(tapped, conn));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorities_split_into_host_and_port() {
        assert_eq!(
            authority("lobby-hi.svc.halowaypoint.com:443"),
            Some(("lobby-hi.svc.halowaypoint.com".to_string(), 443))
        );
        assert_eq!(authority("[::1]:8080"), Some(("::1".to_string(), 8080)));
        assert_eq!(authority("nohost"), None);
    }

    #[tokio::test]
    async fn a_tunnel_back_to_the_proxy_is_refused() {
        let (tx, _rx) = std::sync::mpsc::channel();
        // Given a port other than the one it listens on, so only the check after
        // connecting catches the loop.
        let proxy = Arc::new(Proxy::new(&Root::make().unwrap(), 1, tx).unwrap());
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(proxy.serve(listener));
        let mut c = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        c.write_all(format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\n").as_bytes()).await.unwrap();
        let mut got = [0u8; 12];
        c.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"HTTP/1.1 403");
    }

    /// A client that trusts only the root gets through to a fake upstream, and the
    /// tap sees the server's WebSocket message.
    #[tokio::test]
    async fn a_decrypted_relay_passes_bytes_and_taps_them() {
        let root = Root::make().unwrap();
        let host = crate::cert::HOSTS[0];
        let lobby = acceptor(&root, host).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(rustls::pki_types::CertificateDer::from(root.der.clone())).unwrap();
        let client_config = rustls::ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let (client_end, proxy_client) = tokio::io::duplex(1 << 16);
        let (proxy_server, mut server_end) = tokio::io::duplex(1 << 16);
        let (tx, rx) = std::sync::mpsc::channel();
        let proxy = tokio::spawn(async move {
            let c = lobby.accept(proxy_client).await.unwrap();
            relay(c, proxy_server, tap::new(Host::Lobby, 1, tx)).await;
        });
        let connector = TlsConnector::from(Arc::new(client_config));
        let mut c = connector.connect(ServerName::try_from(host).unwrap(), client_end).await.unwrap();
        let request = b"GET / HTTP/1.1\r\nUpgrade: websocket\r\n\r\n";
        c.write_all(request).await.unwrap();
        let mut got = vec![0u8; request.len()];
        server_end.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, request);
        let reply = b"HTTP/1.1 101 Switching Protocols\r\n\r\n\x82\x02hi";
        server_end.write_all(reply).await.unwrap();
        let mut back = vec![0u8; reply.len()];
        c.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, reply);
        drop(c);
        drop(server_end);
        proxy.await.unwrap();
        let items: Vec<Vec<u8>> =
            rx.try_iter().filter_map(|o| if let Out::Item(i) = o { Some(i.payload) } else { None }).collect();
        assert_eq!(items, vec![b"hi".to_vec()]);
    }
}
