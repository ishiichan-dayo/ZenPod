# 貢献のしかた / Contributing

ZenPod に関心を持っていただきありがとうございます。

## 不具合の報告・要望

[Issues](https://github.com/ishiichan-dayo/ZenPod/issues) に書き込んでください。iPod の機種 (ウィンドウの左上に出ます)・モード (iPod OS / Rockbox)・OS を書いていただけると助かります。

セキュリティの問題は Issue に書かず、[SECURITY.md](SECURITY.md) の方法で知らせてください。

## プルリクエスト

- 不具合の修正は歓迎します。機能の追加や大きな変更は、先に Issue で相談してください (方針に合わず取り込めないことがあります)
- ビルドの仕方・テスト・コードの構成は [DEVELOPMENT.md](DEVELOPMENT.md) にあります
- PR では、テスト・clippy (警告もエラー)・画面のビルド・ライセンスの一覧の確認が自動で動きます。初めての方の PR は、メンテナーが承認してから動きます
- 依存ライブラリを足したり上げたりしたときは、`python3 scripts/third-party-licenses.py` で `THIRD_PARTY_LICENSES.md` を作り直してください
- 画面の文言を足したときは、`src/locales/en.json` に英語も足してください
- コードのコメントは日本語ですが、PR やコメントは英語でもかまいません
- 送っていただいたコードは、このリポジトリと同じ GPL-3.0-or-later で公開されます

---

## Bug reports and requests

Please use [Issues](https://github.com/ishiichan-dayo/ZenPod/issues). Including your iPod model (shown at the top left of the window), the mode (iPod OS / Rockbox) and your OS helps a lot.

For security issues, please don't open an issue. Follow [SECURITY.md](SECURITY.md) instead.

## Pull requests

- Bug fixes are welcome. For new features or larger changes, please open an issue to discuss first (some changes may not fit the direction of the app)
- How to build, test, and how the code is organized: [DEVELOPMENT.md](DEVELOPMENT.md) (in Japanese)
- Tests, clippy (warnings are errors), the frontend build and the license list are checked automatically on pull requests. For first-time contributors, the checks run after a maintainer approves them
- If you add or update dependencies, regenerate `THIRD_PARTY_LICENSES.md` with `python3 scripts/third-party-licenses.py`
- If you add UI text, please add the English text to `src/locales/en.json`
- Code comments are in Japanese, but pull requests and comments in English are welcome
- Contributions are published under the same GPL-3.0-or-later license as this repository
