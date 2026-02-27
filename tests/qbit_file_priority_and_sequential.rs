use std::sync::Arc;
use dload::db::{self, repository::Repository};
use dload::domain::{Download, DownloadStatus, Settings};
use dload::manager::ManagerState;

fn make_manager() -> Arc<ManagerState> {
    let db = Arc::new(db::Database::new(":memory:").unwrap());
    let repo = Arc::new(Repository::new(db));
    let settings = Settings::default();
    repo.save_settings(&settings).unwrap();
    Arc::new(ManagerState::new(settings, repo))
}

fn torrent_download(id: &str) -> Download {
    let mut d = Download::new(format!("magnet:?xt=urn:btih:{}", id), "/downloads");
    d.id = id.to_string();
    d.status = DownloadStatus::Downloading;
    d
}

#[tokio::test]
async fn toggle_sequential_download_flips_flag() {
    let manager = make_manager();
    manager.add_download(torrent_download("abc")).await;

    // Initially false
    let all = manager.get_all().await;
    let d = all.iter().find(|d| d.id == "abc").unwrap();
    assert!(!d.sequential_download);

    // Toggle on
    manager.toggle_sequential_download("abc").await;
    let all = manager.get_all().await;
    let d = all.iter().find(|d| d.id == "abc").unwrap();
    assert!(d.sequential_download);

    // Toggle off
    manager.toggle_sequential_download("abc").await;
    let all = manager.get_all().await;
    let d = all.iter().find(|d| d.id == "abc").unwrap();
    assert!(!d.sequential_download);
}

#[tokio::test]
async fn toggle_first_last_piece_prio_flips_flag() {
    let manager = make_manager();
    manager.add_download(torrent_download("abc")).await;

    manager.toggle_first_last_piece_prio("abc").await;

    let all = manager.get_all().await;
    let d = all.iter().find(|d| d.id == "abc").unwrap();
    assert!(d.first_last_piece_prio);
}

#[tokio::test]
async fn file_prio_updates_stored_map() {
    let manager = make_manager();
    manager.add_download(torrent_download("abc")).await;

    // Set file index 0 to priority 6 (high), index 1 to priority 0 (skip)
    manager.set_file_priority("abc", &[(0, 6), (1, 0)]).await;

    let all = manager.get_all().await;
    let d = all.iter().find(|d| d.id == "abc").unwrap();
    let prios: serde_json::Value = serde_json::from_str(
        d.file_priorities_json.as_deref().unwrap_or("{}")
    ).unwrap();
    assert_eq!(prios["0"], 6);
    assert_eq!(prios["1"], 0);
}
