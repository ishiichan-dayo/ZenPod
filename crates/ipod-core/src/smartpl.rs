//! スマートプレイリストの条件 (SLst) と設定。
//!
//! - 条件は iTunesDB の MHOD 51 と iTunes の XML の Smart Criteria で同じ形 (ビッグエンディアン)
//! - 設定は MHOD 50 (iTunesDB) と Smart Info (XML) で並びが違うので、`SmartPrefs` を間に置いて変換する

use crate::error::{Error, Result};
use crate::t;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const SLST_MAGIC: &[u8; 4] = b"SLst";
const SLST_HLEN: usize = 136;
/// 条件 1 つの見出し (項目・比較・44 バイト・データの長さ)
const RULE_HLEN: usize = 56;
/// 文字以外の条件のデータの長さ
const NUM_LEN: usize = 0x44;
/// 入れ子のグループの深さの上限 (壊れたデータで読む・比べるときにスタックを使い切らないように)
const MAX_DEPTH: usize = 32;
const MHOD_HLEN: usize = 24;
/// MHOD 50 の全体の長さ (iTunes・libgpod とも 96)
const MHOD50_LEN: usize = 96;
/// XML の Smart Info の長さ
const XML_INFO_LEN: usize = 112;

/// 比較 (action) のビット: 文字の比較
pub const ACTION_STRING: u32 = 0x0100_0000;
/// 比較 (action) のビット: 否定 (ではない・含まない など)
pub const ACTION_NOT: u32 = 0x0200_0000;

/// 条件の並び。`any` が true なら「いずれかに一致」
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rules {
    pub any: bool,
    pub items: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Rule {
    /// 入れ子のグループ (iTunes の条件の画面で「…」から足すもの、対象メディアの包み)
    Group(Rules),
    Cond { field: u32, action: u32, value: Value },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Str(String),
    Num(NumValue),
}

/// 文字以外の条件の値。単位はふつう 1 で、「過去 N 日以内」のときだけ from_date = -N・from_units = 秒。
/// 画面へは文字で渡す (JavaScript の数では 2^53 を超える値が丸められるため)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NumValue {
    #[serde(with = "i64_str")]
    pub from: i64,
    #[serde(with = "i64_str")]
    pub from_date: i64,
    #[serde(with = "i64_str")]
    pub from_units: i64,
    #[serde(with = "i64_str")]
    pub to: i64,
    #[serde(with = "i64_str")]
    pub to_date: i64,
    #[serde(with = "i64_str")]
    pub to_units: i64,
}

mod i64_str {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &i64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
        String::deserialize(d)?.parse().map_err(serde::de::Error::custom)
    }
}

impl NumValue {
    pub fn single(v: i64) -> Self {
        Self::range(v, v)
    }
    pub fn range(from: i64, to: i64) -> Self {
        NumValue {
            from,
            from_units: 1,
            to,
            to_units: 1,
            ..Default::default()
        }
    }
}

fn rd32be(b: &[u8], o: usize) -> Result<u32> {
    b.get(o..o + 4)
        .map(|s| u32::from_be_bytes(s.try_into().unwrap()))
        .ok_or_else(|| Error::Other(t!("SLst: 途中で切れています")))
}

fn rd64be(b: &[u8], o: usize) -> i64 {
    i64::from_be_bytes(b[o..o + 8].try_into().unwrap())
}

impl Rules {
    pub fn parse(b: &[u8]) -> Result<Rules> {
        Rules::parse_at(b, 0)
    }

    fn parse_at(b: &[u8], depth: usize) -> Result<Rules> {
        if b.get(..4) != Some(SLST_MAGIC.as_slice()) || b.len() < SLST_HLEN {
            return Err(Error::Other(t!("SLst ではありません")));
        }
        if depth > MAX_DEPTH {
            return Err(Error::Other(t!("SLst: 入れ子が深すぎます")));
        }
        let n = rd32be(b, 8)?;
        let any = rd32be(b, 12)? == 1;
        // 数は壊れていることがあるので、入りきる数までしか先に確保しない
        let mut items = Vec::with_capacity((n as usize).min((b.len() - SLST_HLEN) / RULE_HLEN));
        let mut p = SLST_HLEN;
        for _ in 0..n {
            let field = rd32be(b, p)?;
            let action = rd32be(b, p + 4)?;
            let len = rd32be(b, p + 52)? as usize;
            let data = b
                .get(p + RULE_HLEN..p + RULE_HLEN + len)
                .ok_or_else(|| Error::Other(t!("SLst: 途中で切れています")))?;
            let rule = if field == 0 && data.starts_with(SLST_MAGIC) {
                Rule::Group(Rules::parse_at(data, depth + 1)?)
            } else if action & ACTION_STRING != 0 {
                let units: Vec<u16> = data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_be_bytes(*c))
                    .collect();
                Rule::Cond {
                    field,
                    action,
                    value: Value::Str(String::from_utf16_lossy(&units)),
                }
            } else {
                if len < NUM_LEN {
                    return Err(Error::Other(t!("SLst: 値の長さが違います")));
                }
                Rule::Cond {
                    field,
                    action,
                    value: Value::Num(NumValue {
                        from: rd64be(data, 0),
                        from_date: rd64be(data, 8),
                        from_units: rd64be(data, 16),
                        to: rd64be(data, 24),
                        to_date: rd64be(data, 32),
                        to_units: rd64be(data, 40),
                    }),
                }
            };
            items.push(rule);
            p += RULE_HLEN + len;
        }
        Ok(Rules { any, items })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(SLST_HLEN + self.items.len() * (RULE_HLEN + NUM_LEN));
        out.extend_from_slice(SLST_MAGIC);
        out.extend_from_slice(&0x0001_0001u32.to_be_bytes());
        out.extend_from_slice(&(self.items.len() as u32).to_be_bytes());
        out.extend_from_slice(&(self.any as u32).to_be_bytes());
        out.resize(SLST_HLEN, 0);
        for r in &self.items {
            let (field, action, data, group) = match r {
                Rule::Group(g) => (0, 1, g.to_bytes(), true),
                Rule::Cond {
                    field,
                    action,
                    value: Value::Str(s),
                } => (
                    *field,
                    *action,
                    s.encode_utf16().flat_map(u16::to_be_bytes).collect(),
                    false,
                ),
                Rule::Cond {
                    field,
                    action,
                    value: Value::Num(v),
                } => {
                    let mut d = Vec::with_capacity(NUM_LEN);
                    for x in [v.from, v.from_date, v.from_units, v.to, v.to_date, v.to_units] {
                        d.extend_from_slice(&x.to_be_bytes());
                    }
                    d.resize(NUM_LEN, 0);
                    (*field, *action, d, false)
                }
            };
            out.extend_from_slice(&field.to_be_bytes());
            out.extend_from_slice(&action.to_be_bytes());
            let mut pad = [0u8; 44];
            // グループの印 (iTunes が書く形)
            pad[0] = group as u8;
            out.extend_from_slice(&pad);
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(&data);
        }
        out
    }

    /// iTunesDB の MHOD 51 (見出し 24 バイト付き)
    pub fn to_mhod(&self) -> Vec<u8> {
        mhod(51, &self.to_bytes())
    }

    /// iTunesDB の MHOD 51 から読む
    pub fn from_mhod(m: &[u8]) -> Result<Rules> {
        Rules::parse(m.get(MHOD_HLEN..).unwrap_or_default())
    }
}

