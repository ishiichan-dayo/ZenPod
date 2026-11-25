#!/usr/bin/env python3
"""配布物に入る依存ライブラリのライセンスを THIRD_PARTY_LICENSES.md にまとめる。

使い方 (リポジトリの一番上で): python3 scripts/third-party-licenses.py
依存を足したり上げたりしたら作り直してコミットする。

- Rust: `cargo tree -p zenpod` の通常の依存 (全 OS 分。ビルドだけに使うもの・proc-macro は除く)
- npm: package.json の dependencies (画面に入るもの) とその依存
- 各パッケージのフォルダにある LICENSE / COPYING / NOTICE などを、同じ中身ごとにまとめて載せる
"""

import json
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "THIRD_PARTY_LICENSES.md"
STANDARD_TEXT_DIR = ROOT / "scripts" / "license-texts"
FILE_RE = re.compile(r"^(LICEN[CS]E|COPYING|NOTICE|COPYRIGHT|UNLICENSE)([-_.].*)?$", re.I)


def run(*args):
    return subprocess.run(args, cwd=ROOT, check=True, capture_output=True, text=True).stdout


def license_files(folder: Path):
    files = []
    for p in sorted(folder.iterdir()):
        if p.is_file() and FILE_RE.match(p.name):
            files.append(p)
    # ライセンスを子のフォルダに置くもの (LICENSES/ など)
    for sub in ("LICENSES", "licenses", "license"):
        d = folder / sub
        if d.is_dir():
            files += sorted(x for x in d.iterdir() if x.is_file())
    return files


def rust_packages():
    tree = run(
        "cargo", "tree", "-p", "zenpod", "-e", "normal,no-proc-macro", "--target", "all",
        "--prefix", "none", "--format", "{p}",
    )
    wanted = set()
    for line in tree.splitlines():
        m = re.match(r"^(\S+) v(\S+)", line)
        if m:
            wanted.add((m.group(1), m.group(2)))
    meta = json.loads(run("cargo", "metadata", "--format-version", "1"))
    pkgs = []
    for p in meta["packages"]:
        if p["source"] is None:  # このリポジトリのもの
            continue
        if (p["name"], p["version"]) not in wanted:
            continue
        pkgs.append({
            "name": p["name"],
            "version": p["version"],
            "license": p.get("license") or "",
            "url": p.get("repository") or "",
            "dir": Path(p["manifest_path"]).parent,
        })
    return pkgs


def npm_packages():
    lock = json.loads((ROOT / "package-lock.json").read_text())["packages"]
    pkgs = []
    for key, v in lock.items():
        if not key or v.get("dev") or v.get("devOptional"):
            continue
        name = key.split("node_modules/")[-1]
        url = v.get("resolved", "")
        pkgs.append({
            "name": name,
            "version": v.get("version", ""),
            "license": v.get("license", ""),
            "url": f"https://www.npmjs.com/package/{name}" if url else "",
            "dir": ROOT / key,
        })
    return pkgs


# 「A OR B」で選べるときに使う順 (短く、著作権の表記がそのまま載るもの)
PREFER = ["MIT", "Zlib", "BSD-3-Clause", "BSD-2-Clause", "ISC", "0BSD", "Unlicense", "CC0-1.0",
          "MIT-0", "BSL-1.0", "Unicode-3.0", "Apache-2.0", "MPL-2.0"]
# 全文を最後に 1 回だけ載せる長い定型文 (中に作者ごとの表記が無いもの)
STANDARD = {
    "Apache-2.0": "TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION",
    "MPL-2.0": "Mozilla Public License Version 2.0",
}


def choose(expr: str):
    """SPDX の式から使うライセンスを 1 つ選ぶ (OR のときだけ。AND は全部)"""
    e = expr.replace("/", " OR ").replace("(", " ").replace(")", " ")
    if " AND " in e:
        return None
    opts = [o.strip() for o in e.split(" OR ") if o.strip()]
    for want in PREFER:
        if want in opts:
            return want
    return None


def pick_files(files, chosen):
    """選んだライセンスのファイルと NOTICE だけにする (名前で分かるとき)"""
    if not chosen:
        return files
    key = {"Apache-2.0": "APACHE", "BSL-1.0": "BSL", "Unicode-3.0": "UNICODE"}.get(chosen, chosen.split("-")[0]).upper()
    named = [f for f in files if key in f.name.upper()]
    notice = [f for f in files if f.name.upper().startswith("NOTICE")]
    if named:
        return named + [f for f in notice if f not in named]
    # 名前でライセンスが分からないもの (LICENSE 1 つなど) はそのまま
    tagged = [f for f in files if any(k in f.name.upper() for k in ("MIT", "APACHE", "ZLIB", "BSD", "LGPL", "GPL", "MPL", "UNLICENSE", "BSL"))]
    rest = [f for f in files if f not in tagged]
    return rest or files


def norm(t: str) -> str:
    return "\n".join(l.rstrip() for l in t.strip().splitlines())


