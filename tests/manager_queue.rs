use std::sync::Arc;
use dload::db::{self, repository::Repository};
use dload::domain::{Download, DownloadStatus, Settings};
use dload::manager::ManagerState;

fn make_manager(max_concurrent: u32) -> Arc<ManagerState> {
    let db = Arc::new(db::Database::new(":memory:").unwrap());
    let repo = Arc::new(Repository::new(db));
    let mut settings = Settings::default();
    settings.max_concurrent = max_concurrent;
    repo.save_settings(&settings).unwrap();
    Arc::new(ManagerState::new(settings, repo))
}

fn queued_download(id: &str, save_dir: &str) -> Download {
    let mut d = Download::new(format!("http://example.com/{}", id), save_dir);
    d.id = id.to_string();
    d.status = DownloadStatus::Queued;
    d
}

#[tokio::test]
async fn does_not_start_more_than_max_concurrent() {
    let manager = make_manager(1);

    let d1 = queued_download("dl-1", "/tmp");
    let d2 = queued_download("dl-2", "/tmp");
    manager.add_download(d1).await;
    manager.add_download(d2).await;

    manager.schedule_queued().await;

    let all = manager.get_all().await;
    let downloading: Vec<_> = all.iter().filter(|d| d.status == DownloadStatus::Downloading).collect();
    let queued: Vec<_> = all.iter().filter(|d| d.status == DownloadStatus::Queued).collect();

    assert_eq!(downloading.len(), 1, "exactly one should be Downloading");
    assert_eq!(queued.len(), 1, "exactly one should remain Queued");
}

#[tokio::test]
async fn starts_next_queued_when_active_download_finishes() {
    let manager = make_manager(1);

    let mut d1 = queued_download("dl-1", "/tmp");
    d1.status = DownloadStatus::Completed;
    let d2 = queued_download("dl-2", "/tmp");

    manager.add_download(d1).await;
    manager.add_download(d2).await;

    manager.schedule_queued().await;

    let all = manager.get_all().await;
    let downloading: Vec<_> = all.iter().filter(|d| d.status == DownloadStatus::Downloading).collect();

    assert_eq!(downloading.len(), 1, "second download should be promoted to Downloading");
    assert_eq!(downloading[0].id, "dl-2");
}

#[tokio::test]
async fn allows_max_concurrent_two() {
    let manager = make_manager(2);

    let d1 = queued_download("dl-1", "/tmp");
    let d2 = queued_download("dl-2", "/tmp");
    let d3 = queued_download("dl-3", "/tmp");
    manager.add_download(d1).await;
    manager.add_download(d2).await;
    manager.add_download(d3).await;

    manager.schedule_queued().await;

    let all = manager.get_all().await;
    let downloading: Vec<_> = all.iter().filter(|d| d.status == DownloadStatus::Downloading).collect();
    let queued: Vec<_> = all.iter().filter(|d| d.status == DownloadStatus::Queued).collect();

    assert_eq!(downloading.len(), 2, "two should be Downloading with max_concurrent=2");
    assert_eq!(queued.len(), 1, "one should remain Queued");
}
