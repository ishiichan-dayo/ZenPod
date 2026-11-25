//! iTunesDB / iTunesCDB と Locations.itdb.cbk の hash72 署名 (iPod nano 5G)。
//!
//! 署名は 46 バイト: `01 00` + 乱数 12 バイト + AES-128-CBC で暗号化した (SHA1 20 バイト + 乱数 12 バイト)。
//! 鍵は全機種で同じで、機種ごとに違うのは CBC の IV だけ。IV を作る方法は公開されていないが、正しい署名が 1 つあれば
//! 平文の先頭 16 バイトが分かるので IV を逆算できる。そのため iTunes (Finder) で一度同期した iPod の iTunesCDB から
//! IV を取り出し、libgpod と同じ `iPod_Control/Device/HashInfo` (54 バイト) に覚えておく。
//!
//! libgpod 0.8.3 の itdb_hash72.c を移植した (hash_generate / hash_extract は Chris Lee 氏のコードで WTFPL、
//! ほかは LGPL-2.1-or-later)。

use aes::Aes128;
use aes::cipher::{BlockCipherDecrypt, BlockCipherEncrypt, KeyInit};
use sha1::{Digest, Sha1};

const AES_KEY: [u8; 16] = [0x61, 0x8c, 0xa1, 0x0d, 0xc7, 0xf5, 0x7f, 0xd3, 0xb4, 0x72, 0x3e, 0x08, 0x15, 0x74, 0x63, 0xd7];

/// mhbd の hash72 の位置
pub const HASH72_AT: usize = 0x72;
pub const HASH72_LEN: usize = 46;
/// HashInfo ファイルの大きさ
pub const HASH_INFO_LEN: usize = 54;

/// 機種ごとの署名の材料 (`iPod_Control/Device/HashInfo`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashInfo {
    /// FireWire GUID (8 バイト) を 20 バイトに 0 で埋めたもの
    pub uuid: [u8; 20],
    pub rnd: [u8; 12],
    pub iv: [u8; 16],
}

impl HashInfo {
    pub fn new(fwid: &[u8; 8], rnd: [u8; 12], iv: [u8; 16]) -> Self {
        let mut uuid = [0u8; 20];
        uuid[..8].copy_from_slice(fwid);
        HashInfo { uuid, rnd, iv }
    }

    /// HashInfo ファイルを読む。`fwid` の iPod のものでなければ None
    pub fn parse(data: &[u8], fwid: &[u8; 8]) -> Option<Self> {
        if data.len() != HASH_INFO_LEN || &data[..6] != b"HASHv0" {
            return None;
        }
        let info = HashInfo {
            uuid: data[6..26].try_into().ok()?,
            rnd: data[26..38].try_into().ok()?,
            iv: data[38..54].try_into().ok()?,
        };
        (info.uuid[..8] == fwid[..] && info.uuid[8..].iter().all(|&b| b == 0)).then_some(info)
    }

    pub fn to_bytes(&self) -> [u8; HASH_INFO_LEN] {
        let mut out = [0u8; HASH_INFO_LEN];
        out[..6].copy_from_slice(b"HASHv0");
        out[6..26].copy_from_slice(&self.uuid);
        out[26..38].copy_from_slice(&self.rnd);
        out[38..54].copy_from_slice(&self.iv);
        out
    }

    /// `sha1` の署名 (46 バイト)
    pub fn sign(&self, sha1: &[u8; 20]) -> [u8; HASH72_LEN] {
        let mut plain = [0u8; 32];
        plain[..20].copy_from_slice(sha1);
        plain[20..].copy_from_slice(&self.rnd);
        let cipher = Aes128::new(&AES_KEY.into());
        // CBC: 1 つ目は IV、2 つ目は 1 つ目の暗号文と混ぜてから暗号化する
        let mut prev = self.iv;
        let mut out = [0u8; HASH72_LEN];
        out[0] = 0x01;
        out[2..14].copy_from_slice(&self.rnd);
        for (i, chunk) in plain.chunks(16).enumerate() {
            let mut block = [0u8; 16];
            for (b, (p, v)) in block.iter_mut().zip(chunk.iter().zip(prev)) {
                *b = p ^ v;
            }
            let mut b = block.into();
            cipher.encrypt_block(&mut b);
            prev = b.into();
            out[14 + 16 * i..30 + 16 * i].copy_from_slice(&prev);
        }
        out
    }

