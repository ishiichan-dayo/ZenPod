# 開発

> Code comments and this document are written in Japanese. Issues and pull requests in English are welcome.

ZenPod は Tauri 2 + Rust + TypeScript (Vite、フレームワークなし) で作っています。

## 必要なもの

- Rust (stable)
- Node.js 20 以上
- 各 OS の Tauri の前提のライブラリ (<https://tauri.app/start/prerequisites/>)
- ffmpeg (なくても動きます。変換のテストは ffmpeg がないと確かめずに通ります)

## ビルドと起動

```sh
npm install
npm run tauri dev      # 開発版を起動
npm run tauri build    # インストーラを作る
npm run build          # 画面だけ (tsc + vite build。翻訳の抜けも確かめる)
```

## テスト

```sh
cargo test -p ipod-core    # コア (iTunesDB・アートワーク・Rockbox・タグなど。libgpod との互換テストを含む)
cargo test -p zenpod-app   # ユースケース (偽物のポートと一時フォルダの空の iPod で動かす)
cargo clippy --workspace --all-targets
```

検証用の CLI もあります: `cargo run --example ipodtool -- dump|add|rm|artpng|repair-art|verify-hash ...`

### WSL から Windows で確かめる

WSL (Ubuntu) で Windows 版をクロスビルドし、Windows 側で起動できます。インストールした ZenPod とは別のアプリ「ZenPod Dev」として動くので、設定は混ざりません。

```sh
npm run win:setup      # 初回だけ: Windows 向けのビルドの道具を用意する (sudo は不要)
npm run win:dev        # 開発版 (画面は WSL の Vite から読む。src/ の変更は保存ですぐ反映)
npm run win:run        # リリース版で起動
npm run win:installer  # インストーラを作ってダウンロードフォルダに置く
npm run win:stop       # 開発版を終了する
```

## 構成

```
crates/ipod-core/        iPod の中身を読み書きするコア (画面に依存しない)
  src/domain/            曲・プレイリスト・形式などの型と決まりごと、日本語の読み (ソート用) の生成
  src/library.rs         iPod OS と Rockbox を同じ操作で扱う窓口
  src/ipod/              iPod OS: iTunesDB (itunesdb.rs)・hash58 署名 (hash58.rs)・アートワーク (artwork.rs)・動画と写真
  src/rockbox/           Rockbox: 曲のファイルとデータベース (tagcache)
  src/device.rs          iPod の検出・機種と FireWire GUID の取得・取り出し
  src/media/             タグの読み書き・ジャケット・変換 (ffmpeg)
  src/online/            ジャケットと曲の情報の検索 (iTunes / Deezer / MusicBrainz)・ポッドキャスト
  examples/ipodtool.rs   検証用の CLI
crates/zenpod-app/       ユースケース (画面と Tauri に依存しない。外の世界にはポートの trait 越しに触る)
src-tauri/               Tauri の部分 (コマンドの入口と組み立て)
src/                     画面 (TypeScript)。依存は ui → app → infra / domain の一方向
```

`ipod-core` は libgpod 0.8.3 と相互に確かめています。libgpod が書いたデータベースを読めること、ZenPod が書いたデータベースを libgpod が読めること、hash58 の署名が libgpod と一致することを確認しています (`crates/ipod-core/tests/libgpod_compat.rs`)。アートワークも、ZenPod が書いた ArtworkDB / .ithmb を libgpod が読んで正しい画像を返すこと、libgpod が書いたアートワークを ZenPod が読めることを確認しています。

## ffmpeg を同梱する

自分でビルドするアプリに ffmpeg を入れたいときは、`src-tauri/bin/ffmpeg(.exe)` に置き、`src-tauri/tauri.conf.json` の `bundle.resources` に `"bin/*": "bin/"` を足してください (リソースの中の `bin/` を自動で探します)。ffmpeg のライセンス (LGPL / GPL) に注意してください。

## ライセンスの一覧

配布物に入る依存ライブラリのライセンスは `THIRD_PARTY_LICENSES.md` にまとめています。依存を足したり上げたりしたら、作り直してコミットしてください (リリースのビルドで、最新でなければ止まります)。

```sh
python3 scripts/third-party-licenses.py
```

## 不具合の報告・変更の提案

- 不具合と要望は [Issues](https://github.com/ishiichan-dayo/ZenPod/issues) へ。iPod の機種・モード (iPod OS / Rockbox)・OS を書いていただけると助かります
- 大きな変更は、先に Issue で相談してください
- 送っていただいたコードは、このリポジトリと同じ GPL-3.0-or-later で公開されます

## リリース (メンテナー向け)

1. `src-tauri/tauri.conf.json`・`src-tauri/Cargo.toml`・`package.json` (と `package-lock.json`・`Cargo.lock`) の版を上げてコミットし、main に push する
2. 注釈付きタグを main の先頭に付けて push する (メッセージがリリースノートとアプリの更新案内になる)
3. `gh workflow run release --ref main -f publish=true` (先行版は `-f early=true` も)

署名の鍵 (`TAURI_SIGNING_PRIVATE_KEY`・`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) は、main だけから使える environment「release」に置いています。ビルドのジョブには渡さず、署名のジョブだけが使います (`.github/workflows/build.yml` の冒頭)。
