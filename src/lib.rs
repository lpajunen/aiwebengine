use axum::body::Body;
use axum::http::Request;
use axum::http::StatusCode;
use axum::{Router, routing::get};
use axum::{extract::Path, response::IntoResponse};
use rquickjs::Value;
use rquickjs::{Context, Function, Runtime};
use std::sync::{Arc, mpsc};

pub mod repository;

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

fn spawn_js_worker(scripts: Vec<(String, String)>) -> anyhow::Result<mpsc::Sender<WorkerRequest>> {
    let (tx, rx) = mpsc::channel::<WorkerRequest>();

    std::thread::spawn(move || {
        let rt = match Runtime::new() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("js runtime init error: {}", e);
                return;
            }
        };
        let ctx = match Context::full(&rt) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("js context error: {}", e);
                return;
            }
        };

        // install host logging APIs into the JS global object before evaluating scripts
        if let Err(e) = ctx.with(|ctx| -> Result<(), rquickjs::Error> {
            let global = ctx.globals();

            let write_fn = Function::new(
                ctx.clone(),
                |_ctx: rquickjs::Ctx<'_>, msg: String| -> Result<(), rquickjs::Error> {
                    repository::insert_log_message(&msg);
                    Ok(())
                },
            )?;
            global.set("writeLog", write_fn)?;

            let list_fn = Function::new(
                ctx.clone(),
                |_ctx: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                    Ok(repository::fetch_log_messages())
                },
            )?;
            global.set("listLogs", list_fn)?;

            // Script management host functions
            let upsert_fn = Function::new(
                ctx.clone(),
                |_ctx: rquickjs::Ctx<'_>,
                 uri: String,
                 content: String|
                 -> Result<(), rquickjs::Error> {
                    repository::upsert_script(&uri, &content);
                    Ok(())
                },
            )?;
            global.set("upsertScript", upsert_fn)?;

            let get_fn = Function::new(
                ctx.clone(),
                |_ctx: rquickjs::Ctx<'_>, uri: String| -> Result<Option<String>, rquickjs::Error> {
                    Ok(repository::fetch_script(&uri))
                },
            )?;
            global.set("getScript", get_fn)?;

            let delete_fn = Function::new(
                ctx.clone(),
                |_ctx: rquickjs::Ctx<'_>, uri: String| -> Result<bool, rquickjs::Error> {
                    Ok(repository::delete_script(&uri))
                },
            )?;
            global.set("deleteScript", delete_fn)?;

            let list_scripts_fn = Function::new(
                ctx.clone(),
                |_ctx: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                    let map = repository::fetch_scripts();
                    Ok(map.keys().cloned().collect())
                },
            )?;
            global.set("listScripts", list_scripts_fn)?;

            Ok(())
        }) {
            eprintln!("failed to install host functions: {:?}", e);
            return;
        }

        // Evaluate all provided scripts in the JS context
        // Install a small bootstrap that provides a register(path, handler|handlerName)
        // helper and a handle(...) function usable by scripts. This is backward
        // compatible: register accepts either a function or a string name which
        // will be looked up on globalThis.
        let bootstrap = r#"
            globalThis._routes = globalThis._routes || new Map();
            globalThis.register = function(path, handler) {
                let h = handler;
                if (typeof handler === 'string') {
                    h = globalThis[handler];
                }
                globalThis._routes.set(path, h);
            };
            globalThis.handle = function(path, req) {
                const h = globalThis._routes && globalThis._routes.get(path);
                if (!h) return { status: 404, body: 'Not found' };
                try {
                    if (typeof h === 'function') return h(req);
                    return { status: 500, body: 'handler not callable' };
                } catch (e) {
                    return { status: 500, body: String(e) };
                }
            };
        "#;
        let _ = ctx.with(|ctx| ctx.eval::<(), _>(bootstrap));
        for (uri, script) in scripts.iter() {
            // set current script uri visible to host register() if needed
            let js_uri = serde_json::to_string(uri).unwrap_or("\"local\"".to_string());
            let set_code = format!("globalThis.__CURRENT_SCRIPT_URI = {}", js_uri);
            if let Err(e) = ctx.with(|ctx| ctx.eval::<(), _>(set_code.as_str())) {
                eprintln!("failed to set __CURRENT_SCRIPT_URI: {:?}", e);
            }
            if let Err(e) = ctx.with(|ctx| ctx.eval::<(), _>(script.as_str())) {
                // print debug info and the script contents to help diagnose QuickJS exceptions
                eprintln!(
                    "script eval error: {:?}\n--- script start ---\n{}\n--- script end ---",
                    e, script
                );
                return;
            }
            // clear current script uri
            let _ = ctx.with(|ctx| ctx.eval::<(), _>("globalThis.__CURRENT_SCRIPT_URI = undefined"));
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
                let res: Result<Value, rquickjs::Error> = handle.call((
                    req.path.clone(),
                    rquickjs::Object::new(ctx)
                        .and_then(|o| o.set("method", method.clone()).map(|_| o)),
                ));
                match res {
                    Ok(val) => {
                        // convert to object and read properties
                        let obj = match val.as_object() {
                            Some(o) => o,
                            None => return Err("expected object".to_string()),
                        };
                        let status: i32 = obj
                            .get("status")
                            .map_err(|e| format!("missing status: {}", e))?;
                        let body: String = obj
                            .get("body")
                            .map_err(|e| format!("missing body: {}", e))?;
                        Ok((status as u16, body))
                    }
                    Err(e) => Err(format!("call error: {:?}", e)),
                }
            });

            let _ = req.resp.send(reply);
        }
    });

    Ok(tx)
}