fn mhod(kind: u32, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(MHOD_HLEN + body.len());
    out.extend_from_slice(b"mhod");
    out.extend_from_slice(&(MHOD_HLEN as u32).to_le_bytes());
    out.extend_from_slice(&((MHOD_HLEN + body.len()) as u32).to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.resize(MHOD_HLEN, 0);
    out.extend_from_slice(body);
    out
}

// ---- 上限の単位 (iTunesDB と XML で同じ) ----
pub const LIMIT_MINUTES: u8 = 1;
pub const LIMIT_MB: u8 = 2;
pub const LIMIT_ITEMS: u8 = 3;
pub const LIMIT_HOURS: u8 = 4;
pub const LIMIT_GB: u8 = 5;

// ---- 上限の選択方法 (XML の値。逆順は `SmartPrefs::reverse_sort`) ----
pub const SORT_RANDOM: u32 = 0x02;
pub const SORT_NAME: u32 = 0x05;
pub const SORT_ALBUM: u32 = 0x06;
pub const SORT_ARTIST: u32 = 0x07;
pub const SORT_GENRE: u32 = 0x09;
pub const SORT_ADDED: u32 = 0x15;
pub const SORT_PLAY_COUNT: u32 = 0x19;
pub const SORT_LAST_PLAYED: u32 = 0x1a;
pub const SORT_RATING: u32 = 0x1c;

/// XML の選択方法を iTunesDB の値にする。名前・アルバム・アーティスト・ジャンルは libgpod の値
/// (iTunes が iPod に書いた値では未確認)
fn sort_to_db(xml: u32) -> u8 {
    match xml {
        SORT_NAME => 0x03,
        SORT_ALBUM => 0x04,
        SORT_ARTIST => 0x05,
        SORT_GENRE => 0x07,
        v => v as u8,
    }
}

fn sort_from_db(db: u8) -> u32 {
    match db {
        0x03 => SORT_NAME,
        0x04 => SORT_ALBUM,
        0x05 => SORT_ARTIST,
        0x07 => SORT_GENRE,
        v => v as u32,
    }
}

/// 上限・ライブアップデートなどの設定
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartPrefs {
    pub live_update: bool,
    /// 条件を使う (iTunes はいつも true)
    pub check_rules: bool,
    pub check_limits: bool,
    pub limit_type: u8,
    /// XML の値 (`SORT_*`)
    pub limit_sort: u32,
    pub reverse_sort: bool,
    pub limit_value: u32,
    pub match_checked: bool,
}

impl Default for SmartPrefs {
    /// iTunes で新しく作ったときと同じ
    fn default() -> Self {
        SmartPrefs {
            live_update: true,
            check_rules: true,
            check_limits: false,
            limit_type: LIMIT_ITEMS,
            limit_sort: SORT_RANDOM,
            reverse_sort: false,
            limit_value: 25,
            match_checked: false,
        }
    }
}

impl SmartPrefs {
    /// iTunesDB の MHOD 50 (見出し 24 バイト付き) から読む
    pub fn from_mhod(m: &[u8]) -> Result<SmartPrefs> {
        let b = m
            .get(MHOD_HLEN..MHOD_HLEN + 14)
            .ok_or_else(|| Error::Other(t!("MHOD 50 が短すぎます")))?;
        Ok(SmartPrefs {
            live_update: b[0] != 0,
            check_rules: b[1] != 0,
            check_limits: b[2] != 0,
            limit_type: b[3],
            limit_sort: sort_from_db(b[4]),
            limit_value: u32::from_le_bytes(b[8..12].try_into().unwrap()),
            match_checked: b[12] != 0,
            reverse_sort: b[13] != 0,
        })
    }

    /// iTunesDB の MHOD 50 にする。`base` があれば、知らない場所はその値を残す
    pub fn to_mhod(&self, base: Option<&[u8]>) -> Vec<u8> {
        let mut body = match base {
            Some(m) if m.len() >= MHOD50_LEN => m[MHOD_HLEN..].to_vec(),
            _ => {
                let mut b = vec![0u8; MHOD50_LEN - MHOD_HLEN];
                // 意味は分からないが iTunes が書く値
                b[16] = 5;
                b
            }
        };
        body[0] = self.live_update as u8;
        body[1] = self.check_rules as u8;
        body[2] = self.check_limits as u8;
        body[3] = self.limit_type;
        body[4] = sort_to_db(self.limit_sort);
        body[8..12].copy_from_slice(&self.limit_value.to_le_bytes());
        body[12] = self.match_checked as u8;
        body[13] = self.reverse_sort as u8;
        mhod(50, &body)
    }

    /// XML の Smart Info から読む
    pub fn from_xml_info(b: &[u8]) -> Result<SmartPrefs> {
        if b.len() < 14 {
            return Err(Error::Other(t!("Smart Info が短すぎます")));
        }
        Ok(SmartPrefs {
            live_update: b[0] != 0,
            check_rules: b[1] != 0,
            check_limits: b[2] != 0,
            limit_type: b[3],
            limit_sort: u32::from_be_bytes(b[4..8].try_into().unwrap()),
            limit_value: u32::from_be_bytes(b[8..12].try_into().unwrap()),
            match_checked: b[12] != 0,
            reverse_sort: b[13] != 0,
        })
    }

    /// XML の Smart Info にする。`base` があれば、知らない場所はその値を残す
    pub fn to_xml_info(&self, base: Option<&[u8]>) -> Vec<u8> {
        let mut b = match base {
            Some(b) if b.len() >= XML_INFO_LEN => b.to_vec(),
            _ => {
                let mut b = vec![0u8; XML_INFO_LEN];
                // 意味は分からないが iTunes が書く値
                b[19] = 7;
                b
            }
        };
        b[0] = self.live_update as u8;
        b[1] = self.check_rules as u8;
        b[2] = self.check_limits as u8;
        b[3] = self.limit_type;
        b[4..8].copy_from_slice(&self.limit_sort.to_be_bytes());
        b[8..12].copy_from_slice(&self.limit_value.to_be_bytes());
        b[12] = self.match_checked as u8;
        b[13] = self.reverse_sort as u8;
        b
    }
}

// ============================================================
// 画面とのやり取り (対象メディア)
// ============================================================

/// iTunes の条件の画面の「対象メディア」
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaTarget {
    All,
    Music,
    Movies,
    Tv,
    Podcasts,
    Audiobooks,
}

impl MediaTarget {
    /// iTunes が条件全体を包むときのメディアの種類の条件 (比較, 値)。「すべてのメディア」は包まない
    fn conds(self) -> &'static [(u32, i64)] {
        match self {
            MediaTarget::All => &[],
            MediaTarget::Music => &[(OP_IS, 1), (OP_IS, 32)],
            MediaTarget::Movies => &[(OP_IS, 2), (OP_IS, 1024)],
            MediaTarget::Tv => &[(OP_IS, 64)],
            MediaTarget::Podcasts => &[(OP_BIT_AND, 4)],
            MediaTarget::Audiobooks => &[(OP_IS, 8)],
        }
    }
}

