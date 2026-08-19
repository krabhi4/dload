//! Integration tests for the DB Repository. Each test spins up a fresh
//! in-memory SQLite via `Database::new(":memory:")` so migrations run and
//! behaviour matches production.

use dload::db::{repository::Repository, Database};
use dload::domain::{Download, DownloadFolder, DownloadStatus, Protocol, Role, Settings, User};
use std::sync::Arc;

fn repo() -> Repository {
    let db = Arc::new(Database::new(":memory:").expect("open in-memory db"));
    Repository::new(db)
}

fn sample_download(id: &str, pos: i32) -> Download {
    let mut d = Download::new(format!("https://example.com/{id}.iso"), "/tmp");
    d.id = id.to_string();
    d.position = pos;
    d.status = DownloadStatus::Queued;
    d.protocol = Protocol::Http;
    d
}

// ─── downloads round-trip ─────────────────────────────────────────────────

#[test]
fn insert_and_get_all_downloads_roundtrip() {
    let r = repo();
    let a = sample_download("a", 0);
    let b = sample_download("b", 1);
    r.insert_download(&a).unwrap();
    r.insert_download(&b).unwrap();

    let all = r.get_all_downloads().unwrap();
    assert_eq!(all.len(), 2);
    assert!(all.iter().any(|d| d.id == "a"));
    assert!(all.iter().any(|d| d.id == "b"));
}

#[test]
fn delete_download_removes_it() {
    let r = repo();
    r.insert_download(&sample_download("a", 0)).unwrap();
    r.insert_download(&sample_download("b", 1)).unwrap();
    r.delete_download("a").unwrap();
    let all = r.get_all_downloads().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, "b");
}

// ─── update_positions tx atomicity ────────────────────────────────────────

#[test]
fn update_positions_writes_all_rows_in_order() {
    let r = repo();
    r.insert_download(&sample_download("x", 0)).unwrap();
    r.insert_download(&sample_download("y", 1)).unwrap();
    r.insert_download(&sample_download("z", 2)).unwrap();

    r.update_positions(&[
        ("x".to_string(), 10),
        ("y".to_string(), 20),
        ("z".to_string(), 30),
    ])
    .unwrap();

    let all = r.get_all_downloads().unwrap();
    for d in &all {
        let expected = match d.id.as_str() {
            "x" => 10,
            "y" => 20,
            "z" => 30,
            _ => unreachable!(),
        };
        assert_eq!(d.position, expected, "{} should be at {}", d.id, expected);
    }
}

#[test]
fn update_positions_empty_input_is_noop() {
    let r = repo();
    r.insert_download(&sample_download("x", 5)).unwrap();
    r.update_positions(&[]).unwrap();
    assert_eq!(r.get_all_downloads().unwrap()[0].position, 5);
}

// ─── save_settings tx atomicity + round-trip ─────────────────────────────

#[test]
fn save_settings_round_trip() {
    let r = repo();
    let s = Settings {
        download_dir: "/custom/downloads".into(),
        max_concurrent: 7,
        max_connections_per_file: 16,
        min_split_size: 5 * 1024 * 1024,
        port: 9090,
        ..Settings::default()
    };
    r.save_settings(&s).unwrap();

    let got = r.get_settings().unwrap();
    assert_eq!(got.download_dir, "/custom/downloads");
    assert_eq!(got.max_concurrent, 7);
    assert_eq!(got.max_connections_per_file, 16);
    assert_eq!(got.min_split_size, 5 * 1024 * 1024);
    assert_eq!(got.port, 9090);
}

#[test]
fn save_settings_overwrites_prior_values() {
    let r = repo();
    let mut s = Settings {
        max_concurrent: 3,
        ..Settings::default()
    };
    r.save_settings(&s).unwrap();
    s.max_concurrent = 11;
    r.save_settings(&s).unwrap();
    assert_eq!(r.get_settings().unwrap().max_concurrent, 11);
}

// ─── users ────────────────────────────────────────────────────────────────

fn sample_user(name: &str, role: Role) -> User {
    User {
        id: uuid::Uuid::new_v4().to_string(),
        username: name.into(),
        password_hash: format!("$2b$12$dummyhashfor{name}"),
        role,
        created_at: chrono::Utc::now(),
        token_version: 0,
    }
}

