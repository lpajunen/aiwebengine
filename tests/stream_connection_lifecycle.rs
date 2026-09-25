//! What happens to a stream connection between opening and going away.
//!
//! Both of these were broken in the same place, and for the same reason: the
//! state lived in `stream_registry` while the code that opened a connection
//! lived in `stream_manager`, whose `StreamConnectionManager` was constructed
//! fresh per connection, used once and dropped.
//!
//! So the limits compared against a map that was always empty — `0 >= limit`
//! on every connection — and the id handed to the SSE handler was a second
//! `Uuid::new_v4()` that this registry had never stored, so closing by it
//! matched nothing. Nothing aged the connections out either, which left every
//! SSE connection ever made sitting in the registry with a 1000-message
//! broadcast buffer until the process restarted.

use std::collections::HashMap;

use aiwebengine::stream_registry::{
    ConnectionGuard, GLOBAL_STREAM_REGISTRY, OpenError, StreamLimits,
};

/// Each test gets its own path, since the registry is global.
fn register(path: &str) {
    GLOBAL_STREAM_REGISTRY
        .register_stream(path, "test://stream_lifecycle", None)
        .expect("register the stream");
}

fn connections_on(path: &str) -> usize {
    GLOBAL_STREAM_REGISTRY
        .get_stream_stats()
        .expect("stats")
        .get(path)
        .and_then(|value| value.get("connection_count"))
        .and_then(|value| value.as_u64())
        .unwrap_or(0) as usize
}

#[test]
fn the_id_handed_out_is_the_id_the_registry_stored() {
    let path = "/lifecycle/id_round_trip";
    register(path);

    let opened = GLOBAL_STREAM_REGISTRY
        .open_connection(path, None, StreamLimits::default())
        .expect("open");
    assert_eq!(connections_on(path), 1);

    // This is what the SSE handler does when the response is dropped. Before
    // the fix it answered false and the count stayed at 1.
    let closed = GLOBAL_STREAM_REGISTRY.close_connection(path, &opened.connection_id);

    assert!(closed, "closing by the returned id should remove it");
    assert_eq!(
        connections_on(path),
        0,
        "the connection should be gone from the registry"
    );
}

#[test]
fn dropping_the_guard_closes_the_connection() {
    let path = "/lifecycle/guard_drop";
    register(path);

    let opened = GLOBAL_STREAM_REGISTRY
        .open_connection(path, None, StreamLimits::default())
        .expect("open");

    {
        let _guard = ConnectionGuard::new(path.to_string(), opened.connection_id.clone());
        assert_eq!(connections_on(path), 1, "still open inside the scope");
    }

    // A client disconnect produces no event in the SSE pipeline: axum drops
    // the response body and nothing is polled. The guard's Drop is the only
    // thing that fires, which is why cleanup hangs off it.
    assert_eq!(
        connections_on(path),
        0,
        "dropping the guard should have closed the connection"
    );
}

#[test]
fn the_per_stream_limit_is_enforced() {
    let path = "/lifecycle/per_stream_limit";
    register(path);

    let limits = StreamLimits {
        max_connections_per_stream: 2,
        max_total_connections: 100,
    };

    let _first = GLOBAL_STREAM_REGISTRY
        .open_connection(path, None, limits)
        .expect("first connection is under the limit");
    let _second = GLOBAL_STREAM_REGISTRY
        .open_connection(path, None, limits)
        .expect("second connection is at the limit");

    let third = GLOBAL_STREAM_REGISTRY.open_connection(path, None, limits);

    match third {
        Err(OpenError::StreamFull { limit, .. }) => assert_eq!(limit, 2),
        other => panic!(
            "expected the stream to be full, got {:?}",
            other.map(|_| ())
        ),
    }
    assert_eq!(
        connections_on(path),
        2,
        "the refusal must not have inserted"
    );
}

#[test]
fn a_refusal_is_capacity_not_an_internal_error() {
    let path = "/lifecycle/capacity_kind";
    register(path);

    let limits = StreamLimits {
        max_connections_per_stream: 1,
        max_total_connections: 100,
    };
    let _held = GLOBAL_STREAM_REGISTRY
        .open_connection(path, None, limits)
        .expect("first");

    let refused = GLOBAL_STREAM_REGISTRY
        .open_connection(path, None, limits)
        .expect_err("second should be refused");

    // The SSE handler branches on this to answer 503 rather than 500.
    assert!(refused.is_capacity());
    assert!(
        !OpenError::Internal("lock".to_string()).is_capacity(),
        "an internal error is not a capacity refusal"
    );
}

#[test]
fn closing_frees_a_slot() {
    let path = "/lifecycle/slot_reuse";
    register(path);

    let limits = StreamLimits {
        max_connections_per_stream: 1,
        max_total_connections: 100,
    };

    let first = GLOBAL_STREAM_REGISTRY
        .open_connection(path, None, limits)
        .expect("first");
    assert!(
        GLOBAL_STREAM_REGISTRY
            .open_connection(path, None, limits)
            .is_err(),
        "the stream is full"
    );

    GLOBAL_STREAM_REGISTRY.close_connection(path, &first.connection_id);

    // The point of fixing the id: a leaked connection would hold this slot
    // forever, so the stream would never accept another client.
    GLOBAL_STREAM_REGISTRY
        .open_connection(path, None, limits)
        .expect("the closed connection should have freed its slot");
}

#[test]
fn an_unregistered_path_is_refused_and_is_not_a_capacity_problem() {
    let refused = GLOBAL_STREAM_REGISTRY
        .open_connection("/lifecycle/never_registered", None, StreamLimits::default())
        .expect_err("nothing is registered there");

    assert!(matches!(refused, OpenError::NotRegistered(_)));
    assert!(!refused.is_capacity());
}

#[test]
fn client_metadata_survives_the_open() {
    let path = "/lifecycle/metadata";
    register(path);

    let mut metadata = HashMap::new();
    metadata.insert("role".to_string(), "admin".to_string());

    let opened = GLOBAL_STREAM_REGISTRY
        .open_connection(path, Some(metadata), StreamLimits::default())
        .expect("open");

    // Filtered broadcast is the reason metadata exists, so a connection that
    // matches should receive and the count should confirm it was stored.
    let mut filter = HashMap::new();
    filter.insert("role".to_string(), "admin".to_string());
    let result = GLOBAL_STREAM_REGISTRY
        .broadcast_to_stream_with_filter_local(path, "hello", &filter)
        .expect("filtered broadcast");
    assert_eq!(result.successful_sends, 1);

    GLOBAL_STREAM_REGISTRY.close_connection(path, &opened.connection_id);
}
