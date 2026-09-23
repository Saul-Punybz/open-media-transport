//! Socket behaviour probes run on the CI runners; see README.md. Not built
//! with the crate.
use std::io::Read;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn blocked_reader(s: TcpStream) -> mpsc::Receiver<(String, Duration)> {
    let (tx, rx) = mpsc::channel();
    let start = Instant::now();
    std::thread::spawn(move || {
        let mut s = s;
        let mut b = [0u8; 16];
        let r = s.read(&mut b);
        let _ = tx.send((format!("{r:?}"), start.elapsed()));
    });
    rx
}

fn pair() -> (TcpStream, TcpStream) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
    let (s, _) = l.accept().unwrap();
    (c, s)
}

#[test]
fn diag_shutdown_on_clone_wakes_blocked_read() {
    let (c, _peer) = pair();
    let rx = blocked_reader(c.try_clone().unwrap());
    std::thread::sleep(Duration::from_millis(300));
    let t = Instant::now();
    c.shutdown(Shutdown::Both).unwrap();
    let r = rx.recv_timeout(Duration::from_secs(3));
    eprintln!("DIAG clone shutdown(Both): {r:?} after {:?}", t.elapsed());
}

#[test]
fn diag_shutdown_read_on_clone() {
    let (c, _peer) = pair();
    let rx = blocked_reader(c.try_clone().unwrap());
    std::thread::sleep(Duration::from_millis(300));
    let t = Instant::now();
    c.shutdown(Shutdown::Read).unwrap();
    let r = rx.recv_timeout(Duration::from_secs(3));
    eprintln!("DIAG clone shutdown(Read): {r:?} after {:?}", t.elapsed());
}

#[test]
fn diag_peer_fin_after_shutdown() {
    // Our shutdown sends FIN; the peer answers with its own shutdown.
    let (c, peer) = pair();
    let rx = blocked_reader(c.try_clone().unwrap());
    std::thread::sleep(Duration::from_millis(300));
    c.shutdown(Shutdown::Both).unwrap();
    let mut p = peer;
    let mut b = [0u8; 16];
    let t = Instant::now();
    let pr = p.read(&mut b);
    eprintln!(
        "DIAG peer read after our shutdown: {pr:?} after {:?}",
        t.elapsed()
    );
    let _ = p.shutdown(Shutdown::Both);
    let r = rx.recv_timeout(Duration::from_secs(3));
    eprintln!(
        "DIAG our read after peer shutdown: {r:?} after {:?}",
        t.elapsed()
    );
}

#[test]
fn diag_connect_refused_time() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let a = l.local_addr().unwrap();
    drop(l);
    let t = Instant::now();
    let r = TcpStream::connect_timeout(&a, Duration::from_secs(5));
    eprintln!(
        "DIAG connect to closed port: {:?} after {:?}",
        r.map(|_| ()),
        t.elapsed()
    );
}

fn dual_stack() -> TcpListener {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP)).unwrap();
    s.set_only_v6(false).unwrap();
    s.bind(&std::net::SocketAddr::from(([0u16; 8], 0)).into())
        .unwrap();
    s.listen(5).unwrap();
    s.into()
}

#[test]
fn diag_specific_bind_over_dual_stack() {
    let d = dual_stack();
    let p = d.local_addr().unwrap().port();
    let r = TcpListener::bind(("127.0.0.1", p));
    eprintln!(
        "DIAG explicit 127.0.0.1:{p} over [::]:{p}: {:?}",
        r.as_ref().map(|_| ())
    );
    drop(r);
    let mut held = Vec::new();
    let mut hits = 0;
    for _ in 0..3000 {
        match TcpListener::bind("127.0.0.1:0") {
            Ok(l) => {
                if l.local_addr().unwrap().port() == p {
                    hits += 1;
                }
                held.push(l);
            }
            Err(e) => {
                eprintln!("DIAG bind error after {}: {e}", held.len());
                break;
            }
        }
    }
    eprintln!(
        "DIAG ephemeral 127.0.0.1:0 got the dual-stack port {hits} times of {}",
        held.len()
    );
    // Where does a connect to 127.0.0.1:p go when both exist?
    let d2 = dual_stack();
    let p2 = d2.local_addr().unwrap().port();
    if let Ok(spec) = TcpListener::bind(("127.0.0.1", p2)) {
        spec.set_nonblocking(true).unwrap();
        d2.set_nonblocking(true).unwrap();
        let _c = TcpStream::connect(("127.0.0.1", p2)).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        eprintln!(
            "DIAG connect 127.0.0.1 went to: specific={} dual={}",
            spec.accept().is_ok(),
            d2.accept().is_ok()
        );
    }
}

#[test]
fn diag_shutdown_wakes_blocked_write() {
    let (c, _peer) = pair();
    let w = c.try_clone().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        use std::io::Write;
        let mut w = w;
        let big = vec![0u8; 1 << 20];
        let start = Instant::now();
        let r = loop {
            if let Err(e) = w.write_all(&big) {
                break e;
            }
        };
        let _ = tx.send((format!("{r:?}"), start.elapsed()));
    });
    std::thread::sleep(Duration::from_millis(1000));
    let t = Instant::now();
    c.shutdown(Shutdown::Both).unwrap();
    let r = rx.recv_timeout(Duration::from_secs(3));
    eprintln!(
        "DIAG blocked write after clone shutdown: {r:?} after {:?}",
        t.elapsed()
    );
}

#[test]
fn diag_dual_stack_ephemeral_over_held_v4() {
    let mut v4 = std::collections::HashSet::new();
    let mut held = Vec::new();
    for _ in 0..3000 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        v4.insert(l.local_addr().unwrap().port());
        held.push(l);
    }
    let mut hits = Vec::new();
    let mut duals = Vec::new();
    for _ in 0..500 {
        let d = dual_stack();
        let p = d.local_addr().unwrap().port();
        if v4.contains(&p) {
            hits.push(p);
        }
        duals.push(d);
    }
    eprintln!("DIAG dual-stack [::]:0 got a port held by 127.0.0.1: {} of 500 {hits:?}", hits.len());
}
