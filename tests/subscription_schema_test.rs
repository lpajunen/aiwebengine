mod common;

use common::{TestContext, wait_for_server};
use serde_json::{Value, json};
use tracing::info;

#[tokio::test]
async fn test_subscription_schema_configured() {
    let _ = tracing_subscriber::fmt::try_init();

    let context = TestContext::new();

    // Start the server
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    info!("Server started on port: {}", port);

    // Test GraphQL introspection query for subscription type
    let client = reqwest::Client::new();
    let introspection_query = json!({
        "query": "query { __schema { subscriptionType { name fields { name } } } }"
    });

    let response = client
        .post(format!("http://127.0.0.1:{}/graphql", port))
        .json(&introspection_query)
        .send()
        .await
        .expect("Failed to send introspection query");

    let response_body: Value = response.json().await.expect("Failed to parse response");

    // Check if subscription type exists
    if let Some(schema) = response_body.get("data").and_then(|d| d.get("__schema")) {
        if let Some(subscription_type) = schema.get("subscriptionType") {
            if subscription_type.is_null() {
                panic!("❌ GraphQL subscription type is not configured");
            } else {
                info!("✅ GraphQL subscription type is configured!");
                info!("Subscription type: {:?}", subscription_type);

                // Check for expected subscription fields
                if let Some(fields) = subscription_type.get("fields")
                    && let Some(fields_array) = fields.as_array()
                {
                    let field_names: Vec<String> = fields_array
                        .iter()
                        .filter_map(|f| f.get("name").and_then(|n| n.as_str().map(String::from)))
                        .collect();

                    info!("Available subscription fields: {:?}", field_names);

                    // We expect at least one subscription field (scriptUpdates from core.js)
                    assert!(
                        !field_names.is_empty(),
                        "Expected at least one subscription field"
                    );
                    assert!(
                        field_names.contains(&"scriptUpdates".to_string()),
                        "Expected 'scriptUpdates' subscription field to be present"
                    );
                }
            }
        } else {
            panic!("❌ No subscriptionType field in schema");
        }
    } else {
        panic!("❌ Invalid GraphQL schema response: {:?}", response_body);
    }

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}
