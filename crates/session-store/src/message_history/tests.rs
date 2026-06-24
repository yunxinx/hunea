use std::fs;
use std::sync::Arc;

use crate::metadata::MetadataIndex;

fn temp_index_path(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "hunea-message-history-{label}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir.join("index.sqlite")
}

#[tokio::test]
async fn message_history_table_exists_after_index_open() {
    let path = temp_index_path("schema");
    let _index = MetadataIndex::open(&path).await.expect("index should open");
    let conn = rusqlite::Connection::open(&path).expect("sqlite");
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='message_history'",
            [],
            |row| row.get(0),
        )
        .expect("query");
    assert_eq!(count, 1);
    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}

#[tokio::test]
async fn record_dedup_adjacent_and_trim() {
    let path = temp_index_path("dedup-trim");
    let index = MetadataIndex::open(&path).await.expect("index should open");

    for i in 0..5 {
        index
            .record_message_history(format!("msg-{i}"), 3)
            .await
            .expect("record");
    }
    index
        .record_message_history("msg-4".to_string(), 3)
        .await
        .expect("adjacent dup");

    let all = index.load_message_history_all().await.expect("load all");
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].text, "msg-2");
    assert_eq!(all[2].text, "msg-4");

    let recent = index.load_message_history_recent(25).await.expect("recent");
    assert_eq!(recent.len(), 3);
    assert_eq!(recent[0].text, "msg-2");
    assert_eq!(recent[2].text, "msg-4");
    assert!(recent[0].ts <= recent[2].ts);

    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}

#[tokio::test]
async fn record_skips_whitespace_only_text() {
    let path = temp_index_path("whitespace");
    let index = MetadataIndex::open(&path).await.expect("index should open");

    index
        .record_message_history("   \t\n  ".to_string(), 25)
        .await
        .expect("record");
    index
        .record_message_history("real".to_string(), 25)
        .await
        .expect("record");

    let all = index.load_message_history_all().await.expect("load");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].text, "real");

    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}

#[tokio::test]
async fn concurrent_adjacent_duplicate_records_once() {
    let path = temp_index_path("concurrent-dedup");
    let index = Arc::new(MetadataIndex::open(&path).await.expect("index should open"));

    for round in 0..8 {
        let text = format!("same concurrent prompt {round}");
        let barrier = Arc::new(tokio::sync::Barrier::new(129));
        let mut tasks = Vec::new();
        for _ in 0..128 {
            let index = Arc::clone(&index);
            let barrier = Arc::clone(&barrier);
            let text = text.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                index
                    .record_message_history(text, 100)
                    .await
                    .expect("record should succeed")
            }));
        }
        barrier.wait().await;
        for task in tasks {
            task.await.expect("record task should not panic");
        }
    }

    let all = index.load_message_history_all().await.expect("load");
    assert_eq!(all.len(), 8);
    for (round, row) in all.iter().enumerate() {
        assert_eq!(row.text, format!("same concurrent prompt {round}"));
    }

    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}

#[tokio::test]
async fn lowered_limit_trims_on_next_insert() {
    let path = temp_index_path("lower-limit");
    let index = MetadataIndex::open(&path).await.expect("index should open");

    for i in 0..5 {
        index
            .record_message_history(format!("line-{i}"), 100)
            .await
            .expect("record");
    }
    index
        .record_message_history("line-extra".to_string(), 2)
        .await
        .expect("record with lower limit");

    let all = index.load_message_history_all().await.expect("load");
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].text, "line-4");
    assert_eq!(all[1].text, "line-extra");

    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}

#[tokio::test]
async fn lowered_limit_trims_on_adjacent_duplicate() {
    let path = temp_index_path("lower-limit-duplicate");
    let index = MetadataIndex::open(&path).await.expect("index should open");

    for i in 0..5 {
        index
            .record_message_history(format!("line-{i}"), 100)
            .await
            .expect("record");
    }
    index
        .record_message_history("line-4".to_string(), 2)
        .await
        .expect("duplicate record with lower limit");

    let all = index.load_message_history_all().await.expect("load");
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].text, "line-3");
    assert_eq!(all[1].text, "line-4");

    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}