/// 条件の画面で編集する形。iTunes の対象メディアの包みは `media` に分けて、`rules` は画面に出す条件だけ
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartDef {
    pub media: MediaTarget,
    pub rules: Rules,
    pub prefs: SmartPrefs,
}

impl SmartDef {
    pub fn from_parts(prefs: SmartPrefs, rules: Rules) -> SmartDef {
        if let Some((media, inner)) = split_media(&rules) {
            return SmartDef {
                media,
                rules: inner,
                prefs,
            };
        }
        SmartDef {
            media: MediaTarget::All,
            rules,
            prefs,
        }
    }

    pub fn into_parts(self) -> (SmartPrefs, Rules) {
        let conds = self.media.conds();
        if conds.is_empty() {
            return (self.prefs, self.rules);
        }
        let wrap = Rules {
            any: true,
            items: conds
                .iter()
                .map(|&(op, v)| Rule::Cond {
                    field: F_MEDIA_KIND,
                    action: op,
                    value: Value::Num(NumValue::single(v)),
                })
                .collect(),
        };
        (
            self.prefs,
            Rules {
                any: false,
                items: vec![Rule::Group(wrap), Rule::Group(self.rules)],
            },
        )
    }
}

/// iTunes の対象メディアの包み (すべてに一致の 2 つのグループで、1 つ目がメディアの種類だけ) なら分ける
fn split_media(rules: &Rules) -> Option<(MediaTarget, Rules)> {
    let [Rule::Group(wrap), Rule::Group(inner)] = rules.items.as_slice() else {
        return None;
    };
    if rules.any || !wrap.any {
        return None;
    }
    let mut got: Vec<(u32, i64)> = wrap
        .items
        .iter()
        .map(|r| match r {
            Rule::Cond {
                field: F_MEDIA_KIND,
                action,
                value: Value::Num(v),
            } if v.from == v.to => Some((*action, v.from)),
            _ => None,
        })
        .collect::<Option<_>>()?;
    got.sort();
    [
        MediaTarget::Music,
        MediaTarget::Movies,
        MediaTarget::Tv,
        MediaTarget::Podcasts,
        MediaTarget::Audiobooks,
    ]
    .into_iter()
    .find(|m| {
        let mut want = m.conds().to_vec();
        want.sort();
        want == got
    })
    .map(|m| (m, inner.clone()))
}

// ============================================================
// 判定
// ============================================================

// ---- 項目 ----
pub const F_NAME: u32 = 0x02;
pub const F_ALBUM: u32 = 0x03;
pub const F_ARTIST: u32 = 0x04;
pub const F_BITRATE: u32 = 0x05;
pub const F_SAMPLE_RATE: u32 = 0x06;
pub const F_YEAR: u32 = 0x07;
pub const F_GENRE: u32 = 0x08;
pub const F_KIND: u32 = 0x09;
pub const F_DATE_MODIFIED: u32 = 0x0a;
pub const F_TRACK_NUMBER: u32 = 0x0b;
pub const F_SIZE: u32 = 0x0c;
pub const F_TIME: u32 = 0x0d;
pub const F_COMMENT: u32 = 0x0e;
pub const F_DATE_ADDED: u32 = 0x10;
pub const F_COMPOSER: u32 = 0x12;
pub const F_PLAY_COUNT: u32 = 0x16;
pub const F_LAST_PLAYED: u32 = 0x17;
pub const F_DISC_NUMBER: u32 = 0x18;
pub const F_RATING: u32 = 0x19;
pub const F_CHECKED: u32 = 0x1d;
pub const F_COMPILATION: u32 = 0x1f;
pub const F_BPM: u32 = 0x23;
pub const F_ARTWORK: u32 = 0x25;
pub const F_GROUPING: u32 = 0x27;
pub const F_PLAYLIST: u32 = 0x28;
pub const F_PURCHASED: u32 = 0x29;
pub const F_DESCRIPTION: u32 = 0x36;
pub const F_CATEGORY: u32 = 0x37;
pub const F_MEDIA_KIND: u32 = 0x3c;
pub const F_TV_SHOW: u32 = 0x3e;
pub const F_SEASON: u32 = 0x3f;
pub const F_SKIP_COUNT: u32 = 0x44;
pub const F_LAST_SKIPPED: u32 = 0x45;
pub const F_ALBUM_ARTIST: u32 = 0x47;
pub const F_SORT_NAME: u32 = 0x4e;
pub const F_SORT_ALBUM: u32 = 0x4f;
pub const F_SORT_ARTIST: u32 = 0x50;
pub const F_SORT_ALBUM_ARTIST: u32 = 0x51;
pub const F_SORT_COMPOSER: u32 = 0x52;
pub const F_SORT_TV_SHOW: u32 = 0x53;
pub const F_CONTENT_RATING: u32 = 0x59;
pub const F_ALBUM_RATING: u32 = 0x5a;
pub const F_LOCATION: u32 = 0x85;
pub const F_ICLOUD: u32 = 0x86;
pub const F_LOVE: u32 = 0x9a;
pub const F_ALBUM_LOVE: u32 = 0x9c;
pub const F_WORK: u32 = 0x9f;
pub const F_MOVEMENT_NAME: u32 = 0xa0;
pub const F_MOVEMENT_NUMBER: u32 = 0xa1;

