//! Fetching for deployments: what reaches past the worker, through the
//! supervisor, which checks every destination ([`Egress`]).
//!
//! A destination is a URL over HTTP or HTTPS, on a port the policy allows
//! (80 and 443), naming no credentials; its host's addresses are resolved
//! here, each checked ([`public`]: no loopback, private, link-local,
//! shared, reserved, multicast or documentation address, none embedding
//! IPv4 in IPv6, none of this machine's own), and the connection is made
//! to an address checked, never looked up again. TLS certificates are
//! verified for the host asked for, against Mozilla's roots. Redirects are
//! followed here, a few at most, each target checked as a request anew,
//! credentials dropped when the origin changes. Bodies, both ways, URLs,
//! headers, time, name lookups at once and connections kept (none) are
//! bounded. Addresses the machine is reached by without an interface of
//! its own (a NAT's) are not known here: the network around it must deny
//! them.

use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use reqwest::{
    Method, Url,
    dns::{Addrs, Name, Resolve, Resolving},
};
use techne_vm::data::Data;

/// What a deployment may reach.
#[derive(Clone, Debug)]
pub struct Policy {
    /// Whether an address may be connected to: [`public`].
    pub allow: fn(IpAddr) -> bool,
    /// The ports a URL may name.
    pub ports: Vec<u16>,
    /// Redirects followed, at most.
    pub redirects: usize,
    /// Bytes of a request's body, and of a response's.
    pub request: usize,
    pub response: usize,
    /// Time a fetch may take, its redirects included.
    pub timeout: Duration,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy { allow: public, ports: vec![80, 443], redirects: 5, request: 1 << 20, response: 8 << 20, timeout: Duration::from_secs(10) }
    }
}

/// Whether `ip` is an address of the internet at large: none of the
/// special-use ranges (RFC 6890 and after) nor of this machine's
/// interfaces now (none at all if they cannot be listed).
pub fn public(ip: IpAddr) -> bool {
    let special = match ip {
        IpAddr::V4(a) => special_v4(a),
        IpAddr::V6(a) => special_v6(a),
    };
    !special && own_addresses().is_some_and(|own| !own.contains(&ip))
}

fn special_v4(a: Ipv4Addr) -> bool {
    let [o0, o1, o2, _] = a.octets();
    a.is_unspecified()
        || a.is_loopback()
        || a.is_private()
        || a.is_link_local()
        || a.is_broadcast()
        || a.is_multicast()
        || a.is_documentation()
        || o0 == 0
        // Shared address space (carrier-grade NAT).
        || (o0 == 100 && o1 & 0xc0 == 64)
        // IETF protocol assignments, the 6to4 relay anycast.
        || (o0 == 192 && o1 == 0 && o2 == 0)
        || (o0 == 192 && o1 == 88 && o2 == 99)
        // Benchmarking.
        || (o0 == 198 && o1 & 0xfe == 18)
        // Reserved.
        || o0 >= 240
}

fn special_v6(a: Ipv6Addr) -> bool {
    let s = a.segments();
    a.is_unspecified()
        || a.is_loopback()
        || a.is_multicast()
        // IPv4 inside: mapped, compatible, NAT64, 6to4, Teredo.
        || a.to_ipv4_mapped().is_some()
        || s[..6] == [0; 6]
        || (s[0] == 0x64 && s[1] == 0xff9b)
        || s[0] == 0x2002
        || (s[0] == 0x2001 && s[1] == 0)
        // Unique local, link-local, site-local.
        || s[0] & 0xfe00 == 0xfc00
        || s[0] & 0xffc0 == 0xfe80
        || s[0] & 0xffc0 == 0xfec0
        // Documentation, discard.
        || (s[0] == 0x2001 && s[1] == 0xdb8)
        || s[..4] == [0x100, 0, 0, 0]
}

/// This machine's addresses, as its interfaces have them: a public one
/// reaches the machine's own services. None if they cannot be listed.
fn own_addresses() -> Option<HashSet<IpAddr>> {
    let mut own = HashSet::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `list`, freed below; each entry's address is
    // read as the family it says.
    unsafe {
        if libc::getifaddrs(&mut list) != 0 {
            return None;
        }
        let mut at = list;
        while !at.is_null() {
            let a = (*at).ifa_addr;
            if !a.is_null() {
                match i32::from((*a).sa_family) {
                    libc::AF_INET => {
                        let a = &*a.cast::<libc::sockaddr_in>();
                        own.insert(IpAddr::V4(Ipv4Addr::from(u32::from_be(a.sin_addr.s_addr))));
                    }
                    libc::AF_INET6 => {
                        let a = &*a.cast::<libc::sockaddr_in6>();
                        own.insert(IpAddr::V6(Ipv6Addr::from(a.sin6_addr.s6_addr)));
                    }
                    _ => {}
                }
            }
            at = (*at).ifa_next;
        }
        libc::freeifaddrs(list);
    }
    Some(own)
}