# このリポジトリの中に移植したコード (依存のパッケージではないもの)。ソースの中のライセンスの表記を載せる
PORTED = [
    {
        "name": "libgpod (itdb_hash58.c)",
        "license": "BSD-3-Clause",
        "url": "https://sourceforge.net/projects/gtkpod/",
        "file": "crates/ipod-core/src/ipod/hash58.rs",
        "start": "Copyright (C) 2007 Christophe Fergeau",
        "end": "OF SUCH DAMAGE.",
    },
    {
        "name": "dstaley/hashab",
        "license": "Unlicense",
        "url": "https://github.com/dstaley/hashab",
        "file": "crates/ipod-core/src/ipod/hashab.rs",
        "start": "This is free and unencumbered software",
        "end": "<https://unlicense.org>",
    },
    {
        "name": "RHash (edonr.c, in dstaley/hashab)",
        "license": "0BSD",
        "url": "https://github.com/rhash/RHash",
        "file": "crates/ipod-core/src/ipod/hashab.rs",
        "start": "Copyright (c) 2011, Aleksey Kravchenko",
        "end": "PERFORMANCE OF THIS SOFTWARE.",
    },
    {
        # LGPL-2.1-or-later のコードを GPL-3.0-or-later (ZenPod の LICENSE) として使う。文は LICENSE と同じなので載せない
        "name": "libgpod (itdb_hash72.c, itdb_zlib.c, itdb_sqlite.c, iTunesSD in itdb_itunesdb.c)",
        "license": "LGPL-2.1-or-later, used under GPL-3.0-or-later",
        "url": "https://sourceforge.net/projects/gtkpod/",
        "file": "crates/ipod-core/src/ipod/{hash72,cdb,sqlitedb,shuffle}.rs",
    },
]


def ported_notice(item):
    lines = (ROOT / item["file"]).read_text().splitlines()
    start = next(i for i, l in enumerate(lines) if item["start"] in l)
    end = next(i for i, l in enumerate(lines) if i >= start and item["end"] in l)
    text = [l.lstrip("/").removeprefix(" ").rstrip() for l in lines[start:end + 1]]
    return "\n".join(text).strip()


def main():
    pkgs = rust_packages() + npm_packages()
    groups = defaultdict(list)  # ライセンス文の組 -> パッケージ
    missing = []
    used_standard = set()
    for p in sorted(pkgs, key=lambda p: (p["name"].lower(), p["version"])):
        files = license_files(p["dir"]) if p["dir"].is_dir() else []
        chosen = choose(p["license"])
        files = pick_files(files, chosen)
        if not files:
            missing.append(p)
            continue
        texts = []
        for f in files:
            t = norm(f.read_text(errors="replace"))
            std = next((k for k, mark in STANDARD.items() if mark in t), None)
            if std:
                used_standard.add(std)
                texts.append(f"[{std}: full text at the end of this file]")
            else:
                texts.append(t)
        if chosen and chosen != p["license"]:
            p["license"] = f"{p['license']}; used under {chosen}"
        groups[tuple(texts)].append(p)

    out = [
        "# Third-party licenses",
        "",
        "ZenPod includes the following third-party software. Each is used under the license shown.",
        "ZenPod 本体のライセンスは GPL-3.0-or-later です (LICENSE)。",
        "",
        "This file is generated by `scripts/third-party-licenses.py`.",
        "",
    ]
    out += [
        "---",
        "",
        "- **Linux AppImage**: the AppImage also bundles libraries from Ubuntu (GTK, WebKitGTK, GStreamer, the FFmpeg libraries and others,",
        "  under licenses such as LGPL-2.1+, GPL-2+ and BSD). The list of those packages with their versions, licenses and copyright files,",
        "  and how to get their source code, is attached to each release as `ZenPod_<version>_linux-libraries.txt`",
        "  (https://github.com/ishiichan-dayo/ZenPod/releases). The Windows and macOS versions and the .deb package don't bundle them.",
        "  Linux の AppImage には Ubuntu のライブラリも入っています。一覧・ライセンス・ソースコードの入手先は、各リリースに添付の `ZenPod_<版>_linux-libraries.txt` にあります。",
        "",
    ]
    for item in PORTED:
        out.append("---")
        out.append("")
        out.append(f"- **{item['name']}** ({item['license']}) — {item['url']} (ported to `{item['file']}`)")
        out.append("")
        if "start" in item:
            out.append("```text")
            out.append(ported_notice(item))
            out.append("```")
            out.append("")
    for texts, ps in sorted(groups.items(), key=lambda kv: kv[1][0]["name"].lower()):
        out.append("---")
        out.append("")
        for p in ps:
            line = f"- **{p['name']}** {p['version']}"
            if p["license"]:
                line += f" ({p['license']})"
            if p["url"]:
                line += f" — {p['url']}"
            out.append(line)
        out.append("")
        for t in texts:
            out.append("```text")
            out.append(t.replace("```", "'''"))
            out.append("```")
            out.append("")
    if missing:
        out.append("---")
        out.append("")
        out.append("The following packages do not ship a license file; they are used under the license they declare.")
        out.append("")
        for p in missing:
            out.append(f"- **{p['name']}** {p['version']} ({p['license'] or 'unknown'})"
                       + (f" — {p['url']}" if p["url"] else ""))
        out.append("")
    for std in sorted(used_standard):
        text = STANDARD_TEXT_DIR / f"{std}.txt"
        out.append("---")
        out.append("")
        out.append(f"## {std}")
        out.append("")
        out.append("```text")
        out.append(norm(text.read_text()))
        out.append("```")
        out.append("")
    OUT.write_text("\n".join(out))
    print(f"{OUT.name}: {len(pkgs)} packages, {len(groups)} license texts, {len(missing)} without files",
          file=sys.stderr)
    for p in missing:
        print(f"  no license file: {p['name']} {p['version']} ({p['license']})", file=sys.stderr)


if __name__ == "__main__":
    main()
