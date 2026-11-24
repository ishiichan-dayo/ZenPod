//! `ipodaudio://localhost/<曲 ID>` で iPod の曲を、`ipodaudio://localhost/pc?p=<場所>` で PC の登録フォルダの曲を返す

use std::path::PathBuf;

use tauri::http::{header, Method, Request, Response, StatusCode};
use tauri::{AppHandle, Manager};
use zenpod_app::application::playback::{AudioReply, AudioTarget};

use crate::state::AppState;

pub fn serve(app: &AppHandle, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    if req.method() == Method::OPTIONS {
        return Response::builder()
            .status(StatusCode::NO_CONTENT)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "Range")
            .body(Vec::new())
            .unwrap();
    }
    let range = req.headers().get(header::RANGE).and_then(|v| v.to_str().ok());
    let reply = app.state::<AppState>().zen.playback.serve(target(req), range);
    match reply {
        AudioReply::Partial { mime, start, end, total, body } => Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_TYPE, mime)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{total}"))
            .header(header::CACHE_CONTROL, "no-store")
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::ACCESS_CONTROL_EXPOSE_HEADERS, "Content-Range")
            .body(body)
            .unwrap(),
        AudioReply::Unsatisfiable { total } => Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{total}"))
            .body(Vec::new())
            .unwrap(),
        // 画面 (開発時は Vite のオリジン) から失敗理由を fetch で読めるようにする
        AudioReply::Fail { status, message } => Response::builder()
            .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(message.into_bytes())
            .unwrap(),
    }
}

/// /<曲 ID> = iPod の曲、/video/<動画 ID> = iPod の動画、/pc?p=<場所> = PC の登録フォルダの曲
fn target(req: &Request<Vec<u8>>) -> AudioTarget {
    let path = req.uri().path().trim_matches('/');
    if let Some(id) = path.strip_prefix("video/") {
        return AudioTarget::Video(id.to_string());
    }
    if path != "pc" {
        return AudioTarget::Track(path.to_string());
    }
    let url = tauri::Url::parse(&req.uri().to_string()).ok();
    let p = url.and_then(|u| u.query_pairs().find(|(k, _)| k == "p").map(|(_, v)| PathBuf::from(v.as_ref())));
    // 場所が無ければ空の場所にする (登録フォルダの外なので 403 になる)
    AudioTarget::Pc(p.unwrap_or_default())
}
