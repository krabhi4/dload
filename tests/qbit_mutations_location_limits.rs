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
    d.status = DownloadStatus::Completed;
    d
}

#[tokio::test]
async fn set_location_updates_save_path_within_download_dir() {
    let manager = make_manager();
    let mut dl = torrent_download("aabbcc");
    dl.save_path = "/downloads/old-name".to_string();
    manager.add_download(dl).await;

    // set_location to a path inside download_dir
    manager.set_location("aabbcc", "/downloads/new-location").await;

    let all = manager.get_all().await;
    let updated = all.iter().find(|d| d.id == "aabbcc").unwrap();
    assert_eq!(updated.save_path, "/downloads/new-location");
}

#[tokio::test]
async fn set_location_rejects_path_outside_download_dir() {
    let manager = make_manager();
    let mut dl = torrent_download("aabbcc");
    dl.save_path = "/downloads/original".to_string();
    manager.add_download(dl).await;

    // Attempt to move outside download_dir — should be ignored
    manager.set_location("aabbcc", "/etc/passwd").await;

    let all = manager.get_all().await;
    let d = all.iter().find(|d| d.id == "aabbcc").unwrap();
    assert_eq!(d.save_path, "/downloads/original", "path outside download_dir must not be accepted");
}

#[tokio::test]
async fn set_download_limit_persists() {
    let manager = make_manager();
    manager.add_download(torrent_download("abc123")).await;

    manager.set_download_limit("abc123", 1_000_000).await;

    let all = manager.get_all().await;
    let d = all.iter().find(|d| d.id == "abc123").unwrap();
    assert_eq!(d.dl_limit, 1_000_000);
}

#[tokio::test]
async fn set_upload_limit_persists() {
    let manager = make_manager();
    manager.add_download(torrent_download("abc123")).await;

    manager.set_upload_limit("abc123", 500_000).await;

    let all = manager.get_all().await;
    let d = all.iter().find(|d| d.id == "abc123").unwrap();
    assert_eq!(d.up_limit, 500_000);
}
