//! Linux 用: 再生する曲・動画を PC の中だけの HTTP (`http://127.0.0.1:<空いたポート>/<合言葉>/...`) で返す。
//!
//! WebKitGTK の `<audio>` / `<video>` は GStreamer で読み、GStreamer の WebKit のソース要素は
//! http / https / blob しか扱わない。独自プロトコルの `ipodaudio://` を渡すと「この形式は再生できない」
//! (MEDIA_ERR_SRC_NOT_SUPPORTED) になるので、Linux ではこちらを使う (配布リポジトリの Issue #28)。
//! 中身は `ipodaudio://` と同じ (`audio_protocol::serve`)。127.0.0.1 だけで待ち、推測できない合言葉が
//! パスの先頭に無い要求は断るので、ほかのアプリやブラウザのページからは読めない。

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::OnceLock;
use std::time::Duration;

use tauri::http::{header, Method, Request, Response};
use tauri::AppHandle;

use crate::audio_protocol;

/// 画面が曲の URL の頭に使う `http://127.0.0.1:<ポート>/<合言葉>/`。始められなかったら None
static BASE: OnceLock<String> = OnceLock::new();

/// 要求の頭 (要求行とヘッダー) の上限
const MAX_HEAD: usize = 16 * 1024;

/// 受け口を開く。失敗したら画面は今までどおり `ipodaudio://` を使う
pub fn start(app: AppHandle) {
    if !cfg!(target_os = "linux") {
        return;
    }
    let Some(token) = random_token() else {
        log::warn!("media server: no random token");
        return;
    };
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(l) => l,
        Err(e) => {
            log::warn!("media server: bind failed: {e}");
            return;
        }
    };
    let Ok(addr) = listener.local_addr() else { return };
    let _ = BASE.set(format!("http://127.0.0.1:{}/{token}/", addr.port()));
    log::info!("media server: 127.0.0.1:{}", addr.port());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let app = app.clone();
            let token = token.clone();
            // ファイルの読み込みや変換に時間がかかるので 1 つの接続ごとに別スレッド
            std::thread::spawn(move || handle(&app, stream, &token));
        }
    });
}

/// 画面が曲の URL の頭に使う場所。Linux 以外と、受け口を開けなかったときは None (`ipodaudio://` を使う)
#[tauri::command]
pub fn media_base_url() -> Option<String> {
    BASE.get().cloned()
}

/// 推測できない合言葉 (128 ビット、16 進)
fn random_token() -> Option<String> {
    use std::io::Read;
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).ok()?;
    Some(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// 1 つの接続で 1 つの要求に答えて閉じる (Connection: close)
fn handle(app: &AppHandle, stream: TcpStream, token: &str) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let Ok(read_half) = stream.try_clone() else { return };
    let Some(head) = read_head(BufReader::new(read_half)) else { return };
    let head_only = head.lines().next().is_some_and(|l| l.starts_with("HEAD "));
    let Some(req) = to_request(&head, token) else {
        let _ = write_response(stream, Response::builder().status(404).body(Vec::new()).unwrap(), head_only);
        return;
    };
    // 範囲の指定が無い要求 (WebKitGTK の最初の要求) は曲全体を 200 で流す。`serve` は範囲が無いと最初の
    // 一塊だけを 206 で返すが、範囲を頼んでいない相手は塊の長さを曲の長さとみなし、そこで曲が終わってしまう
    let _ = if req.method() == Method::GET && !req.headers().contains_key(header::RANGE) {
        stream_whole(app, req, stream, head_only)
    } else {
        write_response(stream, audio_protocol::serve(app, &req), head_only)
    };
}

/// 曲全体を 200 で返す。`serve` に一塊ずつ頼んで順に書く (曲全体をメモリに置かない)。
/// 相手が途中で接続を閉じたら (シークで別の範囲を頼み直したときなど) 書けなくなった所でやめる
fn stream_whole(app: &AppHandle, mut req: Request<Vec<u8>>, mut out: TcpStream, head_only: bool) -> std::io::Result<()> {
    req.headers_mut().insert(header::RANGE, header::HeaderValue::from_static("bytes=0-"));
    let first = audio_protocol::serve(app, &req);
    let Some((_, mut end, total)) = content_range(&first) else {
        // 失敗 (404 など) や空の曲はそのまま返す
        return write_response(out, first, head_only);
    };
    let (mut parts, body) = first.into_parts();
    parts.status = tauri::http::StatusCode::OK;
    parts.headers.remove(header::CONTENT_RANGE);
    write_head(&mut out, &Response::from_parts(parts, ()), total)?;
    if head_only {
        return out.flush();
    }
    out.write_all(&body)?;
    while end + 1 < total {
        let range = format!("bytes={}-", end + 1);
        req.headers_mut().insert(header::RANGE, header::HeaderValue::from_str(&range).unwrap());
        let next = audio_protocol::serve(app, &req);
        let Some((_, e, _)) = content_range(&next).filter(|(s, _, t)| *s == end + 1 && *t == total) else {
            // 途中で読めなくなった (iPod が外れたなど)。長さが足りないまま閉じる
            return Ok(());
        };
        out.write_all(next.body())?;
        end = e;
    }
    out.flush()
}

