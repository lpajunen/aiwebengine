use axum::body::Body;
use axum::http::Request;
use axum::http::StatusCode;
use axum::{Router, routing::get};
use axum::{extract::Path, response::IntoResponse};
use rquickjs::{Context, Function, Runtime, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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

// Stateless per-request execution model:
// - Build registrations at startup by evaluating the bootstrap script and
//   repository scripts in ephemeral contexts that call `register(path, handler)`.
// - On each HTTP request, create a fresh QuickJS Runtime/Context inside a
//   blocking task, evaluate the bootstrap and owner script, call handler, and
//   return the response. A timeout protects long-running scripts.

/// Build registrations by evaluating each script once and capturing calls to
/// `register(path, handlerName)`. The returned map is shared by the server.
fn build_registrations(
    script_path: &str,
    local_script: &str,
) -> anyhow::Result<Arc<Mutex<HashMap<String, (String, String)>>>> {
    let regs: Arc<Mutex<HashMap<String, (String, String)>>> = Arc::new(Mutex::new(HashMap::new()));

    // gather scripts: evaluate the local bootstrap script first, then
    // repository scripts which call `register(...)`.
    let mut scripts_vec: Vec<(String, String)> = Vec::new();
    scripts_vec.push((script_path.to_string(), local_script.to_string()));
    for (uri, content) in repository::fetch_scripts().into_iter() {
        scripts_vec.push((uri, content));
    }

    for (uri, script) in scripts_vec.into_iter() {
        // create a new runtime/context per script evaluation to keep things simple
        match Runtime::new() {
            Ok(rt) => {
                match Context::full(&rt) {
                    Ok(ctx) => {
                        if let Err(e) = ctx.with(|ctx| -> Result<(), rquickjs::Error> {
                            let global = ctx.globals();
                            let regs_cl = regs.clone();
                            let uri_cl = uri.clone();

                            // writeLog/listLogs host functions (use repository)
                            let write_fn = Function::new(
                                ctx.clone(),
                                |_ctx: rquickjs::Ctx<'_>,
                                 msg: String|
                                 -> Result<(), rquickjs::Error> {
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
                            let list_scripts_fn = Function::new(
                                ctx.clone(),
                                |_ctx: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                                    let map = repository::fetch_scripts();
                                    Ok(map.keys().cloned().collect())
                                },
                            )?;
                            global.set("listScripts", list_scripts_fn)?;

                            let register_fn = Function::new(
                                ctx.clone(),
                                move |_ctx: rquickjs::Ctx<'_>, path: String, handler_name: String|
                                      -> Result<(), rquickjs::Error> {
                                    if let Ok(mut guard) = regs_cl.lock() {
                                        guard.insert(path, (uri_cl.clone(), handler_name));
                                    }
                                    Ok(())
                                },
                            )?;
                            global.set("register", register_fn)?;
                            // evaluate script; ignore script failures but log them
                            ctx.eval::<(), _>(script.as_str())?;
                            Ok(())
                        }) {
                            eprintln!("failed to evaluate script for {}: {:?}", uri, e);
                        }
                    }
                    Err(e) => eprintln!("failed to create context for {}: {}", uri, e),
                }
            }
            Err(e) => eprintln!("failed to create runtime for {}: {}", uri, e),
        }
    }

    Ok(regs)
}

/// Start server on 0.0.0.0:4000 and load the JS script which can register paths.
pub async fn start_server_with_script(script_path: &str) -> anyhow::Result<()> {
    // read local bootstrap script
    let local = std::fs::read_to_string(script_path)?;

    // build registrations once by evaluating scripts; this avoids a long-lived
    // JS worker and keeps behavior simple and deterministic.
    let registrations = build_registrations(script_path, &local)?;

    // shared clone for request handlers
    let registrations = Arc::new(registrations);
    // in-memory cache of active/upserted scripts which should remain callable
    // even if repository.delete_script is called later (matches previous
    // behavior where upsert kept a loaded context alive).
    let active_scripts: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));

    // handler: per-request Context creation and execution with timeout
    let app = Router::new().route(
        "/{*path}",
        get(move |Path(path): Path<String>, req: Request<Body>| {
            let registrations = registrations.clone();
            async move {
                let path = if path.is_empty() {
                    "/".to_string()
                } else {
                    format!("/{}", path)
                };

                // look up registration
                let reg_opt = registrations
                    .lock()
                    .map(|g| g.get(&path).cloned())
                    .ok()
                    .flatten();
                let (owner_uri, handler_name) = match reg_opt {
                    Some(t) => t,
                    None => {
                        return (StatusCode::NOT_FOUND, "not found".to_string()).into_response();
                    }
                };

                // spawn blocking: create runtime + context, install host functions,
                // evaluate bootstrap + owner script and call handler
                let method = req.method().to_string();
                let owner_uri_cl = owner_uri.clone();
                let handler_name_cl = handler_name.clone();
                let path_log = path.clone();
                // clone registrations and active_scripts Arcs so host functions
                // inside the JS context can update them when upsertScript/deleteScript
                // are called.
                let regs_for_worker = registrations.clone();
                let active_for_worker = active_scripts.clone();
                let res = tokio::task::spawn_blocking(move || -> Result<(u16, String), String> {
                    // create runtime and context inside this thread
                    let rt = Runtime::new().map_err(|e| format!("runtime new: {}", e))?;
                    let ctx = Context::full(&rt).map_err(|e| format!("context create: {}", e))?;
                    // install host functions
                    ctx.with(|ctx| -> Result<(), rquickjs::Error> {
                        let global = ctx.globals();

                        // no-op register to allow owner scripts to call register()
                        let reg_noop = Function::new(
                            ctx.clone(),
                            |_ctx: rquickjs::Ctx<'_>, _path: String, _handler: String|
                            -> Result<(), rquickjs::Error> { Ok(()) },
                        )?;
                        global.set("register", reg_noop)?;

                        // writeLog
                        let write_fn = Function::new(
                            ctx.clone(),
                            |_ctx: rquickjs::Ctx<'_>, msg: String| -> Result<(), rquickjs::Error> {
                                repository::insert_log_message(&msg);
                                Ok(())
                            },
                        )?;
                        global.set("writeLog", write_fn)?;

                        // listLogs
                        let list_fn = Function::new(
                            ctx.clone(),
                            |_ctx: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                                Ok(repository::fetch_log_messages())
                            },
                        )?;
                        global.set("listLogs", list_fn)?;

                        // listScripts
                        let list_scripts_fn = Function::new(
                            ctx.clone(),
                            |_ctx: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                                let map = repository::fetch_scripts();
                                Ok(map.keys().cloned().collect())
                            },
                        )?;
                        global.set("listScripts", list_scripts_fn)?;

                        // getScript
                        let get_fn = Function::new(
                            ctx.clone(),
                            |_ctx: rquickjs::Ctx<'_>, uri: String|
                            -> Result<Option<String>, rquickjs::Error> { Ok(repository::fetch_script(&uri)) },
                        )?;
                        global.set("getScript", get_fn)?;

                        // deleteScript: delete from repository dynamic store and
                        // remove registrations that point to the URI immediately.
                        let regs_del = regs_for_worker.clone();
                        let delete_fn = Function::new(
                            ctx.clone(),
                            move |_ctx: rquickjs::Ctx<'_>, uri: String|
                            -> Result<bool, rquickjs::Error> {
                                let existed = repository::delete_script(&uri);
                                if let Ok(mut guard) = regs_del.lock() {
                                    guard.retain(|_k, v| v.0 != uri);
                                }
                                Ok(existed)
                            },
                        )?;
                        global.set("deleteScript", delete_fn)?;

                        // upsertScript: persist script, insert into active cache and
                        // collect registrations by evaluating it in a transient ctx.
                        let regs_upsert = regs_for_worker.clone();
                        let active_upsert = active_for_worker.clone();
                        let upsert_fn = Function::new(
                            ctx.clone(),
                            move |_ctx: rquickjs::Ctx<'_>, uri: String, content: String|
                            -> Result<(), rquickjs::Error> {
                                repository::upsert_script(&uri, &content);
                                if let Ok(mut a) = active_upsert.lock() {
                                    a.insert(uri.clone(), content.clone());
                                }

                                // evaluate new script in a transient context to collect
                                // registrations
                                if let Ok(rt2) = Runtime::new() {
                                    if let Ok(ctx2) = Context::full(&rt2) {
                                        let collected = std::sync::Arc::new(Mutex::new(Vec::<(String, String)>::new()));
                                        let coll = collected.clone();
                                        let _ = ctx2.with(|inner_ctx| -> Result<(), rquickjs::Error> {
                                            let reg_fn = Function::new(
                                                inner_ctx.clone(),
                                                move |_c: rquickjs::Ctx<'_>, p: String, h: String|
                                                -> Result<(), rquickjs::Error> {
                                                    if let Ok(mut g) = coll.lock() {
                                                        g.push((p, h));
                                                    }
                                                    Ok(())
                                                },
                                            )?;
                                            inner_ctx.globals().set("register", reg_fn)?;
                                            let _ = inner_ctx.eval::<(), _>(content.as_str());
                                            Ok(())
                                        });
                                        // commit collected registrations
                                        if let Ok(col) = collected.lock() {
                                            if let Ok(mut guard) = regs_upsert.lock() {
                                                guard.retain(|_k, v| v.0 != uri);
                                                for (p, h) in col.iter() {
                                                    guard.insert(p.clone(), (uri.clone(), h.clone()));
                                                }
                                            }
                                        }
                                    }
                                }
                                Ok(())
                            },
                        )?;
                        global.set("upsertScript", upsert_fn)?;

                        Ok(())
                    })
                    .map_err(|e| format!("install host fns: {}", e))?;

                    // fetch owner script
                    // check active scripts cache first (upserted scripts)
                    let owner_script = if let Ok(active) = active_for_worker.lock() {
                        if let Some(s) = active.get(&owner_uri_cl) {
                            s.clone()
                        } else {
                            match repository::fetch_script(&owner_uri_cl) {
                                Some(s) => s,
                                None => {
                                    // try reading local file path equal to the URI (useful for tests)
                                    if let Ok(s) = std::fs::read_to_string(&owner_uri_cl) {
                                        s
                                    } else {
                                        return Err(format!("no script for uri {}", owner_uri_cl));
                                    }
                                }
                            }
                        }
                    } else {
                        // lock failed: fallback to repository/FS
                        match repository::fetch_script(&owner_uri_cl) {
                            Some(s) => s,
                            None => {
                                if let Ok(s) = std::fs::read_to_string(&owner_uri_cl) {
                                    s
                                } else {
                                    return Err(format!("no script for uri {}", owner_uri_cl));
                                }
                            }
                        }
                    };
                    ctx.with(|ctx| ctx.eval::<(), _>(owner_script.as_str()))
                        .map_err(|e| format!("owner eval: {}", e))?;

                    // call handler by name and extract status/body in the same ctx.with
                    let (status, body) = ctx.with(|ctx| -> Result<(u16, String), String> {
                        let global = ctx.globals();
                        let func: Function = global
                            .get::<_, Function>(handler_name_cl.clone())
                            .map_err(|e| format!("no handler {}: {}", handler_name_cl, e))?;
                        let req_obj = rquickjs::Object::new(ctx)
                            .and_then(|o| o.set("method", method.clone()).map(|_| o))
                            .map_err(|e| format!("make req obj: {}", e))?;
                        let val = func
                            .call::<_, Value>((path.clone(), req_obj))
                            .map_err(|e| format!("call error: {}", e))?;
                        let obj = val
                            .as_object()
                            .ok_or_else(|| "expected object".to_string())?;
                        let status: i32 = obj
                            .get("status")
                            .map_err(|e| format!("missing status: {}", e))?;
                        let body: String = obj
                            .get("body")
                            .map_err(|e| format!("missing body: {}", e))?;
                        Ok((status as u16, body))
                    })?;

                    Ok((status, body))
                })
                .await
                .map_err(|e| format!("join error: {}", e));

                // enforce timeout on the blocking task
                let timed =
                    match tokio::time::timeout(std::time::Duration::from_millis(2000), async {
                        res
                    })
                    .await
                    {
                        Ok(r) => r,
                        Err(_) => {
                            return (StatusCode::GATEWAY_TIMEOUT, "script timeout".to_string())
                                .into_response();
                        }
                    };

                match timed {
                    Ok(Ok((status, body))) => (
                        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                        body,
                    )
                        .into_response(),
                    Ok(Err(e)) => {
                        eprintln!("script error for {}: {}", path_log, e);
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("script error: {}", e),
                        )
                            .into_response()
                    }
                    Err(e) => {
                        eprintln!("task error for {}: {}", path_log, e);
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("task error: {}", e),
                        )
                            .into_response()
                    }
                }
            }
        }),
    );

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 4000));
    println!("listening on {}", addr);
    axum_server::bind(addr)
        .serve(app.into_make_service())
        .await?;
    Ok(())
}
