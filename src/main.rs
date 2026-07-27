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
    #[inline(always)]
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
    #[inline(always)]
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
    #[inline(always)]
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

const HEX: [u16; 256] = {
    let mut t = [0u16; 256];
    let h = b"0123456789abcdef";
    let mut i = 0;
    while i < 256 {
        t[i] = ((h[i >> 4] as u16) << 8) | h[i & 15] as u16;
        i += 1;
    }
    t
};

#[inline(always)]
fn fmt(o: &mut [u8], b: &[u8; 16]) {
    let mut p = 0;
    let mut i = 0;
    while i < 16 {
        if i == 4 || i == 6 || i == 8 || i == 10 {
            o[p] = b'-';
            p += 1;
        }
        let h = HEX[b[i] as usize].to_ne_bytes();
        o[p] = h[0];
        o[p + 1] = h[1];
        p += 2;
        i += 1;
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

const HDR: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: ";
const HDR_END: &[u8] = b"\r\nConnection: close\r\n\r\n";

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

    let len_str = body.len().to_string();
    let mut hdr = Vec::with_capacity(HDR.len() + len_str.len() + HDR_END.len());
    hdr.extend_from_slice(HDR);
    hdr.extend_from_slice(len_str.as_bytes());
    hdr.extend_from_slice(HDR_END);

    let _ = s.write_all(&hdr);
    let _ = s.write_all(&body);
}

fn main() {
    let l = TcpListener::bind("127.0.0.1:4545").expect("bind");
    println!("http://127.0.0.1:4545");
    for s in l.incoming().flatten() {
        thread::spawn(|| handle(s));
    }
}