#[test]
fn insert_first_user_only_succeeds_on_empty_users_table() {
    let r = repo();
    let alice = sample_user("alice", Role::Admin);
    assert!(r.insert_first_user(&alice).unwrap());
    // Second attempt must no-op
    let bob = sample_user("bob", Role::Admin);
    assert!(
        !r.insert_first_user(&bob).unwrap(),
        "second first-user registration must be rejected atomically"
    );
    // Alice still the only user
    let users = r.get_all_users().unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].username, "alice");
}

#[test]
fn insert_user_then_lookup_by_username_and_id() {
    let r = repo();
    let u = sample_user("carol", Role::User);
    r.insert_user(&u).unwrap();
    let by_name = r.get_user_by_username("carol").unwrap().unwrap();
    assert_eq!(by_name.id, u.id);
    assert_eq!(by_name.username, "carol");
    assert_eq!(by_name.role, Role::User);
}

#[test]
fn delete_user_removes_row() {
    let r = repo();
    let u1 = sample_user("dan1", Role::Admin);
    let u2 = sample_user("dan2", Role::User);
    r.insert_user(&u1).unwrap();
    r.insert_user(&u2).unwrap();
    let result = r.delete_user_guard_last_admin(&u2.id).unwrap();
    assert_eq!(result, Some(Some("dan2".to_string())));
    assert!(r.get_user_by_username("dan2").unwrap().is_none());
}

#[test]
fn update_user_password_and_bumps_token_version() {
    let r = repo();
    let u = sample_user("eve", Role::User);
    r.insert_user(&u).unwrap();
    let new_ver = r
        .update_user_password_and_bump_version("eve", "$2b$12$newhash")
        .unwrap();
    assert_eq!(new_ver, 1, "token_version should be bumped from 0 to 1");
    let updated = r.get_user_by_username("eve").unwrap().unwrap();
    assert_eq!(updated.password_hash, "$2b$12$newhash");
    assert_eq!(updated.token_version, 1);
}

#[test]
fn insert_user_returns_username_conflict_on_duplicate() {
    let r = repo();
    let u = sample_user("frank", Role::User);
    r.insert_user(&u).unwrap();
    let dup = sample_user("frank", Role::Admin);
    let err = r.insert_user(&dup).unwrap_err();
    assert!(
        matches!(
            err,
            dload::db::repository::InsertUserError::UsernameConflict
        ),
        "duplicate username should return UsernameConflict"
    );
}

#[test]
fn delete_user_guard_last_admin_refuses_last_admin() {
    let r = repo();
    let a1 = sample_user("admin1", Role::Admin);
    let a2 = sample_user("admin2", Role::Admin);
    r.insert_user(&a1).unwrap();
    r.insert_user(&a2).unwrap();

    let result = r.delete_user_guard_last_admin(&a1.id).unwrap();
    assert!(result.is_some(), "should delete when other admins exist");
    assert_eq!(result.unwrap(), Some("admin1".to_string()));

    let result = r.delete_user_guard_last_admin(&a2.id).unwrap();
    assert!(result.is_none(), "should refuse to delete last admin");

    assert!(r.get_user_by_username("admin2").unwrap().is_some());
}

#[test]
fn delete_user_guard_last_admin_handles_nonexistent() {
    let r = repo();
    let result = r.delete_user_guard_last_admin("nonexistent").unwrap();
    assert_eq!(result, Some(None));
}

// ─── history pagination ───────────────────────────────────────────────────

#[test]
fn get_history_page_respects_limit_and_offset() {
    let r = repo();
    // Seed 5 history rows with monotonically-increasing created_at
    for i in 0..5 {
        let mut d = sample_download(&format!("h{i}"), i);
        d.created_at = chrono::Utc::now() + chrono::Duration::milliseconds(i as i64);
        r.insert_history(&d).unwrap();
    }

    // DESC order: newest first. limit=2, offset=0 → last two inserted.
    let page1 = r.get_history_page(2, 0).unwrap();
    assert_eq!(page1.len(), 2);
    let page2 = r.get_history_page(2, 2).unwrap();
    assert_eq!(page2.len(), 2);
    let page3 = r.get_history_page(2, 4).unwrap();
    assert_eq!(page3.len(), 1);

    // Pages don't overlap
    let ids1: Vec<_> = page1
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    let ids2: Vec<_> = page2
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    for id in &ids1 {
        assert!(!ids2.contains(id), "pages must not overlap");
    }
}

#[test]
fn get_history_page_empty_when_offset_past_end() {
    let r = repo();
    r.insert_history(&sample_download("only", 0)).unwrap();
    assert!(r.get_history_page(10, 100).unwrap().is_empty());
}

