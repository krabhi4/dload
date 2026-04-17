//! Slot-lock concurrency test: verifies that under N concurrent
//! add_and_maybe_start claims, at most `max_concurrent` downloads ever get the
//! Downloading slot. Uses `claim_or_queue_for_test` so we don't invoke real
//! network workers.

use dload::db::{repository::Repository, Database};
use dload::domain::{Download, DownloadStatus, Settings};
use dload::manager::ManagerState;
use std::sync::Arc;

fn build_state(max_concurrent: u32) -> Arc<ManagerState> {
    let db = Arc::new(Database::new(":memory:").unwrap());
    let repo = Arc::new(Repository::new(db));
    let settings = Settings {
        max_concurrent,
        download_dir: "/tmp".into(),
        ..Settings::default()
    };
    let (state, _) = ManagerState::new(settings, repo);
    Arc::new(state)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slot_lock_grants_exactly_max_concurrent_on_burst() {
    let state = build_state(3);

    let mut handles = Vec::new();
    for i in 0..50 {
        let s = state.clone();
        handles.push(tokio::spawn(async move {
            let dl = Download::new(format!("https://example.com/{i}.iso"), "/tmp");
            s.claim_or_queue_for_test(dl).await
        }));
    }
    let results: Vec<DownloadStatus> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    let downloading = results
        .iter()
        .filter(|s| **s == DownloadStatus::Downloading)
        .count();
    let queued = results
        .iter()
        .filter(|s| **s == DownloadStatus::Queued)
        .count();

    assert_eq!(
        downloading, 3,
        "expected exactly max_concurrent=3 to be granted Downloading, got {downloading}"
    );
    assert_eq!(queued, 47);

    // Map state agrees with claim outcomes
    let map = state.downloads.read().await;
    assert_eq!(map.len(), 50);
    let map_downloading = map
        .values()
        .filter(|d| d.status == DownloadStatus::Downloading)
        .count();
    assert_eq!(map_downloading, 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slot_lock_allows_single_claim_when_max_is_one() {
    let state = build_state(1);

    let mut handles = Vec::new();
    for i in 0..20 {
        let s = state.clone();
        handles.push(tokio::spawn(async move {
            let dl = Download::new(format!("https://example.com/{i}.iso"), "/tmp");
            s.claim_or_queue_for_test(dl).await
        }));
    }
    let results: Vec<_> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();
    let downloading = results
        .iter()
        .filter(|s| **s == DownloadStatus::Downloading)
        .count();
    assert_eq!(downloading, 1);
}

#[tokio::test]
async fn positions_increase_monotonically_for_serial_adds() {
    let state = build_state(100); // high cap so every call gets Downloading
    for i in 0..5 {
        let dl = Download::new(format!("https://example.com/{i}.iso"), "/tmp");
        state.claim_or_queue_for_test(dl).await;
    }
    let map = state.downloads.read().await;
    let mut positions: Vec<i32> = map.values().map(|d| d.position).collect();
    positions.sort();
    for (i, p) in positions.iter().enumerate() {
        assert_eq!(*p, i as i32, "positions should be 0..n");
    }
}
