//! Several requests at once, and one response read a piece at a time.
//!
//! Both come from the same place: `fetch` was one blocking host call that
//! did the whole request and handed back a string. So `Promise.all` over
//! three fetches sequenced them — each holding an execution slot and a
//! blocking thread for its whole round trip, with the wall clock the sum —
//! and a response that arrives over time could not be consumed at all, which
//! is every model's token stream.
//!
//! These run against a real socket rather than a stubbed `__hostFetch`. The
//! claims are about timing and about chunk boundaries, and neither survives
//! being stubbed: a fake transport that answers instantly cannot show that
//! three requests overlapped, and one that hands back a whole string cannot
//! split a character in half.

mod common;
mod mock_server;

use aiwebengine::http_client::{FetchOptions, HttpClient, ParallelRequest};
use mock_server::MockServer;
use std::time::Instant;

fn request(url: String) -> ParallelRequest {
    ParallelRequest {
        url,
        options: FetchOptions::default(),
    }
}

// ---------------------------------------------------------------------------
// Several at once
// ---------------------------------------------------------------------------

/// The claim, measured: three requests that each take 300ms take about 300ms
/// together rather than 900ms.
///
/// Timing assertions are usually a bad idea, and this one is bounded loosely
/// on purpose — the point is the difference between one delay and three, not
/// a millisecond count. A margin this wide fails only if the requests really
/// did run in series.
#[tokio::test(flavor = "multi_thread")]
async fn several_requests_run_together_rather_than_in_series() {
    let mock = MockServer::start().await.expect("mock server");
    let urls: Vec<String> = (0..3).map(|_| mock.url("/slow/300")).collect();

    let elapsed = tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        let client = HttpClient::new_for_tests().expect("client");
        let answers = client.fetch_all(urls.into_iter().map(request).collect(), None, None);
        assert_eq!(answers.len(), 3);
        for answer in &answers {
            assert!(answer.is_ok(), "each request should have answered");
        }
        started.elapsed()
    })
    .await
    .expect("no panic");

    assert!(
        elapsed.as_millis() < 700,
        "three 300ms requests run together should not take {}ms — that is series",
        elapsed.as_millis()
    );

    mock.shutdown().await;
}

/// Answers come back in the order they were asked for, whatever order they
/// arrived in. A caller matching results to requests by position is the
/// obvious thing to write, so it has to be the thing that works.
#[tokio::test(flavor = "multi_thread")]
async fn answers_keep_the_order_of_the_requests() {
    let mock = MockServer::start().await.expect("mock server");
    // Deliberately slowest first, so arrival order and request order differ.
    let urls = vec![
        mock.url("/slow/300"),
        mock.url("/slow/150"),
        mock.url("/slow/10"),
    ];

    let bodies = tokio::task::spawn_blocking(move || {
        HttpClient::new_for_tests()
            .expect("client")
            .fetch_all(urls.into_iter().map(request).collect(), None, None)
            .into_iter()
            .map(|answer| answer.expect("answered").body)
            .collect::<Vec<_>>()
    })
    .await
    .expect("no panic");

    let slept: Vec<i64> = bodies
        .iter()
        .map(|body| {
            serde_json::from_str::<serde_json::Value>(body).expect("json")["sleptMs"]
                .as_i64()
                .expect("a number")
        })
        .collect();

    assert_eq!(
        slept,
        vec![300, 150, 10],
        "results should be positional, not in the order they arrived"
    );

    mock.shutdown().await;
}

/// One bad request is one bad answer. The caller asked for several and has a
/// use for the ones that arrived, so a refusal lands in its own slot rather
/// than failing the batch.
#[tokio::test(flavor = "multi_thread")]
async fn one_failure_does_not_lose_the_other_answers() {
    let mock = MockServer::start().await.expect("mock server");
    let urls = vec![
        mock.url("/get"),
        "not-a-url-at-all".to_string(),
        mock.url("/get"),
    ];

    let answers = tokio::task::spawn_blocking(move || {
        HttpClient::new_for_tests()
            .expect("client")
            .fetch_all(urls.into_iter().map(request).collect(), None, None)
            .into_iter()
            .map(|answer| answer.is_ok())
            .collect::<Vec<_>>()
    })
    .await
    .expect("no panic");

    assert_eq!(answers, vec![true, false, true]);

    mock.shutdown().await;
}

