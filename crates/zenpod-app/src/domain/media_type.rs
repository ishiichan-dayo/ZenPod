//! 画面 (WebView) に渡すデータの MIME タイプ

use ipod_core::domain::audio::Format;

/// WebView がそのまま再生できる形式の MIME タイプ。None なら再生用に変換する。
/// macOS の WebKit と Windows の WebView2 / WebKitGTK で再生できる形式が違う
pub fn native_audio_mime(format: Option<Format>, ext: &str, macos: bool) -> Option<&'static str> {
    match format {
        Some(Format::Mp3) => Some("audio/mpeg"),
        Some(Format::Aac) => Some("audio/mp4"),
        Some(Format::Wav) => Some("audio/wav"),
        Some(Format::Alac) if macos => Some("audio/mp4"),
        Some(Format::Aiff) if macos => Some("audio/aiff"),
        _ if ext == "flac" => Some("audio/flac"),
        _ if !macos && matches!(ext, "ogg" | "oga" | "opus") => Some("audio/ogg"),
        _ if !macos && ext == "webm" => Some("audio/webm"),
        _ => None,
    }
}

/// ダウンロードした画像の MIME タイプ (先頭の数バイトで見分ける。分からなければ JPEG)
pub fn image_mime(bytes: &[u8]) -> &'static str {
    match bytes.get(..4) {
        Some([0x89, b'P', b'N', b'G']) => "image/png",
        Some([b'R', b'I', b'F', b'F']) => "image/webp",
        _ => "image/jpeg",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_depends_on_platform() {
        assert_eq!(native_audio_mime(Some(Format::Mp3), "mp3", false), Some("audio/mpeg"));
        assert_eq!(native_audio_mime(Some(Format::Alac), "m4a", true), Some("audio/mp4"));
        assert_eq!(native_audio_mime(Some(Format::Alac), "m4a", false), None);
        assert_eq!(native_audio_mime(Some(Format::ConvertLossless), "flac", true), Some("audio/flac"));
        assert_eq!(native_audio_mime(Some(Format::ConvertLossy), "opus", false), Some("audio/ogg"));
        assert_eq!(native_audio_mime(Some(Format::ConvertLossy), "opus", true), None);
        assert_eq!(native_audio_mime(None, "ape", false), None);
    }

    #[test]
    fn image_by_magic() {
        assert_eq!(image_mime(b"\x89PNG...."), "image/png");
        assert_eq!(image_mime(b"RIFF....WEBP"), "image/webp");
        assert_eq!(image_mime(b"\xFF\xD8\xFF"), "image/jpeg");
        assert_eq!(image_mime(b""), "image/jpeg");
    }
}