/// iPod の DB に値が無い項目。判定では無し・0 として扱うので、iTunes の判定と結果が変わる
pub const FIELDS_NOT_ON_IPOD: &[u32] = &[
    F_PURCHASED,
    F_ALBUM_RATING,
    F_LOVE,
    F_ALBUM_LOVE,
    F_MOVEMENT_NUMBER,
    F_MOVEMENT_NAME,
    F_WORK,
    F_CONTENT_RATING,
    F_LOCATION,
    F_ICLOUD,
];

/// Rockbox の曲で ZenPod が持っていない情報 (`FIELDS_NOT_ON_IPOD` のほかに。画面の SPL_NOT_IN_ROCKBOX と同じ)
pub const FIELDS_NOT_IN_ROCKBOX: &[u32] = &[
    F_PLAY_COUNT,
    F_LAST_PLAYED,
    F_SKIP_COUNT,
    F_LAST_SKIPPED,
    F_DATE_MODIFIED,
    F_BPM,
    F_COMMENT,
    F_GROUPING,
    F_COMPILATION,
    F_SAMPLE_RATE,
    F_TV_SHOW,
    F_SEASON,
    F_DESCRIPTION,
    F_CATEGORY,
    F_SORT_TV_SHOW,
    F_SORT_COMPOSER,
];

// ---- 比較 (ACTION_STRING・ACTION_NOT を外した下位) ----
const OP_IS: u32 = 0x1;
const OP_CONTAINS: u32 = 0x2;
const OP_STARTS: u32 = 0x4;
const OP_ENDS: u32 = 0x8;
const OP_GREATER: u32 = 0x10;
const OP_LESS: u32 = 0x40;
const OP_RANGE: u32 = 0x100;
const OP_IN_LAST: u32 = 0x200;
const OP_BIT_AND: u32 = 0x400;

/// 判定に使う曲の値
pub trait SplTrack {
    fn spl_id(&self) -> u64;
    /// 文字の項目。持っていなければ None (空の文字として比べる)
    fn spl_text(&self, field: u32) -> Option<&str>;
    /// 文字以外の項目 (日付は 1904 年からの秒)。値が無い項目 (iCloud の状況・場所) は None で、
    /// どの「である」にも合わない
    fn spl_num(&self, field: u32) -> Option<i64>;
    /// チェックマークが付いているか
    fn spl_checked(&self) -> bool;
    fn spl_length_ms(&self) -> u64;
    fn spl_size(&self) -> u64;
}

/// 判定に使う今の時刻。iTunes は日付を PC の現地時刻のまま 1904 年からの秒にするので、それに合わせる
pub fn now_mac_local() -> i64 {
    let now = chrono::Local::now();
    crate::ipod::itunesdb::unix_to_mac(now.timestamp() + now.offset().local_minus_utc() as i64) as i64
}

pub struct EvalContext<'a> {
    /// 今の時刻 (1904 年からの秒。曲の日付と同じ基準)
    pub now_mac: i64,
    /// プレイリストの id → 入っている曲 (「プレイリストが…である」の条件用)
    pub playlists: &'a HashMap<u64, HashSet<u64>>,
}

impl Rules {
    pub fn matches<T: SplTrack>(&self, t: &T, cx: &EvalContext) -> bool {
        let mut it = self.items.iter().map(|r| r.matches(t, cx));
        if self.any {
            it.any(|m| m)
        } else {
            it.all(|m| m)
        }
    }

    /// `fields` のどれかを条件に使っているか
    pub fn uses_any_field(&self, fields: &[u32]) -> bool {
        self.items.iter().any(|r| match r {
            Rule::Group(g) => g.uses_any_field(fields),
            Rule::Cond { field, .. } => fields.contains(field),
        })
    }

    /// `fields` の項目の条件を外した条件 (iPod で判定できない条件を外して取り込むとき)。
    /// 外して空になったグループは消す (空の「いずれか」のグループはどの曲にも合わなくなるため)
    pub fn without_fields(&self, fields: &[u32]) -> Rules {
        let items: Vec<Rule> = self
            .items
            .iter()
            .filter_map(|r| match r {
                Rule::Group(g) => {
                    let g = g.without_fields(fields);
                    (!g.items.is_empty()).then_some(Rule::Group(g))
                }
                Rule::Cond { field, .. } if fields.contains(field) => None,
                c => Some(c.clone()),
            })
            .collect();
        // すべて外れたら「すべての曲」にする (空の「いずれか」はどの曲にも合わない)
        Rules { any: self.any && !items.is_empty(), items }
    }

    /// 条件に使っている `fields` の項目 (同じ項目は 1 回、出てきた順)
    pub fn fields_in(&self, fields: &[u32]) -> Vec<u32> {
        let mut out = Vec::new();
        self.collect_fields(fields, &mut out);
        out
    }