/// Start server on 0.0.0.0:4000 and load the JS script which can register paths.
pub async fn start_server_with_script(script_path: &str) -> anyhow::Result<()> {
    // gather scripts: evaluate the local bootstrap script first (it defines register/handle),
    // then evaluate repository scripts which call `register(...)`.
    let mut scripts_vec: Vec<(String, String)> = Vec::new();
    // add local script file first, use the script_path as the URI for the local script
    let local = std::fs::read_to_string(script_path)?;
    scripts_vec.push((script_path.to_string(), local));
    // fetch remote scripts and append as (uri, content)
    for (uri, content) in repository::fetch_scripts().into_iter() {
        scripts_vec.push((uri, content));
    }

    let tx = spawn_js_worker(scripts_vec)?;
    let tx = Arc::new(tx);

    let app = Router::new().route(
        "/{*path}",
        get(move |Path(path): Path<String>, req: Request<Body>| {
            let tx = tx.clone();
            async move {
                let (resp_tx, resp_rx) = mpsc::channel();
                let wr = WorkerRequest {
                    path: format!("/{}", path),
                    method: req.method().to_string(),
                    resp: resp_tx,
                };
                // send to worker (blocking send is fine)
                if let Err(e) = tx.send(wr) {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("send error: {}", e),
                    )
                        .into_response();
                }
                // wait for reply synchronously inside blocking task
                let result = tokio::task::spawn_blocking(move || resp_rx.recv()).await;
                match result {
                    Ok(Ok(Ok((status, body)))) => (
                        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                        body,
                    )
                        .into_response(),
                    Ok(Ok(Err(err))) => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("js error: {}", err),
                    )
                        .into_response(),
                    Ok(Err(recv_err)) => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("recv error: {}", recv_err),
                    )
                        .into_response(),
                    Err(join_err) => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("join error: {}", join_err),
                    )
                        .into_response(),
                }
            }
        }),
    );

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 4000));
    println!("listening on {}", addr);
    // use axum-server to run the app (lightweight server wrapper)
    axum_server::bind(addr)
        .serve(app.into_make_service())
        .await?;
    Ok(())
}