/// 206 の応答の Content-Range (`bytes <始め>-<終わり>/<全体>`)
fn content_range(res: &Response<Vec<u8>>) -> Option<(u64, u64, u64)> {
    if res.status() != tauri::http::StatusCode::PARTIAL_CONTENT {
        return None;
    }
    let v = res.headers().get(header::CONTENT_RANGE)?.to_str().ok()?;
    let (range, total) = v.strip_prefix("bytes ")?.split_once('/')?;
    let (a, b) = range.split_once('-')?;
    Some((a.parse().ok()?, b.parse().ok()?, total.parse().ok()?))
}

/// 空行までを読む。大きすぎる・途中で切れたら None
fn read_head(mut r: impl BufRead) -> Option<String> {
    let mut head = String::new();
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if line == "\r\n" || line == "\n" {
            return Some(head);
        }
        head.push_str(&line);
        if head.len() > MAX_HEAD {
            return None;
        }
    }
}

/// `GET /<合言葉>/<残り> HTTP/1.1` を `ipodaudio://localhost/<残り>` の要求にする。合言葉が違えば None
fn to_request(head: &str, token: &str) -> Option<Request<Vec<u8>>> {
    let mut lines = head.lines();
    let mut parts = lines.next()?.split(' ');
    let method = match parts.next()? {
        "GET" | "HEAD" => Method::GET,
        "OPTIONS" => Method::OPTIONS,
        _ => return None,
    };
    let rest = parts.next()?.strip_prefix('/')?.strip_prefix(token)?.strip_prefix('/')?;
    let mut req = Request::builder().method(method).uri(format!("ipodaudio://localhost/{rest}"));
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("range") {
                req = req.header(header::RANGE, value.trim());
            }
        }
    }
    req.body(Vec::new()).ok()
}

fn write_response(mut out: TcpStream, res: Response<Vec<u8>>, head_only: bool) -> std::io::Result<()> {
    let (parts, body) = res.into_parts();
    write_head(&mut out, &Response::from_parts(parts, ()), body.len() as u64)?;
    if !head_only {
        out.write_all(&body)?;
    }
    out.flush()
}

/// 状態の行とヘッダー (Content-Length と Connection: close を足す)
fn write_head(out: &mut TcpStream, res: &Response<()>, len: u64) -> std::io::Result<()> {
    let status = res.status();
    let mut text = format!("HTTP/1.1 {} {}\r\n", status.as_u16(), status.canonical_reason().unwrap_or(""));
    for (name, value) in res.headers() {
        if let Ok(v) = value.to_str() {
            text.push_str(&format!("{name}: {v}\r\n"));
        }
    }
    text.push_str(&format!("Content-Length: {len}\r\nConnection: close\r\n\r\n"));
    out.write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn maps_path_and_range() {
        let head = format!("GET /{TOKEN}/pc?p=%2Fmusic%2Fa.flac HTTP/1.1\r\nHost: 127.0.0.1\r\nrange: bytes=0-\r\n");
        let req = to_request(&head, TOKEN).unwrap();
        assert_eq!(req.uri().to_string(), "ipodaudio://localhost/pc?p=%2Fmusic%2Fa.flac");
        assert_eq!(req.headers().get(header::RANGE).unwrap(), "bytes=0-");
        assert_eq!(req.method(), Method::GET);
    }

    #[test]
    fn rejects_wrong_token_and_methods() {
        assert!(to_request("GET /pc?p=%2Fa HTTP/1.1\r\n", TOKEN).is_none());
        assert!(to_request(&format!("GET /{TOKEN}x/12 HTTP/1.1\r\n"), TOKEN).is_none());
        assert!(to_request("GET /0123/12 HTTP/1.1\r\n", TOKEN).is_none());
        assert!(to_request(&format!("POST /{TOKEN}/12 HTTP/1.1\r\n"), TOKEN).is_none());
    }

    #[test]
    fn reads_content_range() {
        let res = |status: u16, cr: &str| Response::builder().status(status).header(header::CONTENT_RANGE, cr).body(Vec::new()).unwrap();
        assert_eq!(content_range(&res(206, "bytes 0-2097151/13066600")), Some((0, 2097151, 13066600)));
        assert_eq!(content_range(&res(200, "bytes 0-1/2")), None);
        assert_eq!(content_range(&res(206, "bytes */100")), None);
    }

    #[test]
    fn reads_head_until_blank_line() {
        let raw = b"GET / HTTP/1.1\r\nRange: bytes=0-\r\n\r\nbody";
        assert_eq!(read_head(&raw[..]).unwrap(), "GET / HTTP/1.1\r\nRange: bytes=0-\r\n");
        assert!(read_head(&b"GET / HTTP/1.1\r\n"[..]).is_none());
    }
}
