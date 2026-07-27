use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX: usize = 10_000_000;
const B: usize = 37;
static SEQ: AtomicU64 = AtomicU64::new(0);

struct R([u64; 4]);
impl R {
    fn new(mut z: u64) -> Self {
        let mut s = [0u64; 4];
        for x in &mut s {
            z = z.wrapping_add(0x9e3779b97f4a7c15);
            let mut y = z;
            y = (y ^ y >> 30).wrapping_mul(0xbf58476d1ce4e5b9);
            y = (y ^ y >> 27).wrapping_mul(0x94d049bb133111eb);
            *x = y ^ y >> 31;
        }
        if s == [0; 4] { s[0] = 1 }
        Self(s)
    }
    fn next(&mut self) -> u64 {
        let r = (self.0[0].wrapping_add(self.0[3]))
            .rotate_left(23)
            .wrapping_add(self.0[0]);
        let t = self.0[1] << 17;
        self.0[2] ^= self.0[0];
        self.0[3] ^= self.0[1];
        self.0[1] ^= self.0[2];
        self.0[0] ^= self.0[3];
        self.0[3] = (self.0[3] ^ t).rotate_left(45);
        r
    }
    fn uuid(&mut self) -> [u8; 16] {
        let mut b = ((self.next() as u128) << 64 | self.next() as u128).to_be_bytes();
        b[6] = b[6] & 0xf | 0x40;
        b[8] = b[8] & 0x3f | 0x80;
        b
    }
}

fn seed() -> u64 {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    t ^ SEQ.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9e3779b97f4a7c15)
}

fn fmt(o: &mut [u8], b: &[u8; 16]) {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut p = 0;
    for (i, &v) in b.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            o[p] = b'-';
            p += 1;
        }
        o[p] = H[v as usize >> 4];
        o[p + 1] = H[v as usize & 15];
        p += 2;
    }
    o[p] = b'\n';
}

fn fill(buf: &mut [u8], n: usize, s: u64) {
    let mut r = R::new(s);
    for i in 0..n {
        let u = r.uuid();
        fmt(&mut buf[i * B..(i + 1) * B], &u);
    }
}

fn dedup(buf: &mut [u8], s: u64) {
    let mut seen: HashSet<[u8; 36]> = HashSet::new();
    let mut r = R::new(s);
    for chunk in buf.chunks_mut(B) {
        let mut k = [0u8; 36];
        k.copy_from_slice(&chunk[..36]);
        while !seen.insert(k) {
            let u = r.uuid();
            fmt(chunk, &u);
            k.copy_from_slice(&chunk[..36]);
        }
    }
}

fn make(n: usize) -> Vec<u8> {
    let n = n.min(MAX);
    if n == 0 { return vec![] }
    let mut buf = vec![0u8; n * B];
    if n < 8192 {
        fill(&mut buf, n, seed());
    } else {
        let w = thread::available_parallelism()
            .map(|v| v.get())
            .unwrap_or(4)
            .min(n);
        let s = seed();
        let c = (n + w - 1) / w;
        thread::scope(|sc| {
            for (i, sl) in buf.chunks_mut(c * B).enumerate() {
                let cnt = sl.len() / B;
                let ts = s ^ (i as u64 + 1).wrapping_mul(0xbf58476d1ce4e5b9);
                sc.spawn(move || fill(sl, cnt, ts));
            }
        });
    }
    dedup(&mut buf, seed());
    buf
}

fn parse_n(req: &[u8]) -> usize {
    let Some(t) = req.split(|&b| b == b' ').nth(1) else { return 1 };
    let Some(q) = t.split(|&b| b == b'?').nth(1) else { return 1 };
    for kv in q.split(|&b| b == b'&') {
        if kv.starts_with(b"n=") {
            return kv[2..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .fold(0usize, |a, &c| {
                    a.saturating_mul(10).saturating_add((c - b'0') as usize)
                })
                .min(MAX);
        }
    }
    1
}

fn handle(mut s: TcpStream) {
    let _ = s.set_nodelay(true);
    let mut buf = [0u8; 4096];
    let Ok(n) = s.read(&mut buf) else { return };
    if n == 0 { return }
    let req = &buf[..n];
    if req.split(|&b| b == b' ').nth(1).is_some_and(|t| t.starts_with(b"/favicon")) {
        let _ = s.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
        return;
    }
    let body = make(parse_n(req));
    let _ = write!(
        s,
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(&body);
}

fn main() {
    let l = TcpListener::bind("127.0.0.1:4545").expect("bind");
    println!("http://127.0.0.1:4545");
    for s in l.incoming().flatten() {
        thread::spawn(|| handle(s));
    }
}
