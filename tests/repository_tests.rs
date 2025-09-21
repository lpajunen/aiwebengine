use aiwebengine::repository;

#[test]
fn dynamic_script_lifecycle() {
    // initial static scripts
    let scripts = repository::fetch_scripts();
    assert!(scripts.contains_key("https://example.com/core"));

    // upsert a dynamic script
    repository::upsert_script(
        "https://example.com/dyn",
        "register('/dyn', (req) => ({ status: 200, body: 'dyn' }));",
    );
    let scripts = repository::fetch_scripts();
    assert!(scripts.contains_key("https://example.com/dyn"));

    // fetch single
    let one = repository::fetch_script("https://example.com/dyn");
    assert!(one.is_some());
    assert!(one.unwrap().contains("/dyn"));

    // delete it
    let removed = repository::delete_script("https://example.com/dyn");
    assert!(removed);

    let scripts = repository::fetch_scripts();
    assert!(!scripts.contains_key("https://example.com/dyn"));
}

#[test]
fn upsert_overwrites_existing_script() {
    let uri = "https://example.com/dyn2";
    let content_v1 = "register('/dyn2', (req) => ({ status: 200, body: 'v1' }));";
    let content_v2 = "register('/dyn2', (req) => ({ status: 200, body: 'v2' }));";

    // upsert v1 and verify
    repository::upsert_script(uri, content_v1);
    let got = repository::fetch_script(uri);
    assert!(got.is_some());
    assert!(got.unwrap().contains("v1"));

    // upsert v2 and verify update
    repository::upsert_script(uri, content_v2);
    let got2 = repository::fetch_script(uri);
    assert!(got2.is_some());
    assert!(got2.unwrap().contains("v2"));

    // cleanup
    let _ = repository::delete_script(uri);
}

#[test]
fn insert_and_list_log_messages() {
    // record starting length so test is robust to previous state
    let start = repository::fetch_log_messages("test").len();

    repository::insert_log_message("test", "log-one");
    repository::insert_log_message("test", "log-two");

    let msgs = repository::fetch_log_messages("test");
    assert!(
        msgs.len() >= start + 2,
        "expected at least two new messages"
    );
    // last two messages should be the ones we inserted
    let last = &msgs[msgs.len() - 2..];
    assert_eq!(last[0], "log-one");
    assert_eq!(last[1], "log-two");
}

#[test]
fn prune_keeps_latest_20_logs() {
    // insert 25 distinct messages
    for i in 0..25 {
        repository::insert_log_message("test", &format!("prune-test-{}", i));
    }

    repository::prune_log_messages();
    let msgs = repository::fetch_log_messages("test");
    assert!(msgs.len() <= 20, "prune should keep at most 20 messages");

    // ensure the latest message is the last one we inserted
    if let Some(last) = msgs.last() {
        assert!(
            last.contains("prune-test-24"),
            "expected latest message to be prune-test-24"
        );
    } else {
        panic!("no messages after prune");
    }
}
