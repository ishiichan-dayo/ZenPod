//! 開発・検証用 CLI
//!   ipodtool list                         接続中の iPod を列挙
//!   ipodtool dump <root>                  曲とプレイリストを表示
//!   ipodtool details <file>               ファイルの形式・タグの中身を JSON で表示
//!   ipodtool add <root> <file>...         曲を追加してプレイリスト「追加分」を作成
//!   ipodtool rm <root> <index>...         曲を削除
//!   ipodtool verify-hash <iTunesDB> <fwid> hash58 を再計算して照合
//!   ipodtool rbdb <root>                  Rockbox のデータベースを作って書く
//!   ipodtool rbdump <dir>                 Rockbox のデータベースを JSON 行で表示 (1 行 1 曲)
//!   ipodtool spl <iTunesDB>               スマートプレイリストの今の中身と、条件から判定した中身を比べる
//!   ipodtool xml <XML>                    iTunes / ミュージックの XML の取り込めるプレイリストを表示

use std::path::{Path, PathBuf};

use ipod_core::ipod::hash58;
use ipod_core::media::convert;
use ipod_core::{Ipod, device};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    match args.first().map(String::as_str) {
        Some("list") => {
            for r in device::find_ipod_roots() {
                println!("{:#?}", device::probe(&r, None));
            }
        }
        Some("dump") => dump(&Ipod::open(Path::new(&args[1]))?),
        Some("details") => {
            let d = ipod_core::media::details::file_details(Path::new(&args[1]))?;
            println!("{}", serde_json::to_string_pretty(&d)?);
            let s = ipod_core::media::details::audio_summary(Path::new(&args[1]))?;
            println!("summary: {}", serde_json::to_string(&s)?);
        }
        Some("structure") => println!("{}", ipod_core::media::structure::tag_structure(Path::new(&args[1]))?),
        Some("add") => {
            let mut ipod = Ipod::open(Path::new(&args[1]))?;
            let ff = convert::find_ffmpeg(None, &[]);
            let mut added = Vec::new();
            for f in &args[2..] {
                match ipod.add_file(
                    &PathBuf::from(f),
                    ff.as_deref(),
                    &ipod_core::domain::track::AddOptions {
                        // NO_YOMI=1 で読みの自動生成を止める (確認画面の検証用)
                        auto_reading: std::env::var_os("NO_YOMI").is_none(),
                        // REPLACE=1 で重複を「上書きする」(設定の「同じ曲が iPod にあるとき」と同じ)
                        replace_duplicates: std::env::var_os("REPLACE").is_some(),
                        ..Default::default()
                    },
                ) {
                    Ok(o) => {
                        println!("{f}: {o:?}");
                        if let ipod_core::domain::track::AddOutcome::Added { track, .. } = o {
                            added.push(track.id.parse::<u64>()?);
                        }
                    }
                    Err(e) => println!("{f}: ERROR {e}"),
                }
            }
            if !added.is_empty() {
                ipod.create_playlist("追加分", &added)?;
            }
            ipod.save()?;
            dump(&ipod);
        }
        Some("rm") => {
            let mut ipod = Ipod::open(Path::new(&args[1]))?;
            let tracks = ipod.tracks();
            let ids: Vec<u64> = args[2..]
                .iter()
                .map(|i| tracks[i.parse::<usize>().unwrap()].id.parse().unwrap())
                .collect();
            println!("removed {}", ipod.remove_tracks(&ids)?);
            ipod.save()?;
            dump(&ipod);
        }
        Some("repair-art") => {
            // 欠けている・大きさの違うジャケットのサムネイルを作り直す (アプリが iPod を開いたときと同じ)
            let mut ipod = Ipod::open(Path::new(&args[1]))?;
            let n = ipod.repair_artwork();
            println!("repaired {n}");
            if n > 0 {
                ipod.save()?;
            }
        }
        Some("artpng") => {
            let ipod = Ipod::open(Path::new(&args[1]))?;
            let t = &ipod.tracks()[args[2].parse::<usize>()?];
            let png = ipod.artwork_png(t.artwork_id, true).ok_or("no artwork")?;
            std::fs::write(&args[3], png)?;
            println!("artwork id={} -> {}", t.artwork_id, args[3]);
        }
        Some("verify-hash") => {
            let data = std::fs::read(&args[1])?;
            let fwid = hash58::parse_firewire_id(&args[2]).ok_or("bad fwid")?;
            let mut copy = data.clone();
            hash58::write_hash58(&mut copy, &fwid);
            println!(
                "scheme={} stored={} computed={}",
                u16::from_le_bytes([data[0x30], data[0x31]]),
                hex(&data[0x58..0x6C]),
                hex(&copy[0x58..0x6C])
            );
            println!("{}", if data == copy { "MATCH" } else { "MISMATCH" });
        }
        Some("rbdb") => {
            let root = Path::new(&args[1]);
            // 2 つ目に曲のパス ("/Music/...") を 1 行ずつ並べたファイルを渡すと、その順で入れる (Rockbox の出力との突き合わせ用)
            let b = match args.get(2) {
                Some(order) => {
                    let order: Vec<String> = std::fs::read_to_string(order)?.lines().map(String::from).collect();
                    let (mut files, skipped) = ipod_core::rockbox::rbdb::scan_files(root);
                    files.sort_by_key(|f| order.iter().position(|o| *o == ipod_core::rockbox::rbdb::rel_path(root, &f.path)).unwrap_or(usize::MAX));
                    ipod_core::rockbox::rbdb::write(root, &files, None, skipped)?
                }
                None => ipod_core::rockbox::rbdb::rebuild(root, None)?,
            };
            println!("{}", serde_json::to_string(&b)?);
        }
        Some("spl") => spl(Path::new(&args[1]))?,
        Some("xml") => {
            let lib = ipod_core::pc::itunesxml::XmlLibrary::parse(&std::fs::read(&args[1])?)?;
            println!("曲 {} / プレイリスト {}", lib.tracks.len(), lib.playlists.len());
            for p in &lib.playlists {
                println!("  {:016X} {} 曲 {} {}", p.pid, p.items.len(), if p.smart.is_some() { "スマート" } else { "" }, p.name);
            }
        }
        Some("rbdump") => {
            let db = ipod_core::rockbox::rbdb::read(Path::new(&args[1])).ok_or("no database")?;
            eprintln!("serial={} commitid={} dirty={}", db.serial, db.commitid, db.dirty);
            for e in &db.entries {
                let mut strings: Vec<_> = e.strings.iter().collect();
                strings.sort();
                println!("{}", serde_json::json!({"flag": e.flag, "seek": e.seek.to_vec(), "strings": strings}));
            }
        }
        _ => eprintln!("usage: ipodtool list|dump|details|add|rm|verify-hash|rbdb|rbdump ..."),
    }
    Ok(())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn dump(ipod: &Ipod) {
    println!(
        "name={} model={} checksum={:?} fwid={:?}",
        ipod.name(),
        ipod.info.model_name,
        ipod.info.checksum,
        ipod.info.firewire_id
    );
    for (i, t) in ipod.tracks().iter().enumerate() {
        println!(
            "  [{i}] {} / {} / {} ({} ms, {} kbps, {}) sort=[{} | {} | {}] art={}",
            t.title,
            t.artist,
            t.album,
            t.duration_ms,
            t.bitrate,
            t.filetype,
            t.sort_title,
            t.sort_artist,
            t.sort_album,
            t.artwork_id
        );
    }
    for p in ipod.playlists() {
        println!(
            "  PL '{}' master={} smart={} n={}",
            p.name,
            p.master,
            p.smart,
            p.track_ids.len()
        );
    }
}

