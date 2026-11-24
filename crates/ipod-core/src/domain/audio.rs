//! 音声の形式と曲の種類、転送するときの変換先 (値と規則だけ。ファイルは読まない)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Mp3,
    Aac,
    Alac,
    Wav,
    Aiff,
    /// ALAC に変換して転送 (FLAC など)
    ConvertLossless,
    /// AAC に変換して転送 (Ogg / Opus / WMA など)
    ConvertLossy,
}

impl Format {
    pub fn needs_conversion(self) -> bool {
        matches!(self, Format::ConvertLossless | Format::ConvertLossy)
    }
}

/// 曲の種類。iPod OS では iTunesDB の mediatype と「続きから再生」などの印、
/// Rockbox では置くフォルダ (Podcasts / Audiobooks) で表す
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    #[default]
    Music,
    Podcast,
    Audiobook,
}

/// 変換先の形式。設定には "alac" / "aac:256" / "mp3:320" の文字列で持つ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvertTarget {
    /// 劣化なし (16bit / 48kHz まで)
    Alac,
    Aac { kbps: u32 },
    Mp3 { kbps: u32 },
}

impl ConvertTarget {
    /// 可逆圧縮の曲の既定: ALAC
    pub const LOSSLESS_DEFAULT: ConvertTarget = ConvertTarget::Alac;
    /// 非可逆の曲の既定: AAC 256kbps
    pub const LOSSY_DEFAULT: ConvertTarget = ConvertTarget::Aac { kbps: 256 };

    /// 設定の文字列を読む。知らない値・範囲外のビットレートは None
    pub fn parse(s: &str) -> Option<Self> {
        let (codec, rate) = s.split_once(':').unwrap_or((s, ""));
        let kbps = |max: u32| rate.parse::<u32>().ok().filter(|k| (64..=max).contains(k));
        match codec {
            "alac" => Some(ConvertTarget::Alac),
            "aac" => kbps(320).map(|kbps| ConvertTarget::Aac { kbps }),
            "mp3" => kbps(320).map(|kbps| ConvertTarget::Mp3 { kbps }),
            _ => None,
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            ConvertTarget::Mp3 { .. } => "mp3",
            _ => "m4a",
        }
    }

    /// 変換後のファイルの種類 (iTunesDB に書く形式)
    pub fn format(self) -> Format {
        match self {
            ConvertTarget::Alac => Format::Alac,
            ConvertTarget::Aac { .. } => Format::Aac,
            ConvertTarget::Mp3 { .. } => Format::Mp3,
        }
    }

    /// 「変換して転送」(形式を選んで送る) のとき、その曲をこの形式にするか。`bitrate_kbps` は元の曲のビットレート (0 = 分からない)。
    /// ロスレスの曲 (ALAC・WAV・AIFF・FLAC など) と、iPod で再生できない形式はいつも変換する。MP3・AAC は選んだビットレートより
    /// 高いときだけ (iTunes の「ビットレートの高い曲を変換」と同じ。低い曲を上げても音は良くならず、大きくなるだけ)。
    /// 可変ビットレートの曲は少し上下するので、1 割の幅を見る
    pub fn should_convert(self, fmt: Format, bitrate_kbps: u32) -> bool {
        match fmt {
            Format::ConvertLossless | Format::ConvertLossy => true,
            Format::Wav | Format::Aiff => true,
            Format::Alac => self != ConvertTarget::Alac,
            Format::Mp3 | Format::Aac => match self {
                ConvertTarget::Alac => false,
                ConvertTarget::Aac { kbps } | ConvertTarget::Mp3 { kbps } => bitrate_kbps > kbps + kbps / 10,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_lossless_and_higher_bitrates_only() {
        let aac = ConvertTarget::Aac { kbps: 192 };
        for f in [Format::Alac, Format::Wav, Format::Aiff, Format::ConvertLossless, Format::ConvertLossy] {
            assert!(aac.should_convert(f, 0), "{f:?}");
        }
        assert!(aac.should_convert(Format::Mp3, 320));
        assert!(aac.should_convert(Format::Aac, 256));
        // 同じくらい・低いビットレート、分からないものは送るだけ
        assert!(!aac.should_convert(Format::Mp3, 200));
        assert!(!aac.should_convert(Format::Aac, 128));
        assert!(!aac.should_convert(Format::Mp3, 0));
        // ALAC にするときは、非可逆の曲と ALAC の曲はそのまま
        assert!(!ConvertTarget::Alac.should_convert(Format::Mp3, 320));
        assert!(!ConvertTarget::Alac.should_convert(Format::Alac, 900));
        assert!(ConvertTarget::Alac.should_convert(Format::Wav, 1411));
    }
}
