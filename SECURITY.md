# セキュリティ / Security

## 脆弱性の報告

ZenPod の脆弱性を見つけたときは、公開の Issue には書かず、GitHub の非公開の報告で知らせてください。

1. このリポジトリの [Security](https://github.com/ishiichan-dayo/ZenPod/security) を開く
2. 「Report a vulnerability」から報告する

報告は開発者だけが読めます。再現の手順・影響する版・OS を書いていただけると助かります。

個人で開発しているので、返信や修正にかかる時間はお約束できませんが、確かめて修正したら、先行版または通常の版で配ります。

### 対象

- 最新の版だけを修正します。古い版を使っている場合は、まず最新の版で起きるか確かめてください
- 特に知りたいもの: 自動アップデート・iPod や PC のファイルの読み書き (iPod の外や意図しない場所を触るものなど)・外との通信・アプリの中で開くリンク
- ZenPod が使っているライブラリそのものの脆弱性は、そのライブラリの開発元にも報告してください

## Reporting a vulnerability

If you find a security issue in ZenPod, please do not open a public issue. Report it privately through GitHub instead:

1. Open the [Security](https://github.com/ishiichan-dayo/ZenPod/security) tab of this repository
2. Click "Report a vulnerability"

Only the developer can see your report. Steps to reproduce, the affected version and your OS help a lot.

ZenPod is developed by one person, so I can't promise how quickly I'll reply or fix it, but once a fix is confirmed it will be shipped as an early fix or a regular release.

### Scope

- Only the latest version receives fixes. If you're on an older version, please check whether the issue still happens on the latest one
- Especially of interest: automatic updates, reading and writing files on the iPod or the PC (e.g. touching files outside the iPod or in unintended places), network communication, and links opened from the app
- For vulnerabilities in the libraries ZenPod uses, please also report them to those projects