    fn collect_fields(&self, fields: &[u32], out: &mut Vec<u32>) {
        for r in &self.items {
            match r {
                Rule::Group(g) => g.collect_fields(fields, out),
                Rule::Cond { field, .. } if fields.contains(field) && !out.contains(field) => out.push(*field),
                _ => {}
            }
        }
    }

    /// 「プレイリストが…」の条件で指しているプレイリスト
    pub fn referenced_playlists(&self, out: &mut Vec<u64>) {
        for r in &self.items {
            match r {
                Rule::Group(g) => g.referenced_playlists(out),
                Rule::Cond {
                    field: F_PLAYLIST,
                    value: Value::Num(v),
                    ..
                } => out.push(v.from as u64),
                _ => {}
            }
        }
    }
}

impl Rule {
    fn matches<T: SplTrack>(&self, t: &T, cx: &EvalContext) -> bool {
        let (field, action, value) = match self {
            Rule::Group(g) => return g.matches(t, cx),
            Rule::Cond {
                field,
                action,
                value,
            } => (*field, *action, value),
        };
        let op = action & 0x00FF_FFFF;
        let hit = match value {
            Value::Str(s) => text_match(t.spl_text(field).unwrap_or(""), s, op),
            Value::Num(v) if field == F_PLAYLIST => cx
                .playlists
                .get(&(v.from as u64))
                .is_some_and(|m| m.contains(&t.spl_id())),
            Value::Num(v) => t
                .spl_num(field)
                .is_some_and(|x| num_match(x, v, op, cx.now_mac)),
        };
        hit != (action & ACTION_NOT != 0)
    }
}

/// 大文字・小文字は区別しない (iTunes と同じ)
fn text_match(have: &str, want: &str, op: u32) -> bool {
    let have = have.to_lowercase();
    let want = want.to_lowercase();
    match op {
        OP_IS => have == want,
        OP_CONTAINS => have.contains(&want),
        OP_STARTS => have.starts_with(&want),
        OP_ENDS => have.ends_with(&want),
        _ => false,
    }
}

fn num_match(x: i64, v: &NumValue, op: u32, now: i64) -> bool {
    match op {
        OP_IS => x == v.from,
        OP_GREATER => x > v.from,
        OP_LESS => x < v.from,
        OP_RANGE => (v.from.min(v.to)..=v.from.max(v.to)).contains(&x),
        // from_date = -N、from_units = 1 単位の秒
        OP_IN_LAST => x > now.saturating_add(v.from_date.saturating_mul(v.from_units)),
        OP_BIT_AND => x & v.from != 0,
        _ => false,
    }
}

/// 条件と上限で、入る曲を決める。`tracks` はライブラリの順。
/// `previous` は今入っている曲 (ランダムで選ぶときは、まだ条件に合う曲を先に残す。保存のたびに入れ替わらないように)
pub fn evaluate<T: SplTrack>(
    rules: &Rules,
    prefs: &SmartPrefs,
    tracks: &[T],
    cx: &EvalContext,
    previous: &[u64],
) -> Vec<u64> {
    let mut hit: Vec<&T> = tracks
        .iter()
        .filter(|t| !prefs.match_checked || t.spl_checked())
        .filter(|t| !prefs.check_rules || rules.matches(*t, cx))
        .collect();
    if !prefs.check_limits {
        return hit.iter().map(|t| t.spl_id()).collect();
    }
    sort_for_limit(&mut hit, prefs, previous);
    let cost = |t: &T| -> u64 {
        match prefs.limit_type {
            LIMIT_MINUTES | LIMIT_HOURS => t.spl_length_ms(),
            LIMIT_MB | LIMIT_GB => t.spl_size(),
            _ => 1,
        }
    };
    let v = prefs.limit_value as u64;
    let limit = match prefs.limit_type {
        LIMIT_MINUTES => v * 60_000,
        LIMIT_HOURS => v * 3_600_000,
        LIMIT_MB => v << 20,
        LIMIT_GB => v << 30,
        _ => v,
    };
    let mut total = 0u64;
    let mut out = Vec::new();
    for t in hit {
        total += cost(t);
        if total > limit {
            break;
        }
        out.push(t.spl_id());
    }
    out
}