fn spl(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use ipod_core::ipod::itunesdb::{ITunesDb, now_unix, unix_to_mac};
    use ipod_core::smartpl::{EvalContext, evaluate};
    use std::collections::{HashMap, HashSet};
    let db = ITunesDb::parse(&std::fs::read(path)?)?;
    let members: HashMap<u64, HashSet<u64>> = db
        .playlists
        .iter()
        .map(|p| (p.id(), p.items.iter().copied().collect()))
        .collect();
    let cx = EvalContext {
        now_mac: unix_to_mac(now_unix()) as i64,
        playlists: &members,
    };
    for p in &db.playlists {
        if !p.is_smart() {
            continue;
        }
        let Some((prefs, rules)) = p.smart() else {
            println!("{}: 条件を読めない", p.name);
            continue;
        };
        let got = evaluate(&rules, &prefs, &db.tracks, &cx, &p.items);
        let have: HashSet<u64> = p.items.iter().copied().collect();
        let now: HashSet<u64> = got.iter().copied().collect();
        println!(
            "{}: 今 {} 曲 / 判定 {} 曲 / 判定だけ {} / 今だけ {} / 順番も同じ {}",
            p.name,
            have.len(),
            now.len(),
            now.difference(&have).count(),
            have.difference(&now).count(),
            got == p.items
        );
        println!("  {prefs:?}\n  {rules:?}");
    }
    Ok(())
}
