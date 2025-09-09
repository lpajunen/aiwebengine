use aiwebengine::repository;

#[test]
fn dynamic_script_lifecycle() {
    // initial static scripts
    let scripts = repository::fetch_scripts();
    assert!(scripts.contains_key("https://example.com/core"));
    assert!(scripts.contains_key("https://example.com/helloworld"));

    // upsert a dynamic script
    repository::upsert_script("https://example.com/dyn", "register('/dyn', (req) => ({ status: 200, body: 'dyn' }));");
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