fn sort_for_limit<T: SplTrack>(hit: &mut Vec<&T>, prefs: &SmartPrefs, previous: &[u64]) {
    use rand::seq::SliceRandom;
    use std::cmp::Reverse;
    let text = |t: &T, f| t.spl_text(f).unwrap_or("").to_lowercase();
    let num = |t: &T, f| t.spl_num(f).unwrap_or(0);
    match prefs.limit_sort {
        SORT_RANDOM => {
            let pos: HashMap<u64, usize> = previous.iter().enumerate().map(|(i, d)| (*d, i)).collect();
            let (mut kept, mut rest): (Vec<&T>, Vec<&T>) =
                hit.drain(..).partition(|t| pos.contains_key(&t.spl_id()));
            kept.sort_by_key(|t| pos[&t.spl_id()]);
            rest.shuffle(&mut rand::rng());
            kept.extend(rest);
            *hit = kept;
            return;
        }
        SORT_NAME => hit.sort_by_cached_key(|t| text(t, F_NAME)),
        SORT_ALBUM => hit.sort_by_cached_key(|t| text(t, F_ALBUM)),
        SORT_ARTIST => hit.sort_by_cached_key(|t| text(t, F_ARTIST)),
        SORT_GENRE => hit.sort_by_cached_key(|t| text(t, F_GENRE)),
        // 逆順でないときは新しい・多い・高い順
        SORT_ADDED => hit.sort_by_key(|t| Reverse(num(t, F_DATE_ADDED))),
        SORT_PLAY_COUNT => hit.sort_by_key(|t| Reverse(num(t, F_PLAY_COUNT))),
        SORT_LAST_PLAYED => hit.sort_by_key(|t| Reverse(num(t, F_LAST_PLAYED))),
        SORT_RATING => hit.sort_by_key(|t| Reverse(num(t, F_RATING))),
        _ => {}
    }
    if prefs.reverse_sort {
        hit.reverse();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// iTunes 12.13 (Windows) で作ったスマートプレイリストの Smart Info / Smart Criteria
    fn fixture() -> Vec<(String, Vec<u8>, Vec<u8>)> {
        let bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/itunes12-smart-playlists.plist"
        ));
        let v: plist::Value = plist::from_bytes(bytes).unwrap();
        v.as_dictionary().unwrap()["Playlists"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                let d = p.as_dictionary().unwrap();
                (
                    d["Name"].as_string().unwrap().to_string(),
                    d["Smart Info"].as_data().unwrap().to_vec(),
                    d["Smart Criteria"].as_data().unwrap().to_vec(),
                )
            })
            .collect()
    }

    fn get(name: &str) -> (Vec<u8>, Vec<u8>) {
        let (_, i, c) = fixture().into_iter().find(|(n, ..)| n == name).unwrap();
        (i, c)
    }

    #[test]
    fn criteria_round_trip() {
        let all = fixture();
        assert_eq!(all.len(), 17);
        for (name, _, crit) in all {
            let r = Rules::parse(&crit).unwrap();
            assert_eq!(r.to_bytes(), crit, "{name}");
        }
    }

    #[test]
    fn xml_info_round_trip() {
        for (name, info, _) in fixture() {
            let p = SmartPrefs::from_xml_info(&info).unwrap();
            assert_eq!(p.to_xml_info(Some(&info)), info, "{name}");
            assert_eq!(p.to_xml_info(None), info, "{name}");
        }
    }

    #[test]
    fn decode_kana_boon() {
        // 対象メディア「ミュージック」・購入した項目である・アーティストに「KANA-BOON」を含む
        let (info, crit) = get("KANA-BOONアルバム");
        let r = Rules::parse(&crit).unwrap();
        let num = |field, action, v| Rule::Cond {
            field,
            action,
            value: Value::Num(NumValue::single(v)),
        };
        assert_eq!(
            r,
            Rules {
                any: false,
                items: vec![
                    Rule::Group(Rules {
                        any: true,
                        items: vec![num(0x3c, 1, 1), num(0x3c, 1, 32)],
                    }),
                    Rule::Group(Rules {
                        any: false,
                        items: vec![
                            num(0x29, 0x0200_0001, 0),
                            Rule::Cond {
                                field: 0x04,
                                action: 0x0100_0002,
                                value: Value::Str("KANA-BOON".into()),
                            },
                        ],
                    }),
                ],
            }
        );
        let p = SmartPrefs::from_xml_info(&info).unwrap();
        assert!(p.live_update && p.check_limits);
        assert_eq!((p.limit_type, p.limit_sort, p.limit_value), (LIMIT_ITEMS, SORT_RANDOM, 25));
    }

    #[test]
    fn decode_limits() {
        // zp-lim-12: MB で 13、最も以前に追加した項目、ライブアップデートなし
        let p = SmartPrefs::from_xml_info(&get("zp-lim-12").0).unwrap();
        assert_eq!(
            p,
            SmartPrefs {
                live_update: false,
                check_rules: true,
                check_limits: true,
                limit_type: LIMIT_MB,
                limit_sort: SORT_ADDED,
                reverse_sort: true,
                limit_value: 13,
                match_checked: false,
            }
        );
        // zp-fields は 49 項目を 1 行ずつ
        let r = Rules::parse(&get("zp-fields").1).unwrap();
        assert_eq!(r.items.len(), 49);
    }

    #[test]
    fn mhod50_matches_itunes() {
        // iTunes が iPod に書いた種類別のプレイリストの MHOD 50
        let mut itunes = vec![0u8; MHOD50_LEN];
        itunes[..24].copy_from_slice(&[
            b'm', b'h', b'o', b'd', 0x18, 0, 0, 0, 0x60, 0, 0, 0, 0x32, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0,
        ]);
        itunes[24..40].copy_from_slice(&[1, 1, 0, 3, 2, 0, 0, 0, 0x19, 0, 0, 0, 0, 0, 0, 0]);
        itunes[40] = 5;
        let p = SmartPrefs::from_mhod(&itunes).unwrap();
        assert_eq!(p, SmartPrefs::default());
        assert_eq!(p.to_mhod(None), itunes);
        assert_eq!(p.to_mhod(Some(&itunes)), itunes);
    }

    #[test]
    fn mhod_and_xml_sort_values() {
        let p = SmartPrefs {
            check_limits: true,
            limit_sort: SORT_ALBUM,
            ..Default::default()
        };
        let m = p.to_mhod(None);
        assert_eq!(m[24 + 4], 0x04);
        assert_eq!(SmartPrefs::from_mhod(&m).unwrap(), p);
        let r = Rules::parse(&get("zp-ops").1).unwrap();
        assert_eq!(Rules::from_mhod(&r.to_mhod()).unwrap(), r);
    }

    #[test]
    fn media_target_split_and_wrap() {
        let want = [
            ("zp-lim-0", MediaTarget::Music),
            ("zp-lim-1", MediaTarget::Movies),
            ("zp-lim-2", MediaTarget::Tv),
            ("zp-lim-3", MediaTarget::Podcasts),
            ("zp-lim-4", MediaTarget::Audiobooks),
            ("zp-lim-5", MediaTarget::All),
            ("KANA-BOONアルバム", MediaTarget::Music),
            ("zp-fields", MediaTarget::All),
        ];
        for (name, media) in want {
            let (info, crit) = get(name);
            let prefs = SmartPrefs::from_xml_info(&info).unwrap();
            let rules = Rules::parse(&crit).unwrap();
            let def = SmartDef::from_parts(prefs.clone(), rules.clone());
            assert_eq!(def.media, media, "{name}");
            // 画面とのやり取り (JSON) を通しても、包み直すと iTunes が書いた形に戻る
            let json = serde_json::to_string(&def).unwrap();
            let back: SmartDef = serde_json::from_str(&json).unwrap();
            let (p2, r2) = back.into_parts();
            assert_eq!(p2, prefs, "{name}");
            assert_eq!(r2.to_bytes(), crit, "{name}");
        }
        // 2^53 を超える値は文字で渡す
        let r = Rules::parse(&get("zp-ops").1).unwrap();
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"3291618603768360366\""));
        assert_eq!(serde_json::from_str::<Rules>(&json).unwrap(), r);
    }

    #[test]
    fn drop_fields_not_on_ipod() {
        // KANA-BOONアルバム から「購入した項目」を外すと、対象メディアとアーティストの条件が残る
        let r = Rules::parse(&get("KANA-BOONアルバム").1).unwrap();
        assert_eq!(r.fields_in(FIELDS_NOT_ON_IPOD), [F_PURCHASED]);
        let d = r.without_fields(FIELDS_NOT_ON_IPOD);
        assert!(d.fields_in(FIELDS_NOT_ON_IPOD).is_empty());
        let Rule::Group(inner) = &d.items[1] else { panic!() };
        assert_eq!(inner.items.len(), 1);
        // 「いずれか」のグループの中が全部外れたら、グループごと消す
        let any = Rules {
            any: true,
            items: vec![Rule::Group(Rules {
                any: true,
                items: vec![Rule::Cond { field: F_LOVE, action: 1, value: Value::Num(NumValue::single(2)) }],
            })],
        };
        assert_eq!(any.without_fields(FIELDS_NOT_ON_IPOD), Rules { any: false, items: vec![] });
    }

    // ---------------- 判定 ----------------

    use crate::ipod::itunesdb::{ITunesDb, MHOD_ALBUM, MHOD_ARTIST, MHOD_TITLE, Playlist, Track, unix_to_mac};

    fn track(id: u64, title: &str, artist: &str) -> Track {
        let mut t = Track::new(id);
        t.set_string(MHOD_TITLE, Some(title));
        t.set_string(MHOD_ARTIST, Some(artist));
        t.set_string(MHOD_ALBUM, Some("Album"));
        t.set_length_ms(200_000);
        t.set_size(5 << 20);
        t
    }

    fn cond(field: u32, action: u32, value: Value) -> Rule {
        Rule::Cond {
            field,
            action,
            value,
        }
    }

    fn all(items: Vec<Rule>) -> Rules {
        Rules { any: false, items }
    }

    fn ids(rules: &Rules, prefs: &SmartPrefs, tracks: &[Track], previous: &[u64]) -> Vec<u64> {
        let pls = HashMap::new();
        let cx = EvalContext {
            now_mac: unix_to_mac(1_800_000_000) as i64,
            playlists: &pls,
        };
        evaluate(rules, prefs, tracks, &cx, previous)
    }

    #[test]
    fn text_conditions() {
        let ts = vec![
            track(1, "Sakura no Uta", "KANA-BOON"),
            track(2, "Clone", "kana-boon"),
            track(3, "Other", "Someone"),
        ];
        let p = SmartPrefs::default();
        let s = |a, v: &str| all(vec![cond(F_ARTIST, a, Value::Str(v.into()))]);
        // 大文字・小文字は区別しない
        assert_eq!(ids(&s(0x0100_0002, "kana"), &p, &ts, &[]), [1, 2]);
        assert_eq!(ids(&s(0x0300_0002, "kana"), &p, &ts, &[]), [3]);
        assert_eq!(ids(&s(0x0100_0001, "KANA-BOON"), &p, &ts, &[]), [1, 2]);
        assert_eq!(ids(&s(0x0100_0004, "some"), &p, &ts, &[]), [3]);
        assert_eq!(ids(&s(0x0100_0008, "boon"), &p, &ts, &[]), [1, 2]);
        // 持っていない文字の項目は空の文字
        let r = all(vec![cond(F_WORK, 0x0100_0001, Value::Str(String::new()))]);
        assert_eq!(ids(&r, &p, &ts, &[]), [1, 2, 3]);
    }

    #[test]
    fn itunes_rules_on_ipod_tracks() {
        // KANA-BOONアルバム: 対象メディア「ミュージック」・購入した項目である・アーティストに「KANA-BOON」を含む。
        // iPod の DB には購入の印が無いので、どの曲も合わない
        let (info, crit) = get("KANA-BOONアルバム");
        let rules = Rules::parse(&crit).unwrap();
        let mut prefs = SmartPrefs::from_xml_info(&info).unwrap();
        let ts = vec![track(1, "Sakura no Uta", "KANA-BOON")];
        assert!(ids(&rules, &prefs, &ts, &[]).is_empty());
        // 購入の条件を外すと合う。上限 25 曲もかかる
        let Rule::Group(g) = &rules.items[1] else {
            panic!()
        };
        let mut rules2 = rules.clone();
        rules2.items[1] = Rule::Group(all(g.items[1..].to_vec()));
        assert_eq!(ids(&rules2, &prefs, &ts, &[]), [1]);
        // 動画 (ミュージックビデオ以外) は対象メディアの包みで外れる
        let mut movie = track(2, "Movie", "KANA-BOON");
        movie.set_mediatype(2);
        prefs.check_limits = false;
        assert_eq!(ids(&rules2, &prefs, &[ts[0].clone(), movie], &[]), [1]);
    }

    #[test]
    fn numbers_dates_and_values() {
        let now = unix_to_mac(1_800_000_000) as i64;
        let mut a = track(1, "A", "X");
        a.set_playcount(5);
        a.set_rating(80);
        a.set_time_added_mac((now - 2 * 86400) as u32);
        let mut b = track(2, "B", "X");
        b.set_playcount(0);
        b.set_rating(100);
        b.set_time_added_mac((now - 30 * 86400) as u32);
        b.set_compilation(true);
        let ts = vec![a, b];
        let p = SmartPrefs::default();
        let n = |f, a, v| all(vec![cond(f, a, Value::Num(v))]);
        assert_eq!(ids(&n(F_PLAY_COUNT, 0x10, NumValue::single(0)), &p, &ts, &[]), [1]);
        assert_eq!(ids(&n(F_PLAY_COUNT, 0x0200_0001, NumValue::single(0)), &p, &ts, &[]), [1]);
        // 評価「である ★5」は iTunes では 100〜109 の範囲
        assert_eq!(ids(&n(F_RATING, 0x100, NumValue::range(100, 109)), &p, &ts, &[]), [2]);
        // 過去 7 日以内に追加 / 以内ではない
        let last7 = NumValue {
            from: 0x2DAD_ADAD_ADAD_ADAE,
            from_date: -7,
            from_units: 86400,
            to: 0x2DAD_ADAD_ADAD_ADAE,
            to_date: 0,
            to_units: 1,
        };
        assert_eq!(ids(&n(F_DATE_ADDED, 0x200, last7), &p, &ts, &[]), [1]);
        assert_eq!(ids(&n(F_DATE_ADDED, 0x0200_0200, last7), &p, &ts, &[]), [2]);
        assert_eq!(ids(&n(F_COMPILATION, 0x1, NumValue::single(1)), &p, &ts, &[]), [2]);
        // ラブ「なし」には合い、「ラブ済み」には合わない。iCloud の状況は値が無いのでどれにも合わない
        assert_eq!(ids(&n(F_LOVE, 0x1, NumValue::single(0)), &p, &ts, &[]), [1, 2]);
        assert!(ids(&n(F_LOVE, 0x1, NumValue::single(2)), &p, &ts, &[]).is_empty());
        assert!(ids(&n(F_ICLOUD, 0x1, NumValue::single(2)), &p, &ts, &[]).is_empty());
        assert_eq!(ids(&n(F_ICLOUD, 0x0200_0001, NumValue::single(2)), &p, &ts, &[]), [1, 2]);
        // 「いずれかに一致」
        let any = Rules {
            any: true,
            items: vec![
                cond(F_PLAY_COUNT, 0x10, Value::Num(NumValue::single(3))),
                cond(F_COMPILATION, 0x1, Value::Num(NumValue::single(1))),
            ],
        };
        assert_eq!(ids(&any, &p, &ts, &[]), [1, 2]);
    }

    #[test]
    fn limits() {
        let mut ts = Vec::new();
        for i in 1..=10u64 {
            let mut t = track(i, &format!("T{i}"), "X");
            t.set_rating((i as u8 % 6) * 20);
            t.set_playcount(i as u32);
            ts.push(t);
        }
        let any = all(vec![]);
        let lim = |limit_type, limit_value, limit_sort, reverse_sort| SmartPrefs {
            check_limits: true,
            limit_type,
            limit_value,
            limit_sort,
            reverse_sort,
            ..Default::default()
        };
        assert_eq!(ids(&any, &lim(LIMIT_ITEMS, 3, SORT_PLAY_COUNT, false), &ts, &[]), [10, 9, 8]);
        assert_eq!(ids(&any, &lim(LIMIT_ITEMS, 2, SORT_PLAY_COUNT, true), &ts, &[]), [1, 2]);
        // 評価が同じなら元の順
        assert_eq!(ids(&any, &lim(LIMIT_ITEMS, 3, SORT_RATING, false), &ts, &[]), [5, 4, 10]);
        // 1 曲 5 MB・200 秒
        assert_eq!(ids(&any, &lim(LIMIT_MB, 12, SORT_NAME, false), &ts, &[]), [1, 10]);
        assert_eq!(ids(&any, &lim(LIMIT_MINUTES, 10, SORT_NAME, false), &ts, &[]).len(), 3);
        // ランダムは今入っている曲を先に残す
        let r = ids(&any, &lim(LIMIT_ITEMS, 4, SORT_RANDOM, false), &ts, &[7, 3]);
        assert_eq!(r[..2], [7, 3]);
        assert_eq!(r.len(), 4);
        // チェックの無い曲を外す
        ts[0].hdr[0x78] = 1;
        let p = SmartPrefs {
            match_checked: true,
            ..Default::default()
        };
        assert_eq!(ids(&any, &p, &ts, &[]).len(), 9);
    }

    #[test]
    fn refresh_in_db() {
        let mut db = ITunesDb::new_empty("iPod");
        for (i, a) in [(1, "KANA-BOON"), (2, "Other"), (3, "KANA-BOON")] {
            db.tracks.push(track(i, "T", a));
        }
        db.master_mut().items = vec![1, 2, 3];
        let mut plain = Playlist::new("手作り", 100, false);
        plain.items = vec![2];
        db.playlists.push(plain);
        let mut kb = Playlist::new("KB", 200, false);
        kb.set_smart(
            &SmartPrefs::default(),
            &all(vec![cond(F_ARTIST, 0x0100_0002, Value::Str("kana".into()))]),
        );
        db.playlists.push(kb);
        // 「プレイリストが KB ではない」「手作りではない」(ほかのスマートプレイリストを指す)
        let mut rest = Playlist::new("残り", 300, false);
        rest.set_smart(
            &SmartPrefs::default(),
            &all(vec![
                cond(F_PLAYLIST, 0x0200_0001, Value::Num(NumValue::single(200))),
                cond(F_PLAYLIST, 0x0200_0001, Value::Num(NumValue::single(100))),
            ]),
        );
        db.playlists.push(rest);
        // ライブアップデートでないものは作り直さない
        let mut fixed = Playlist::new("固定", 400, false);
        fixed.set_smart(
            &SmartPrefs {
                live_update: false,
                ..Default::default()
            },
            &all(vec![]),
        );
        db.playlists.push(fixed);
        // iTunes が同期した KANA-BOONアルバム (購入した項目を使う) は iTunes が入れた曲のまま
        let (info, crit) = get("KANA-BOONアルバム");
        let mut synced = Playlist::new("KANA-BOONアルバム", 500, false);
        synced.set_smart(
            &SmartPrefs::from_xml_info(&info).unwrap(),
            &Rules::parse(&crit).unwrap(),
        );
        synced.items = vec![3, 1];
        db.playlists.push(synced);

        assert!(db.refresh_smart_playlists(0));
        assert_eq!(db.playlists[2].items, [1, 3]);
        assert!(db.playlists[3].items.is_empty());
        assert!(db.playlists[4].items.is_empty());
        assert_eq!(db.playlists[5].items, [3, 1]);
        db.tracks.push(track(4, "T", "Third"));
        assert!(db.refresh_smart_playlists(0));
        assert_eq!(db.playlists[3].items, [4]);
        assert!(!db.refresh_smart_playlists(0));
        // 書いて読んでも条件が残る
        let back = ITunesDb::parse(&db.serialize()).unwrap();
        let (p, r) = back.playlists[3].smart().unwrap();
        assert_eq!(p, SmartPrefs::default());
        assert_eq!(r.items.len(), 2);
        assert_eq!(back.playlists[3].items, [4]);
    }
}
