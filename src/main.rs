use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_N: usize = 100;
static SEQ: AtomicU64 = AtomicU64::new(0);
const HEX: &[u8; 16] = b"0123456789abcdef";

struct Rng([u64; 4]);

impl Rng {
    #[inline(always)]
    fn new(seed: u64) -> Self {
        let mut z = seed;
        let mut s = [0u64; 4];

        for x in &mut s {
            z = z.wrapping_add(0x9e3779b97f4a7c15);
            let mut y = z;
            y = (y ^ (y >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            y = (y ^ (y >> 27)).wrapping_mul(0x94d049bb133111eb);
            *x = y ^ (y >> 31);
        }

        if s.iter().all(|&x| x == 0) {
            s[0] = 1;
        }

        Self(s)
    }

    #[inline(always)]
    fn next(&mut self) -> u64 {
        let result = (self.0[0].wrapping_add(self.0[3]))
            .rotate_left(23)
            .wrapping_add(self.0[0]);

        let t = self.0[1] << 17;

        self.0[2] ^= self.0[0];
        self.0[3] ^= self.0[1];
        self.0[1] ^= self.0[2];
        self.0[0] ^= self.0[3];
        self.0[3] ^= t;
        self.0[3] = self.0[3].rotate_left(45);

        result
    }

    #[inline(always)]
    fn uuid(&mut self) -> u128 {
        let mut x = ((self.next() as u128) << 64) | self.next() as u128;

        // version 4
        x = (x & !(0xfu128 << 76)) | (4u128 << 76);

        // variant 10xxxxxx
        x = (x & !(0x3u128 << 62)) | (2u128 << 62);

        x
    }
}

#[inline(always)]
fn push_uuid(out: &mut Vec<u8>, x: u128) {
    let b = x.to_be_bytes();

    for (i, &c) in b.iter().enumerate() {
        if i == 4 || i == 6 || i == 8 || i == 10 {
            out.push(b'-');
        }

        out.push(HEX[(c >> 4) as usize]);
        out.push(HEX[(c & 15) as usize]);
    }

    out.push(b'\n');
}

fn make_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ SEQ
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9e3779b97f4a7c15)
}

fn gen_chunk(count: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(count * 37);

    for _ in 0..count {
        push_uuid(&mut out, rng.uuid());
    }

    out
}

fn gen(n: usize) -> Vec<Vec<u8>> {
    let n = n.min(MAX_N);

    if n == 0 {
        return Vec::new();
    }

    let seed = make_seed();

    // 小さいリクエストはスレッド生成コストを避ける
    if n < 8192 {
        return vec![gen_chunk(n, seed)];
    }

    let workers = thread::available_parallelism()
        .map(|v| v.get())
        .unwrap_or(4)
        .min(n);

    let chunk = (n + workers - 1) / workers;

    thread::scope(|s| {
        let mut handles = Vec::with_capacity(workers);

        for w in 0..workers {
            let start = w * chunk;
            let end = (start + chunk).min(n);

            if start >= end {
                break;
            }

            let count = end - start;
            let thread_seed = seed ^ (w as u64 + 1).wrapping_mul(0xbf58476d1ce4e5b9);

            handles.push(s.spawn(move || gen_chunk(count, thread_seed)));
        }

        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect()
    })
}

fn parse_n(req: &str) -> usize {
    let Some(target) = req.split_whitespace().nth(1) else {
        return 1;
    };

    let Some(query) = target.split_once('?').map(|(_, q)| q) else {
        return 1;
    };

    for kv in query.split('&') {
        if let Some(v) = kv.strip_prefix("n=") {
            let end = v
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(v.len());

            if let Ok(n) = v[..end].parse::<usize>() {
                return n.min(MAX_N);
            }
        }
    }

    1
}

fn handle(mut stream: TcpStream) {
    let _ = stream.set_nodelay(true);

    let mut buf = [0u8; 4096];
    let nread = match stream.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return,
    };

    let req = String::from_utf8_lossy(&buf[..nread]);
    let target = req.split_whitespace().nth(1).unwrap_or("/");

    if target.starts_with("/favicon.ico") {
        let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
        return;
    }

    let n = parse_n(&req);
    let chunks = gen(n);

    let len: usize = chunks.iter().map(|c| c.len()).sum();

    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
    );

    let _ = stream.write_all(head.as_bytes());

    for chunk in chunks {
        let _ = stream.write_all(&chunk);
    }
}

fn main() {
    let addr = "127.0.0.1:4545";
    let listener = TcpListener::bind(addr).expect("bind failed");

    println!("http://{addr}");

    for stream in listener.incoming() {
        if let Ok(stream) = stream {
            thread::spawn(|| handle(stream));
        }
    }
}