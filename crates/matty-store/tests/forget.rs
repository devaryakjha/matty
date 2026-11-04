//! Sign out deletes the account's cache (#160).
#![allow(clippy::unwrap_used)]

use matty_store::{Store, database_file_name, forget};

fn files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files_under(&path));
        } else {
            found.push(path);
        }
    }
    found
}

#[test]
fn forgetting_an_account_leaves_nothing_that_names_the_user() {
    let user_id = "u8kq3x9wjbdizqp7m1n2";
    let server = url::Url::parse("https://chat.example.com").unwrap();
    let root = tempfile::tempdir().unwrap();
    let database = root
        .path()
        .join("accounts")
        .join(database_file_name(&server, user_id));
    std::fs::create_dir_all(database.parent().unwrap()).unwrap();
    let media = root.path().join("media").join("account");
    std::fs::create_dir_all(&media).unwrap();
    std::fs::write(media.join("thumb"), user_id).unwrap();
    let store = Store::open(&database).unwrap();
    store
        .conn()
        .execute_batch(&format!(
            "CREATE TABLE t(x); INSERT INTO t VALUES ('{user_id}');"
        ))
        .unwrap();
    drop(store);

    forget(&database, Some(&media)).unwrap();

    for file in files_under(root.path()) {
        assert!(!file.to_string_lossy().contains(user_id), "{file:?}");
        let bytes = std::fs::read(&file).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(user_id),
            "{file:?}"
        );
    }
    // Forgetting again is fine.
    forget(&database, Some(&media)).unwrap();
}
