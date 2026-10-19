//! Fetching for deployments: every destination checked, redirects too,
//! against a server on loopback that a test policy lets through (the
//! default policy refuses loopback, as it refuses every address below).

use std::net::IpAddr;

use techne_host::egress::{Egress, Policy};
use techne_vm::data::Data;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn get(url: &str) -> Data {
    Data::Dict(vec![(Data::Symbol("url".into()), Data::String(url.into()))])
}

fn with(request: Data, fields: &[(&str, Data)]) -> Data {
    let Data::Dict(mut entries) = request else { unreachable!() };
    entries.extend(fields.iter().map(|(k, v)| (Data::Symbol((*k).into()), v.clone())));
    Data::Dict(entries)
}

fn field<'a>(d: &'a Data, name: &str) -> &'a Data {
    let Data::Dict(entries) = d else { panic!("{d:?}") };
    entries.iter().find(|(k, _)| matches!(k, Data::Symbol(s) if s == name)).map(|(_, v)| v).unwrap()
}

fn body(d: &Data) -> String {
    match field(d, "body") {
        Data::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        other => panic!("{other:?}"),
    }
}

fn run<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}

/// A server on loopback answering by path, as the tests below need: its
/// port.
async fn serve() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut c, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0];
                while !head.ends_with(b"\r\n\r\n") && c.read(&mut byte).await.unwrap_or(0) == 1 {
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let mut lines = head.split("\r\n");
                let mut start = lines.next().unwrap_or("").split(' ');
                let (method, path) = (start.next().unwrap_or(""), start.next().unwrap_or(""));
                let header = |name: &str| {
                    head.split("\r\n")
                        .find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string()))
                };
                let redirect = |status: &str, to: String| format!("HTTP/1.1 {status}\r\nlocation: {to}\r\ncontent-length: 0\r\n\r\n");
                let ok = |b: &str| format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{b}", b.len());
                let answer = match path {
                    "/hello" => "HTTP/1.1 200 OK\r\nx-a: 1\r\nx-a: 2\r\ncontent-length: 2\r\n\r\nhi".to_string(),
                    "/to-other" => redirect("302 Found", format!("http://localhost:{port}/auth")),
                    "/to-same" => redirect("302 Found", "/auth".into()),
                    "/auth" => ok(&header("authorization").unwrap_or_else(|| "none".into())),
                    "/to-private" => redirect("302 Found", format!("http://127.0.0.2:{port}/hello")),
                    "/to-port" => redirect("302 Found", "http://127.0.0.1:9/hello".into()),
                    "/loop" => redirect("302 Found", "/loop".into()),
                    "/see-other" => redirect("303 See Other", "/method".into()),
                    "/method" => ok(method),
                    "/big" => format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{}", 9 << 20, "x".repeat(9 << 20)),
                    _ => "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n".into(),
                };
                let _ = c.write_all(answer.as_bytes()).await;
            });
        }
    });
    port
}

/// Loopback's 127.0.0.1 alone, on `port`: what the server needs.
fn loopback(port: u16) -> Egress {
    Egress::new(Policy { allow: |ip: IpAddr| ip == IpAddr::from([127, 0, 0, 1]), ports: vec![port], ..Policy::default() }).unwrap()
}

#[test]
fn the_default_policy_reaches_nothing_but_the_internet() {
    run(async {
        let egress = Egress::new(Policy::default()).unwrap();
        for url in [
            "http://127.0.0.1/",
            "http://localhost/",
            "http://[::1]/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[fd00:ec2::254]/",
            "http://[::ffff:127.0.0.1]/",
            "http://0.0.0.0/",
            "http://10.0.0.1/",
            "http://2130706433/",
            "http://0x7f.1/",
            "ftp://example.com/",
            "file:///etc/passwd",
            "http://example.com:22/",
            "http://user:secret@example.com/",
        ] {
            let refused = egress.fetch(&get(url)).await;
            assert!(refused.is_err(), "{url} fetched: {refused:?}");
        }
    });
}

#[test]
fn a_fetch_answers_with_status_headers_and_body() {
    run(async {
        let port = serve().await;
        let r = loopback(port).fetch(&get(&format!("http://127.0.0.1:{port}/hello"))).await.unwrap();
        assert_eq!(field(&r, "status"), &Data::Int(200));
        assert_eq!(body(&r), "hi");
        let Data::Dict(headers) = field(&r, "headers") else { panic!() };
        assert!(headers.contains(&(Data::String("x-a".into()), Data::String("1, 2".into()))), "{headers:?}");
    });
}