/// Name lookups at once: each holds a thread (`getaddrinfo` blocks) until
/// it ends, whether or not its fetch still waits.
const LOOKUPS: usize = 16;
static LOOKING_UP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A lookup's place among `LOOKUPS`, given back when it ends.
struct Lookup;

impl Lookup {
    fn take() -> Option<Lookup> {
        use std::sync::atomic::Ordering::SeqCst;
        LOOKING_UP.try_update(SeqCst, SeqCst, |n| (n < LOOKUPS).then_some(n + 1)).ok().map(|_| Lookup)
    }
}

impl Drop for Lookup {
    fn drop(&mut self) {
        LOOKING_UP.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Headers a fetch sends, their bytes, and a URL's.
const HEADERS: usize = 64;
const HEADER_BYTES: usize = 16 << 10;
const URL_BYTES: usize = 8 << 10;

/// Names resolved here, only to the addresses the policy allows.
struct Checked {
    allow: fn(IpAddr) -> bool,
}

impl Resolve for Checked {
    fn resolve(&self, name: Name) -> Resolving {
        let (allow, host) = (self.allow, name.as_str().to_string());
        Box::pin(async move {
            use std::net::ToSocketAddrs;
            let place = Lookup::take().ok_or("too many names being looked up")?;
            let looked_up = tokio::task::spawn_blocking(move || {
                let _place = place;
                (host.as_str(), 0).to_socket_addrs().map(|a| (host.clone(), a.collect::<Vec<_>>()))
            });
            let (host, addrs) = looked_up.await??;
            let addrs: Vec<SocketAddr> = addrs.into_iter().filter(|a| allow(a.ip())).collect();
            if addrs.is_empty() {
                return Err(format!("{host}: no address it may reach").into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// Fetches for deployments, within a policy: see the module.
pub struct Egress {
    client: reqwest::Client,
    policy: Policy,
}

impl Egress {
    pub fn new(policy: Policy) -> Result<Egress, String> {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
        tls.alpn_protocols = vec![b"http/1.1".to_vec()];
        let client = reqwest::Client::builder()
            .use_preconfigured_tls(tls)
            .dns_resolver(Arc::new(Checked { allow: policy.allow }))
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .pool_max_idle_per_host(0)
            .retry(reqwest::retry::never())
            .timeout(policy.timeout)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Egress { client, policy })
    }

    /// `url`, if a deployment may fetch it: why not, else.
    fn check(&self, url: &Url) -> Result<(), String> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(format!("{url}: only http and https are fetched"));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(format!("{url}: a URL naming credentials"));
        }
        match url.port_or_known_default() {
            Some(p) if self.policy.ports.contains(&p) => {}
            _ => return Err(format!("{url}: a port not fetched from")),
        }
        // An address in the URL is connected to as it is, unresolved.
        let literal = match url.host() {
            Some(url::Host::Ipv4(a)) => Some(IpAddr::V4(a)),
            Some(url::Host::Ipv6(a)) => Some(IpAddr::V6(a)),
            Some(url::Host::Domain(_)) => None,
            None => return Err(format!("{url}: no host")),
        };
        match literal {
            Some(ip) if !(self.policy.allow)(ip) => Err(format!("{url}: an address it may not reach")),
            _ => Ok(()),
        }
    }

    /// Fetch `request`, a dict of its `url`, and its `method`, `headers`
    /// (a dict of strings) and `body` (a string or bytevector) if given:
    /// the response, a dict of its `status`, `headers` (names lowercase,
    /// values of one name joined by ", "), `body` (a bytevector) and
    /// `url`, where it was found after redirects; or why not.
    pub async fn fetch(&self, request: &Data) -> Result<Data, String> {
        let field = |name: &str| match request {
            Data::Dict(entries) => entries.iter().find(|(k, _)| matches!(k, Data::Symbol(s) if s == name)).map(|(_, v)| v),
            _ => None,
        };
        let mut url = match field("url") {
            Some(Data::String(u)) if u.len() > URL_BYTES => return Err(format!("a URL past {URL_BYTES} bytes")),
            Some(Data::String(u)) => Url::parse(u).map_err(|e| format!("{u}: {e}"))?,
            _ => return Err("a request to fetch names its url, a string".into()),
        };
        let mut method = match field("method") {
            None => Method::GET,
            Some(Data::String(m)) => Method::from_bytes(m.as_bytes()).map_err(|_| format!("{m}: no method"))?,
            Some(_) => return Err("a method is a string".into()),
        };
        let mut headers = Vec::new();
        match field("headers") {
            None => {}
            Some(Data::Dict(entries)) => {
                let bytes: usize = entries
                    .iter()
                    .map(|(k, v)| match (k, v) {
                        (Data::String(k), Data::String(v)) => k.len() + v.len(),
                        _ => 0,
                    })
                    .sum();
                if entries.len() > HEADERS || bytes > HEADER_BYTES {
                    return Err(format!("more than {HEADERS} headers, or {HEADER_BYTES} bytes of them"));
                }
                for (k, v) in entries {
                    let (Data::String(k), Data::String(v)) = (k, v) else { return Err("headers are strings".into()) };
                    if reqwest::header::HeaderName::from_bytes(k.as_bytes()).is_err() || reqwest::header::HeaderValue::from_str(v).is_err()
                    {
                        return Err(format!("{k}: no header"));
                    }
                    let lower = k.to_ascii_lowercase();
                    // What the client frames and routes by is its own.
                    if !matches!(lower.as_str(), "host" | "connection" | "content-length" | "transfer-encoding" | "upgrade") {
                        headers.push((lower, v.clone()));
                    }
                }
            }
            Some(_) => return Err("headers are a dict of strings".into()),
        }
        let mut body = match field("body") {
            None => Vec::new(),
            Some(Data::Bytes(b)) => b.clone(),
            Some(Data::String(s)) => s.clone().into_bytes(),
            Some(_) => return Err("a body is a string or a bytevector".into()),
        };
        if body.len() > self.policy.request {
            return Err(format!("a body past {} bytes", self.policy.request));
        }
        let fetching = async {
            let mut redirects = 0;
            loop {
                self.check(&url)?;
                let mut builder = self.client.request(method.clone(), url.clone());
                for (k, v) in &headers {
                    builder = builder.header(k, v);
                }
                if !body.is_empty() {
                    builder = builder.body(body.clone());
                }
                let mut response = builder.send().await.map_err(|e| format!("{url}: {}", reason(&e)))?;
                let status = response.status();
                let location = response.headers().get(reqwest::header::LOCATION).and_then(|l| l.to_str().ok()).map(str::to_string);
                if let (true, Some(location)) = (status.is_redirection(), location) {
                    redirects += 1;
                    if redirects > self.policy.redirects {
                        return Err(format!("{url}: more than {} redirects", self.policy.redirects));
                    }
                    let next = url.join(&location).map_err(|e| format!("{location}: {e}"))?;
                    // Another origin is told nothing of this one's.
                    if next.origin() != url.origin() {
                        headers.retain(|(k, _)| !matches!(k.as_str(), "authorization" | "cookie" | "proxy-authorization"));
                    }
                    let gets = status == reqwest::StatusCode::SEE_OTHER || (matches!(status.as_u16(), 301 | 302) && method == Method::POST);
                    if gets {
                        (method, body) = (Method::GET, Vec::new());
                        headers.retain(|(k, _)| !k.starts_with("content-"));
                    }
                    url = next;
                    continue;
                }
                let mut read = Vec::new();
                while let Some(chunk) = response.chunk().await.map_err(|e| format!("{url}: {}", reason(&e)))? {
                    if read.len() + chunk.len() > self.policy.response {
                        return Err(format!("{url}: a response past {} bytes", self.policy.response));
                    }
                    read.extend_from_slice(&chunk);
                }
                let mut fields: Vec<(String, String)> = Vec::new();
                for (k, v) in response.headers() {
                    let v = String::from_utf8_lossy(v.as_bytes()).into_owned();
                    match fields.iter_mut().find(|(name, _)| name == k.as_str()) {
                        Some((_, joined)) => {
                            joined.push_str(", ");
                            joined.push_str(&v);
                        }
                        None => fields.push((k.as_str().to_string(), v)),
                    }
                }
                let sym = |s: &str| Data::Symbol(s.into());
                return Ok(Data::Dict(vec![
                    (sym("status"), Data::Int(status.as_u16().into())),
                    (sym("headers"), Data::Dict(fields.into_iter().map(|(k, v)| (Data::String(k), Data::String(v))).collect())),
                    (sym("body"), Data::Bytes(read)),
                    (sym("url"), Data::String(url.to_string())),
                ]));
            }
        };
        tokio::time::timeout(self.policy.timeout, fetching).await.map_err(|_| "the fetch took too long".to_string())?
    }
}

/// What a client error says, its causes too.
fn reason(e: &reqwest::Error) -> String {
    let mut s = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(cause) = source {
        s.push_str(": ");
        s.push_str(&cause.to_string());
        source = cause.source();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_internet_at_large_is_public() {
        let refused = [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "0.1.2.3",
            "192.0.0.8",
            "192.0.2.1",
            "198.18.0.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::ffff:127.0.0.1",
            "::ffff:8.8.8.8",
            "::127.0.0.1",
            "64:ff9b::7f00:1",
            "2002:7f00:1::",
            "2001:0:4136:e378::",
            "fc00::1",
            "fd00:ec2::254",
            "fe80::1",
            "fec0::1",
            "ff02::1",
            "2001:db8::1",
            "100::1",
        ];
        for a in refused {
            assert!(!public(a.parse().unwrap()), "{a} is no public address");
        }
        for a in ["8.8.8.8", "1.1.1.1", "93.184.216.34", "2606:4700:4700::1111", "2a00:1450:4001::200e"] {
            assert!(public(a.parse().unwrap()), "{a} is a public address");
        }
    }
}
