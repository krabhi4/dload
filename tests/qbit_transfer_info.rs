use std::sync::Arc;
use dload::db::{self, repository::Repository};
use dload::domain::{Download, DownloadStatus, Settings};
use dload::manager::ManagerState;

#[tokio::test]
async fn transfer_info_reflects_active_download_rates() {
    let db = Arc::new(db::Database::new(":memory:").unwrap());
    let repo = Arc::new(Repository::new(db));
    let settings = Settings::default();
    repo.save_settings(&settings).unwrap();
    let manager = Arc::new(ManagerState::new(settings, repo));

    let mut dl = Download::new("http://example.com/file.iso".to_string(), "/tmp");
    dl.status = DownloadStatus::Downloading;
    dl.speed = 5_000_000;
    dl.upload_speed = 1_000_000;
    dl.downloaded_size = 100_000_000;
    manager.add_download(dl).await;

    let snap = manager.transfer_snapshot().await;

    assert_eq!(snap.dl_info_speed, 5_000_000);
    assert_eq!(snap.up_info_speed, 1_000_000);
    assert_eq!(snap.dl_info_data, 100_000_000);
}

#[tokio::test]
async fn transfer_info_zero_when_no_active_downloads() {
    let db = Arc::new(db::Database::new(":memory:").unwrap());
    let repo = Arc::new(Repository::new(db));
    let settings = Settings::default();
    repo.save_settings(&settings).unwrap();
    let manager = Arc::new(ManagerState::new(settings, repo));

    let snap = manager.transfer_snapshot().await;

    assert_eq!(snap.dl_info_speed, 0);
    assert_eq!(snap.dl_info_data, 0);
    assert_eq!(snap.up_info_speed, 0);
    assert_eq!(snap.up_info_data, 0);
}