#[test]
fn redirects_are_checked_as_requests_anew() {
    run(async {
        let port = serve().await;
        let egress = loopback(port);
        let at = |path: &str| format!("http://127.0.0.1:{port}{path}");
        for path in ["/to-private", "/to-port", "/loop"] {
            let refused = egress.fetch(&get(&at(path))).await;
            assert!(refused.is_err(), "{path}: {refused:?}");
        }
        // Credentials go to the same origin only.
        let authorized = |path: &str| {
            with(get(&at(path)), &[("headers", Data::Dict(vec![(Data::String("Authorization".into()), Data::String("secret".into()))]))])
        };
        assert_eq!(body(&egress.fetch(&authorized("/to-same")).await.unwrap()), "secret");
        let r = egress.fetch(&authorized("/to-other")).await.unwrap();
        assert_eq!(body(&r), "none");
        assert_eq!(field(&r, "url"), &Data::String(format!("http://localhost:{port}/auth")));
        // A 303 is fetched with GET.
        let post = with(get(&at("/see-other")), &[("method", Data::String("POST".into())), ("body", Data::String("x".into()))]);
        assert_eq!(body(&egress.fetch(&post).await.unwrap()), "GET");
    });
}

#[test]
fn a_response_past_its_bound_is_refused() {
    run(async {
        let port = serve().await;
        let refused = loopback(port).fetch(&get(&format!("http://127.0.0.1:{port}/big"))).await;
        assert!(refused.as_ref().is_err_and(|e| e.contains("past")), "{refused:?}");
    });
}

/// Over the internet, TLS verified (`cargo test -- --ignored`).
#[test]
#[ignore = "reaches the internet"]
fn fetches_from_the_internet() {
    run(async {
        let egress = Egress::new(Policy::default()).unwrap();
        let r = egress.fetch(&get("https://example.com/")).await.unwrap();
        assert_eq!(field(&r, "status"), &Data::Int(200));
        assert!(body(&r).contains("Example Domain"));
        assert!(egress.fetch(&get("https://expired.badssl.com/")).await.is_err(), "an expired certificate");
        assert!(egress.fetch(&get("https://wrong.host.badssl.com/")).await.is_err(), "another host's certificate");
    });
}

#[test]
fn what_a_request_sends_is_bounded() {
    run(async {
        let port = serve().await;
        let egress = loopback(port);
        let url = format!("http://127.0.0.1:{port}/hello");
        let headers = |n: usize| Data::Dict((0..n).map(|i| (Data::String(format!("x-{i}")), Data::String("v".into()))).collect());
        assert!(egress.fetch(&with(get(&url), &[("headers", headers(64))])).await.is_ok());
        assert!(egress.fetch(&with(get(&url), &[("headers", headers(40_000))])).await.is_err(), "too many headers");
        let bad = Data::Dict(vec![(Data::String("a b".into()), Data::String("v".into()))]);
        assert!(egress.fetch(&with(get(&url), &[("headers", bad)])).await.is_err(), "no header name");
        let long = format!("{url}?{}", "a".repeat(9000));
        assert!(egress.fetch(&get(&long)).await.is_err(), "a URL too long");
    });
}

/// What a deployment's requests fetch goes through their host's broker,
/// a bounded number of times each.
#[test]
fn a_request_fetches_through_its_host() {
    use techne_host::{Deployment, Host, Limits, outbound};
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    tokio::task::LocalSet::new().block_on(&rt, async {
        let port = serve().await;
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.scm");
        let app = r#"
(provide fetch)
(define url "http://127.0.0.1:PORT/hello")
(define (message thunk) (guard (e ((error-object? e) (error-object-message e))) (thunk)))
(define (fetch request env ctx)
  (let ((path (dict-ref request 'path)))
    (cond ((string=? path "/one") (utf8->string (dict-ref (fetch-url url) 'body)))
          ((string=? path "/private") (message (lambda () (fetch-url "http://10.0.0.1:PORT/") "fetched")))
          (else (message (lambda () (fetch-url url) (fetch-url url) (fetch-url url) "three"))))))
"#;
        std::fs::write(&entry, app.replace("PORT", &port.to_string())).unwrap();
        let host = Host::new();
        host.set_broker(outbound::through(std::rc::Rc::new(loopback(port))));
        host.publish("app", Deployment::load(&entry, Limits { fetches: 2, ..Limits::default() }).unwrap());
        let driver = host.clone();
        tokio::task::spawn_local(async move { driver.run(None).await });
        let ask = async |path: &str| {
            let request = Data::Dict(vec![(Data::Symbol("path".into()), Data::String(path.into()))]);
            host.fetch("app", request, None).await.unwrap().head
        };
        assert_eq!(ask("/one").await, Data::String("hi".into()));
        let refused = ask("/private").await;
        assert!(matches!(&refused, Data::String(m) if m.contains("may not reach")), "{refused:?}");
        let third = ask("/many").await;
        assert!(matches!(&third, Data::String(m) if m.contains("2 times at most")), "{third:?}");
    });
}
