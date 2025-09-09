use axum::{routing::get, Router};
use axum::http::StatusCode;
use axum::{extract::Path, response::IntoResponse};
use axum::body::Body;
use axum::http::Request;
use std::sync::{mpsc, Arc};
use rquickjs::{Runtime, Context, Function};
use rquickjs::Value;

/// Handler returning a concrete response tuple for easier testing.
pub async fn root() -> (StatusCode, &'static str) {
    (StatusCode::OK, "Hello from axum!")
}

/// Build the axum `Router` for the application.
pub fn app() -> Router {
    Router::new().route("/", get(root))
}

// --- JS worker + server -------------------------------------------------

struct WorkerRequest {
    path: String,
    method: String,
    resp: mpsc::Sender<Result<(u16, String), String>>,
}

fn spawn_js_worker(script_path: &str) -> anyhow::Result<mpsc::Sender<WorkerRequest>> {
    let script = std::fs::read_to_string(script_path)?;
    let (tx, rx) = mpsc::channel::<WorkerRequest>();

    std::thread::spawn(move || {
        let rt = match Runtime::new() {
            Ok(r) => r,
            Err(e) => { eprintln!("js runtime init error: {}", e); return; }
        };
        let ctx = match Context::full(&rt) {
            Ok(c) => c,
            Err(e) => { eprintln!("js context error: {}", e); return; }
        };

    if let Err(e) = ctx.with(|ctx| ctx.eval::<(), _>(script.as_str())) {
            eprintln!("script eval error: {}", e);
            return;
        }

        while let Ok(req) = rx.recv() {
            let reply = ctx.with(|ctx| {
                let global = ctx.globals();
                let handle: Function = match global.get("handle") {
                    Ok(h) => h,
                    Err(e) => return Err(format!("no handle: {}", e)),
                };
                // call handle(path, { method: "GET" })
                // create a small request object for the JS handler
                let method = req.method.clone();
                let res: Result<Value, rquickjs::Error> = handle.call((req.path.clone(), rquickjs::Object::new(ctx).and_then(|o| { o.set("method", method.clone()).map(|_| o) })));
                match res {
                    Ok(val) => {
                        // convert to object and read properties
                        let obj = match val.as_object() {
                            Some(o) => o,
                            None => return Err("expected object".to_string()),
                        };
                        let status: i32 = obj.get("status").map_err(|e| format!("missing status: {}", e))?;
                        let body: String = obj.get("body").map_err(|e| format!("missing body: {}", e))?;
                        Ok((status as u16, body))
                    }
                    Err(e) => Err(format!("call error: {}", e)),
                }
            });

            let _ = req.resp.send(reply);
        }
    });

    Ok(tx)
}

/// Start server on 0.0.0.0:4000 and load the JS script which can register paths.
pub async fn start_server_with_script(script_path: &str) -> anyhow::Result<()> {
    let tx = spawn_js_worker(script_path)?;
    let tx = Arc::new(tx);

    let app = Router::new().route("/*path", get(move |Path(path): Path<String>, req: Request<Body>| {
        let tx = tx.clone();
        async move {
            let (resp_tx, resp_rx) = mpsc::channel();
            let wr = WorkerRequest { path: format!("/{}", path), method: req.method().to_string(), resp: resp_tx };
            // send to worker (blocking send is fine)
            if let Err(e) = tx.send(wr) {
                return (StatusCode::INTERNAL_SERVER_ERROR, format!("send error: {}", e)).into_response();
            }
            // wait for reply synchronously inside blocking task
            let result = tokio::task::spawn_blocking(move || resp_rx.recv()).await;
            match result {
                Ok(Ok(Ok((status, body)))) => (StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), body).into_response(),
                Ok(Ok(Err(err))) => (StatusCode::INTERNAL_SERVER_ERROR, format!("js error: {}", err)).into_response(),
                Ok(Err(recv_err)) => (StatusCode::INTERNAL_SERVER_ERROR, format!("recv error: {}", recv_err)).into_response(),
                Err(join_err) => (StatusCode::INTERNAL_SERVER_ERROR, format!("join error: {}", join_err)).into_response(),
            }
        }
    }));

    let addr = std::net::SocketAddr::from(([0,0,0,0], 4000));
    println!("listening on {}", addr);
    // use axum-server to run the app (lightweight server wrapper)
    axum_server::bind(addr).serve(app.into_make_service()).await?;
    Ok(())
}
