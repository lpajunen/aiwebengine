use aiwebengine::repository;

#[test]
fn test_asset_management() {
    // Test static asset
    let asset = repository::fetch_asset("/logo.svg");
    assert!(asset.is_some());
    let asset = asset.unwrap();
    assert_eq!(asset.public_path, "/logo.svg");
    assert_eq!(asset.mimetype, "image/svg+xml");
    assert!(!asset.content.is_empty());

    // Test listing assets
    let assets = repository::fetch_assets();
    assert!(assets.contains_key("/logo.svg"));

    // Test upsert and fetch dynamic asset
    let test_content = b"test content".to_vec();
    let test_asset = repository::Asset {
        public_path: "/test.txt".to_string(),
        mimetype: "text/plain".to_string(),
        content: test_content.clone(),
    };
    repository::upsert_asset(test_asset);

    let fetched = repository::fetch_asset("/test.txt");
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.public_path, "/test.txt");
    assert_eq!(fetched.mimetype, "text/plain");
    assert_eq!(fetched.content, test_content);

    // Test delete
    let deleted = repository::delete_asset("/test.txt");
    assert!(deleted);

    // Verify it's gone
    let fetched_after_delete = repository::fetch_asset("/test.txt");
    assert!(fetched_after_delete.is_none());
}