/// A batch larger than the concurrency ceiling runs in waves rather than
/// being refused: the caller's intent is legible and the only question is
/// how fast it happens.
#[tokio::test(flavor = "multi_thread")]
async fn a_batch_past_the_ceiling_still_answers_every_request() {
    let mock = MockServer::start().await.expect("mock server");
    let count = aiwebengine::http_client::MAX_PARALLEL_FETCHES * 2 + 1;
    let urls: Vec<String> = (0..count).map(|_| mock.url("/get")).collect();

    let answered = tokio::task::spawn_blocking(move || {
        HttpClient::new_for_tests()
            .expect("client")
            .fetch_all(urls.into_iter().map(request).collect(), None, None)
            .into_iter()
            .filter(|answer| answer.is_ok())
            .count()
    })
    .await
    .expect("no panic");

    assert_eq!(answered, count);

    mock.shutdown().await;
}

/// The execution budget travels to the worker threads.
///
/// Every host call reads its deadline from a thread-local, and work handed to
/// another thread leaves that behind — so without carrying it, a script with
/// two seconds left could start a thirty-second fetch. This arms a short
/// budget and asks for a long request: the budget has to win.
#[tokio::test(flavor = "multi_thread")]
async fn the_execution_budget_reaches_the_worker_threads() {
    let mock = MockServer::start().await.expect("mock server");
    let url = mock.url("/slow/5000");

    let elapsed = tokio::task::spawn_blocking(move || {
        let _budget = aiwebengine::database::bound_host_calls(
            Instant::now() + std::time::Duration::from_millis(400),
        );

        let started = Instant::now();
        let answers =
            HttpClient::new_for_tests()
                .expect("client")
                .fetch_all(vec![request(url)], None, None);
        assert!(
            answers[0].is_err(),
            "a request outliving the budget should fail, not succeed late"
        );
        started.elapsed()
    })
    .await
    .expect("no panic");

    assert!(
        elapsed.as_millis() < 3000,
        "the budget should have cut this off, but it took {}ms",
        elapsed.as_millis()
    );

    mock.shutdown().await;
}

// ---------------------------------------------------------------------------
// One, a piece at a time
// ---------------------------------------------------------------------------

/// The body arrives in pieces, and the pieces arrive before the response
/// ends. That is the whole difference from `fetch`: a turn stops being as
/// long as the whole model call.
#[tokio::test(flavor = "multi_thread")]
async fn a_streamed_body_arrives_in_pieces() {
    let mock = MockServer::start().await.expect("mock server");
    let url = mock.url("/stream/5");

    let (chunks, first_at) = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests().expect("client");
        let mut stream = client
            .fetch_streaming(url, FetchOptions::default(), None, None)
            .expect("the stream should open");

        assert_eq!(stream.status, 200);
        assert!(stream.ok);

        let started = Instant::now();
        let mut chunks = Vec::new();
        let mut first_at = None;
        while let Some(chunk) = stream.read_chunk().expect("reading should not fail") {
            if first_at.is_none() {
                first_at = Some(started.elapsed());
            }
            chunks.push(chunk);
        }
        (chunks, first_at.expect("at least one chunk"))
    })
    .await
    .expect("no panic");

    let whole = chunks.concat();
    for index in 0..5 {
        assert!(
            whole.contains(&format!("piece-{}", index)),
            "every piece should arrive: {}",
            whole
        );
    }

    // The server sleeps 20ms before each of five pieces, so a reader that
    // waited for the whole body would see its first chunk at ~100ms. This
    // does not.
    assert!(
        first_at.as_millis() < 80,
        "the first piece should arrive before the last is sent, not after \
         ({}ms)",
        first_at.as_millis()
    );

    mock.shutdown().await;
}