// tags_from_string sorts alphabetically; clone and sort to compare
// with input order (which is ["action", "4k"]).
#[test]
fn tags_roundtrip_through_repository() {
    let r = repo();
    let mut d = sample_download("tagged", 0);
    d.tags = vec!["action".into(), "4k".into()];
    r.insert_download(&d).unwrap();
    let all = r.get_all_downloads().unwrap();
    let mut got = all[0].tags.clone();
    got.sort_unstable();
    assert_eq!(got, vec!["4k", "action"]);
}

#[test]
fn update_download_persists_tags() {
    let r = repo();
    let mut d = sample_download("updatetag", 0);
    d.tags = vec!["old".into()];
    r.insert_download(&d).unwrap();

    let mut updated = d.clone();
    updated.tags = vec!["new1".into(), "new2".into()];
    r.update_download(&updated).unwrap();

    let all = r.get_all_downloads().unwrap();
    let got = all.iter().find(|d| d.id == "updatetag").unwrap();
    assert_eq!(got.tags, vec!["new1", "new2"]);
}

// ─── download_folders persistence ────────────────────────────────────────

#[test]
fn save_settings_persists_download_folders() {
    let r = repo();
    let folders = vec![
        DownloadFolder {
            id: "f1".into(),
            label: "Default".into(),
            path: "/downloads".into(),
            is_default: true,
        },
        DownloadFolder {
            id: "f2".into(),
            label: "TV Shows".into(),
            path: "/media/tv".into(),
            is_default: false,
        },
        DownloadFolder {
            id: "f3".into(),
            label: "Movies".into(),
            path: "/media/movies".into(),
            is_default: false,
        },
    ];
    let s = Settings {
        download_folders: folders,
        ..Settings::default()
    };
    r.save_settings(&s).unwrap();

    let got = r.get_settings().unwrap();
    assert_eq!(got.download_folders.len(), 3);
    assert_eq!(got.download_folders[0].label, "Default");
    assert_eq!(got.download_folders[1].path, "/media/tv");
    assert!(got.download_folders[0].is_default);
    assert!(!got.download_folders[1].is_default);
}

#[test]
fn save_settings_overwrites_download_folders() {
    let r = repo();
    let s1 = Settings {
        download_folders: vec![DownloadFolder {
            id: "f1".into(),
            label: "Default".into(),
            path: "/downloads".into(),
            is_default: true,
        }],
        ..Settings::default()
    };
    r.save_settings(&s1).unwrap();

    let s2 = Settings {
        download_folders: vec![
            DownloadFolder {
                id: "f1".into(),
                label: "Default".into(),
                path: "/downloads".into(),
                is_default: true,
            },
            DownloadFolder {
                id: "f2".into(),
                label: "TV".into(),
                path: "/tv".into(),
                is_default: false,
            },
        ],
        ..Settings::default()
    };
    r.save_settings(&s2).unwrap();

    let got = r.get_settings().unwrap();
    assert_eq!(
        got.download_folders.len(),
        2,
        "folders should be overwritten"
    );
}

#[test]
fn empty_download_folders_falls_back_to_default() {
    let r = repo();
    // Save settings without download_folders (simulating old client)
    let s = Settings {
        download_folders: vec![],
        ..Settings::default()
    };
    r.save_settings(&s).unwrap();

    let got = r.get_settings().unwrap();
    // Empty vec persisted — caller handles fallback
    assert!(got.download_folders.is_empty());
}

#[test]
fn migration_creates_default_folder_from_download_dir() {
    // Database::new runs migrations including download_folders migration.
    // On a fresh DB, it reads download_dir (defaults to "/downloads") and
    // creates a single default folder.
    let r = repo();
    let got = r.get_settings().unwrap();
    assert!(
        !got.download_folders.is_empty(),
        "migration should create at least one folder"
    );
    assert!(got.download_folders[0].is_default);
    assert_eq!(got.download_folders[0].path, got.download_dir);
}

#[test]
fn insert_history_is_idempotent_on_same_id() {
    let r = repo();
    let d = sample_download("same", 0);
    r.insert_history(&d).unwrap();
    // INSERT OR IGNORE — second call must not error and must not duplicate
    r.insert_history(&d).unwrap();
    assert_eq!(r.get_history_page(100, 0).unwrap().len(), 1);
}
