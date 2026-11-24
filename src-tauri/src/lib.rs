//! ZenPod の Tauri の部分: 組み立てと、画面とのやり取り。
//!
//! - ユースケースは zenpod-app (Tauri に依存しない)。ここはその入口と、Tauri が要るポートの実装だけ
//! - `commands`: 画面から呼ぶコマンド (薄いアダプタ)
//! - `host`: 画面へのイベントとアプリのフォルダ (zenpod-app のポートの実装)
//! - `audio_protocol`: 再生用の `ipodaudio://`
//! - `platform`: リンクを開く・起動の見張り・WebView2 が落ちたときの立て直し
//! - `logging`: ログ (zenpod.log)

mod audio_protocol;
mod commands;
mod host;
mod logging;
mod platform;
mod state;

use commands::{artwork, devices, library, pc, podcasts, rockbox, settings, transfer, update, visual};
use tauri::Manager;

use crate::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    logging::init();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(AppState::new())
        .manage(update::EarlyUpdate::default())
        .register_asynchronous_uri_scheme_protocol("ipodaudio", |ctx, req, responder| {
            // ファイルの読み込みや変換に時間がかかるので別スレッドで返す
            let app = ctx.app_handle().clone();
            std::thread::spawn(move || responder.respond(audio_protocol::serve(&app, &req)));
        })
        .invoke_handler(tauri::generate_handler![
            devices::scan_devices,
            devices::usb_ipod_connected,
            devices::drive_wait_reason,
            rockbox::rebuild_rockbox_db,
            transfer::count_conversions,
            rockbox::rename_rockbox_files,
            rockbox::set_rockbox_layout,
            rockbox::preview_rockbox_layout,
            rockbox::guess_rockbox_layout,
            settings::open_url,
            update::check_early_update,
            update::install_early_update,
            devices::pick_device_folder,
            devices::open_device,
            devices::close_device,
            devices::snapshot,
            devices::device_present,
            devices::eject_device,
            settings::get_settings,
            settings::pick_ffmpeg,
            settings::reset_ffmpeg_path,
            devices::set_firewire_id,
            transfer::add_paths,
            transfer::cancel_transfer,
            transfer::export_tracks,
            library::remove_tracks,
            library::create_playlist,
            library::rename_playlist,
            library::delete_playlist,
            library::add_to_playlist,
            library::remove_from_playlist,
            library::reorder_playlist,
            library::reorder_playlists,
            library::smart_playlist,
            library::save_smart_playlist,
            library::preview_smart_playlist,
            library::playlist_import_preview,
            library::playlist_import,
            library::export_smart_xml,
            library::export_m3u8,
            devices::rename_ipod,
            artwork::get_artwork,
            artwork::set_artwork,
            artwork::remove_artwork,
            artwork::shrink_artwork,
            artwork::embedded_artwork,
            library::generate_readings,
            library::set_readings,
            library::preview_readings,
            library::apply_readings,
            artwork::search_artwork,
            artwork::probe_artwork,
            artwork::apply_artwork_url,
            artwork::fetch_image,
            library::artist_variants,
            library::lookup_artist_aliases,
            library::cancel_alias_lookup,
            library::unify_artists,
            library::edit_tags,
            library::search_releases,
            library::preview_release,
            library::apply_tag_edits,
            pc::add_pc_folder,
            pc::remove_pc_folder,
            pc::reorder_pc_folders,
            pc::scan_music_folder,
            settings::install_ffmpeg,
            settings::cancel_ffmpeg_install,
            settings::uninstall_ffmpeg,
            settings::load_prefs,
            settings::set_pref,
            pc::edit_pc_tags,
            pc::set_pc_artwork,
            pc::apply_pc_artwork_url,
            pc::pc_embedded_artwork,
            pc::shrink_pc_artwork,
            pc::remove_pc_artwork,
            pc::apply_pc_tag_edits,
            pc::preview_release_pc,
            pc::rename_pc_file,
            pc::rename_pc_files,
            pc::pc_artwork,
            library::track_details,
            library::reveal_track,
            library::track_lyrics,
            library::track_options,
            settings::set_lang,
            library::audio_summaries,
            library::make_readings,
            pc::pc_album_art,
            podcasts::podcast_list,
            podcasts::podcast_subscribe,
            podcasts::podcast_unsubscribe,
            podcasts::podcast_refresh,
            podcasts::podcast_set_keep,
            podcasts::podcast_mark,
            podcasts::podcast_delete_files,
            podcasts::podcast_files_size,
            podcasts::podcast_cover,
            podcasts::podcast_image,
            podcasts::podcast_open_link,
            podcasts::podcast_download,
            podcasts::cancel_podcast_download,
            settings::ui_ready,
            visual::list_videos,
            visual::edit_videos,
            visual::video_file_info,
            visual::remove_videos,
            visual::video_thumb,
            visual::video_play_plan,
            visual::video_stream_start,
            visual::video_stream_read,
            visual::video_stream_stop,
            visual::export_videos,
            visual::photo_library,
            visual::photo_thumb,
            visual::remove_photos,
            visual::create_photo_album,
            visual::rename_photo_album,
            visual::delete_photo_album,
            visual::add_to_photo_album,
            visual::remove_from_photo_album,
            visual::export_photos,
            visual::save_pasted_image,
            settings::ui_log,
        ])
        .setup(|app| {
            if let Ok(dir) = app.path().app_log_dir() {
                logging::attach_file(&dir);
            }
            let state = app.state::<AppState>();
            state.host.bind(app.handle().clone());
            platform::initial_lang(app.handle());
            platform::watch_startup(app.handle());
            state.zen.devices.watch_usb();
            let thumbs = state.thumbs.clone();
            std::thread::spawn(move || thumbs.prune());
            #[cfg(windows)]
            platform::watch_webview_crash(app.handle());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