    /// 正しい署名とその SHA1 から、署名の材料 (IV と乱数) を取り出す。署名が SHA1 と合わなければ None
    pub fn extract(signature: &[u8], sha1: &[u8; 20], fwid: &[u8; 8]) -> Option<Self> {
        if signature.len() != HASH72_LEN || signature[..2] != [0x01, 0x00] {
            return None;
        }
        let rnd: [u8; 12] = signature[2..14].try_into().ok()?;
        let mut plain = [0u8; 32];
        plain[..20].copy_from_slice(sha1);
        plain[20..].copy_from_slice(&rnd);
        let cipher = Aes128::new(&AES_KEY.into());
        let c1: [u8; 16] = signature[14..30].try_into().ok()?;
        let c2: [u8; 16] = signature[30..46].try_into().ok()?;
        // 2 つ目のブロックは IV に関係なく確かめられる (D(C2) ^ C1 = 平文の後半)。libgpod はここを確かめていない
        let mut d2 = c2.into();
        cipher.decrypt_block(&mut d2);
        let d2: [u8; 16] = d2.into();
        if d2.iter().zip(c1).map(|(a, b)| a ^ b).ne(plain[16..].iter().copied()) {
            return None;
        }
        let mut d1 = c1.into();
        cipher.decrypt_block(&mut d1);
        let d1: [u8; 16] = d1.into();
        let mut iv = [0u8; 16];
        for (v, (a, b)) in iv.iter_mut().zip(d1.iter().zip(&plain[..16])) {
            *v = a ^ b;
        }
        Some(HashInfo::new(fwid, rnd, iv))
    }
}

/// hash72 をかける SHA1: db_id (0x18)・hash58 (0x58)・hash72 (0x72) を 0 にして全体を。
/// unk_0x32 と hashing_scheme (0x30) はそのまま含める
pub fn db_sha1(db: &[u8]) -> [u8; 20] {
    let mut h = Sha1::new();
    h.update(&db[..0x18]);
    h.update([0u8; 8]);
    h.update(&db[0x20..0x58]);
    h.update([0u8; 20]);
    h.update(&db[0x6C..HASH72_AT]);
    h.update([0u8; HASH72_LEN]);
    h.update(&db[HASH72_AT + HASH72_LEN..]);
    h.finalize().into()
}

/// 書き出す iTunesDB / iTunesCDB に hash72 をかける。`scheme` は mhbd の 0x30 に書く値
/// (nano 5G は hash58 と両方かけるので 1。hash72 だけなら 2)
pub fn write_hash72(db: &mut [u8], info: &HashInfo, scheme: u16) {
    assert!(db.len() >= HASH72_AT + HASH72_LEN && &db[0..4] == b"mhbd");
    db[0x30..0x32].copy_from_slice(&scheme.to_le_bytes());
    let sig = info.sign(&db_sha1(db));
    db[HASH72_AT..HASH72_AT + HASH72_LEN].copy_from_slice(&sig);
}

/// iTunes が署名した iTunesDB / iTunesCDB から署名の材料を取り出す (縮めたままのバイト列で)
pub fn extract_from_db(db: &[u8], fwid: &[u8; 8]) -> Option<HashInfo> {
    if db.len() < HASH72_AT + HASH72_LEN || &db[0..4] != b"mhbd" {
        return None;
    }
    HashInfo::extract(&db[HASH72_AT..HASH72_AT + HASH72_LEN], &db_sha1(db), fwid)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FWID: [u8; 8] = [0x00, 0x0A, 0x27, 0x00, 0x1A, 0x2B, 0x3C, 0x4D];

    fn info() -> HashInfo {
        HashInfo::new(&FWID, *b"0123456789ab", [7u8; 16])
    }

    #[test]
    fn sign_and_extract_round_trip() {
        let sha1 = [0x5Au8; 20];
        let sig = info().sign(&sha1);
        assert_eq!(&sig[..14], b"\x01\x000123456789ab");
        assert_eq!(HashInfo::extract(&sig, &sha1, &FWID), Some(info()));
        // SHA1 が違えば取り出さない
        assert_eq!(HashInfo::extract(&sig, &[0u8; 20], &FWID), None);
    }

    #[test]
    fn matches_aes_cbc_reference() {
        // AES-128-CBC (鍵 618ca10d…、IV 0) で 0 の 32 バイトを暗号化した値 (OpenSSL で計算)
        let zero = HashInfo::new(&FWID, [0; 12], [0; 16]);
        let sig = zero.sign(&[0; 20]);
        let hex: String = sig[14..].iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, REFERENCE);
    }
    const REFERENCE: &str = include_str!("hash72_reference.txt");

    #[test]
    fn hash_info_file() {
        let bytes = info().to_bytes();
        assert_eq!(bytes.len(), 54);
        assert_eq!(HashInfo::parse(&bytes, &FWID), Some(info()));
        // ほかの iPod のものは使わない
        assert_eq!(HashInfo::parse(&bytes, &[1; 8]), None);
        assert_eq!(HashInfo::parse(&bytes[..53], &FWID), None);
    }

    #[test]
    fn signs_database_and_recovers_material() {
        let mut db = crate::ipod::itunesdb::ITunesDb::new_empty("nano").serialize();
        db[0x18] = 0x99;
        write_hash72(&mut db, &info(), 1);
        assert_eq!(&db[0x30..0x32], &[1, 0]);
        assert_eq!(db[0x18], 0x99, "db_id はそのまま");
        assert_eq!(extract_from_db(&db, &FWID), Some(info()));
        // db_id と hash58 は署名に含めない
        db[0x18] = 0x11;
        db[0x58] = 0x22;
        assert_eq!(extract_from_db(&db, &FWID), Some(info()));
        // それ以外が変われば合わない
        db[0x100] ^= 1;
        assert_eq!(extract_from_db(&db, &FWID), None);
    }
}