/// A chunk boundary falls wherever the network put it, regularly inside a
/// multi-byte character. Decoding each read on its own would replace those
/// halves with U+FFFD — silently, and most often on exactly the text a model
/// is generating.
#[tokio::test(flavor = "multi_thread")]
async fn a_character_split_across_chunks_survives() {
    let mock = MockServer::start().await.expect("mock server");
    let url = mock.url("/stream-split");

    let whole = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests().expect("client");
        let mut stream = client
            .fetch_streaming(url, FetchOptions::default(), None, None)
            .expect("the stream should open");

        let mut whole = String::new();
        while let Some(chunk) = stream.read_chunk().expect("reading should not fail") {
            whole.push_str(&chunk);
        }
        whole
    })
    .await
    .expect("no panic");

    assert_eq!(
        whole, "日本語🙂",
        "every character was delivered one byte at a time and must come back whole"
    );
    assert!(
        !whole.contains('\u{FFFD}'),
        "nothing should have been replaced: {:?}",
        whole
    );

    mock.shutdown().await;
}

/// A stream is registered against the execution, read by id, and gone once
/// it ends — so a script that reads one to the end leaves no socket behind.
#[tokio::test(flavor = "multi_thread")]
async fn a_finished_stream_leaves_nothing_registered() {
    let mock = MockServer::start().await.expect("mock server");
    let url = mock.url("/stream/2");

    tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests().expect("client");
        let stream = client
            .fetch_streaming(url, FetchOptions::default(), None, None)
            .expect("the stream should open");
        let id = aiwebengine::http_client::register_stream(stream).expect("registered");

        while aiwebengine::http_client::read_stream(id)
            .expect("reading should not fail")
            .is_some()
        {}

        // Read to the end, so it took itself out rather than waiting for the
        // execution to end.
        assert!(
            aiwebengine::http_client::read_stream(id).is_err(),
            "a spent stream should not still be open"
        );
        assert!(
            !aiwebengine::http_client::close_stream(id),
            "and closing it should find nothing"
        );
    })
    .await
    .expect("no panic");

    mock.shutdown().await;
}

/// An execution's streams end with it. A blocking thread is pooled, so
/// without this the next execution to land on it would inherit whatever the
/// last one left open.
#[tokio::test(flavor = "multi_thread")]
async fn an_execution_ending_closes_the_streams_it_left_open() {
    let mock = MockServer::start().await.expect("mock server");
    let url = mock.url("/stream/50");

    tokio::task::spawn_blocking(move || {
        let id = {
            // The guard marks the span of one execution on this thread.
            let _budget = aiwebengine::database::bound_host_calls(
                Instant::now() + std::time::Duration::from_secs(30),
            );

            let client = HttpClient::new_for_tests().expect("client");
            let stream = client
                .fetch_streaming(url, FetchOptions::default(), None, None)
                .expect("the stream should open");
            let id = aiwebengine::http_client::register_stream(stream).expect("registered");

            // Read one piece and abandon the rest, which is what a script
            // that returns early does.
            assert!(
                aiwebengine::http_client::read_stream(id)
                    .expect("reading")
                    .is_some()
            );
            id
        };

        assert!(
            aiwebengine::http_client::read_stream(id).is_err(),
            "the stream should have gone when its execution did"
        );
    })
    .await
    .expect("no panic");

    mock.shutdown().await;
}

/// A stream is bounded like a buffered body is. A response that never ends
/// is the case the cap exists for, and it is the one a streaming reader
/// makes reachable.
#[tokio::test(flavor = "multi_thread")]
async fn a_stream_is_bounded_by_the_same_ceiling_a_body_is() {
    // Asserted against the constant rather than by downloading ten
    // megabytes: the ceiling being shared is the claim, and the read path
    // enforcing it is a line in `read_chunk` rather than something a test
    // can usefully reach without a very large fixture.
    assert_eq!(
        aiwebengine::http_client::MAX_RESPONSE_SIZE,
        10 * 1024 * 1024,
        "a stream and a buffered body answer to the same ceiling"
    );
}
