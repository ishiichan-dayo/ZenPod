//! OS とウィンドウまわり: リンクを既定のブラウザで開く・起動の見張り・WebView2 が落ちたときの立て直し
//!
//! ほかのソフト (RivaTuner Statistics Server など) が WebView2 に入り込んで落とすと、
//! ウィンドウが真っ黒のまま何も表示されない。起動後しばらく画面側から応答が無ければ、
//! 標準のダイアログ (WebView2 が落ちていても出せる) で起動し直すか確認する。使用中には何もしない

use std::sync::atomic::Ordering;

use ipod_core::t;
use tauri::{AppHandle, Manager};

use crate::state::AppState;

/// 既定のブラウザで開く (開いてよいかは呼ぶ側で確かめる)
pub fn open_in_browser(url: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.spawn().map(|_| ())
}

/// ファイルを、それが入ったフォルダを開いて選んだ状態で見せる (エクスプローラー / Finder / ファイルマネージャー)。
/// 見せてよいかは呼ぶ側で確かめる
pub fn reveal_in_folder(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // explorer は引数を自分で解釈する。全体を引用符でくくると /select が効かないので、場所だけをくくる
        let mut arg = std::ffi::OsString::from("/select,\"");
        arg.push(path);
        arg.push("\"");
        std::process::Command::new("explorer.exe").raw_arg(arg).spawn().map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg("-R").arg(path).spawn().map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // ファイルを選んで開けるのは FileManager1 に対応したファイルマネージャー。無ければフォルダだけ開く
        let shown = std::process::Command::new("dbus-send")
            .args(["--session", "--print-reply", "--dest=org.freedesktop.FileManager1", "--type=method_call"])
            .args(["/org/freedesktop/FileManager1", "org.freedesktop.FileManager1.ShowItems"])
            .arg(format!("array:string:{}", file_uri(path)))
            .arg("string:")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if shown {
            return Ok(());
        }
        std::process::Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn().map(|_| ())
    }
}

/// file:// の URI。dbus-send は配列の要素を「,」で区切るので、英数字と -._~/ 以外はすべて %XX にする
#[cfg(all(unix, not(target_os = "macos")))]
fn file_uri(path: &std::path::Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut s = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// 画面が起動する前 (起動時の確認ダイアログなど) の言語: 設定で選んだ言語、無ければ OS の言語
pub fn initial_lang(app: &AppHandle) {
    let pref = app.state::<AppState>().zen.prefs.get("lang");
    let os = sys_locale::get_locale().unwrap_or_default();
    ipod_core::i18n::set_lang(&zenpod_app::domain::lang::initial_lang(pref, &os));
}

pub fn watch_startup(app: &AppHandle) {
    let app = app.clone();
    let ready = app.state::<AppState>().ui_ready.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(15));
        if ready.load(Ordering::SeqCst) {
            return;
        }
        use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
        let restart = app
            .dialog()
            .message(t!(
                "画面を表示できませんでした。\n\n画面に情報を重ねて表示するソフト (RivaTuner Statistics Server など) が原因のことがあります。\n起動し直しますか？"
            ))
            .title("ZenPod")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(t!("起動し直す"), t!("このまま待つ")))
            .blocking_show();
        // ダイアログを出している間に表示できた場合は起動し直さない
        if restart && !ready.load(Ordering::SeqCst) {
            app.restart();
        }
    });
}

/// 起動したあとで WebView2 が落ちたときに立て直す (Windows のみ)。
/// RTSS (RivaTuner Statistics Server) が WebView2 に入り込むと、起動から数秒後に WebView2 の本体が
/// 落ちて、ウィンドウが真っ白・真っ黒のまま残ることがある (ui_ready の後なので watch_startup では拾えない)
#[cfg(windows)]
pub fn watch_webview_crash(app: &AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let app = app.clone();
    let _ = win.with_webview(move |wv| unsafe {
        use webview2_com::Microsoft::Web::WebView2::Win32::*;
        use webview2_com::ProcessFailedEventHandler;
        let Ok(core) = wv.controller().CoreWebView2() else {
            return;
        };
        let handler = ProcessFailedEventHandler::create(Box::new(move |sender, args| {
            let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
            if let Some(args) = args {
                args.ProcessFailedKind(&mut kind)?;
            }
            if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED {
                // WebView2 ごと無くなったので、アプリを起動し直すしかない
                restart_after_crash(&app);
            } else if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
                || kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE
            {
                // 画面の中身だけが落ちた場合は読み込み直せば戻る
                if let Some(s) = sender {
                    let _ = s.Reload();
                }
            }
            Ok(())
        }));
        let mut token = 0i64;
        let _ = core.add_ProcessFailed(&handler, &mut token);
    });
}

/// 画面が落ちたあとに起動し直す。転送中なら終わるまで待つ (転送と保存は画面が無くても続いている)。
/// 起動し直してもすぐ落ちる場合に繰り返さないよう、自動で起動し直すのは 2 回まで。それ以降は確認する
#[cfg(windows)]
fn restart_after_crash(app: &AppHandle) {
    const COUNT: &str = "ZENPOD_CRASH_RESTARTS";
    let app = app.clone();
    std::thread::spawn(move || {
        let session = app.state::<AppState>().zen.session.clone();
        while session.is_busy() {
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        let n: u32 = std::env::var(COUNT).ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        if n >= 2 {
            use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
            let restart = app
                .dialog()
                .message(t!(
                    "画面が何度も表示できなくなっています。\n\n画面に情報を重ねて表示するソフト (RivaTuner Statistics Server など) が原因のことがあります。\nそのソフトの設定で ZenPod.exe と msedgewebview2.exe を対象外にしてください。\n\n起動し直しますか？"
                ))
                .title("ZenPod")
                .kind(MessageDialogKind::Warning)
                .buttons(MessageDialogButtons::OkCancelCustom(t!("起動し直す"), t!("終了する")))
                .blocking_show();
            if !restart {
                app.exit(0);
                return;
            }
        }
        // 起動し直したアプリは環境変数を引き継ぐので、回数をそこに持たせる
        std::env::set_var(COUNT, (n + 1).to_string());
        app.restart();
    });
}
