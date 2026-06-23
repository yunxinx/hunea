use std::fs;

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
