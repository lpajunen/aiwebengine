use axum::body::Body;
use axum::http::Request;
use axum::http::StatusCode;
use axum::{Router, routing::get};
use axum::{extract::Path, response::IntoResponse};
use axum_server::Server;
use rquickjs::{Context, Function, Runtime, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub mod repository;

/// Simple health handler used by tests.
pub async fn root() -> (StatusCode, &'static str) {
    (StatusCode::OK, "Hello from axum!")
}

pub fn app() -> Router {
    Router::new().route("/", get(root))
}

fn build_registrations() -> anyhow::Result<Arc<Mutex<HashMap<String, (String, String)>>>> {
    let regs = Arc::new(Mutex::new(HashMap::new()));

    let scripts = repository::fetch_scripts();

    for (uri, content) in scripts.into_iter() {
        if let Ok(rt) = Runtime::new() {
            if let Ok(ctx) = Context::full(&rt) {
                let regs_cl = regs.clone();
                let uri_cl = uri.clone();
                let _ = ctx.with(|ctx| -> Result<(), rquickjs::Error> {
                    let global = ctx.globals();
                    let register = Function::new(
                        ctx.clone(),
                        move |_c: rquickjs::Ctx<'_>,
                              path: String,
                              handler: String|
                              -> Result<(), rquickjs::Error> {
                            if let Ok(mut g) = regs_cl.lock() {
                                g.insert(path, (uri_cl.clone(), handler));
                            }
                            Ok(())
                        },
                    )?;
                    global.set("register", register)?;
                    let _ = ctx.eval::<(), _>(content.as_str());
                    Ok(())
                });
            }
        }
    }

    Ok(regs)
}

pub async fn start_server(
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    let registrations = Arc::new(build_registrations()?);
    let active_scripts: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));

    let app = Router::new().route(
        "/{*path}",
        get(move |Path(path): Path<String>, req: Request<Body>| {
            let regs = registrations.clone();
            let active = active_scripts.clone();
            async move {
                let path = if path.is_empty() { "/".to_string() } else { format!("/{}", path) };
                let reg = regs.lock().ok().and_then(|g| g.get(&path).cloned());
                let (owner_uri, handler_name) = match reg { Some(t) => t, None => return (StatusCode::NOT_FOUND, "not found".to_string()).into_response() };

                let method = req.method().to_string();
                let owner_uri_cl = owner_uri.clone();
                let handler_cl = handler_name.clone();
                let path_log = path.clone();
                let regs_for_worker = regs.clone();
                let active_for_worker = active.clone();

                let worker = move || -> Result<(u16, String), String> {
                    let rt = Runtime::new().map_err(|e| format!("runtime new: {}", e))?;
                    let ctx = Context::full(&rt).map_err(|e| format!("context create: {}", e))?;

                    ctx.with(|ctx| -> Result<(), rquickjs::Error> {
                        let global = ctx.globals();

                        let reg_noop = Function::new(ctx.clone(), |_c: rquickjs::Ctx<'_>, _p: String, _h: String| -> Result<(), rquickjs::Error> { Ok(()) })?;
                        global.set("register", reg_noop)?;

                        let write = Function::new(ctx.clone(), |_c: rquickjs::Ctx<'_>, msg: String| -> Result<(), rquickjs::Error> {
                            repository::insert_log_message(&msg);
                            Ok(())
                        })?;
                        global.set("writeLog", write)?;

                        let list_logs = Function::new(ctx.clone(), |_c: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                            Ok(repository::fetch_log_messages())
                        })?;
                        global.set("listLogs", list_logs)?;

                        let list_scripts = Function::new(ctx.clone(), |_c: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                            let m = repository::fetch_scripts();
                            Ok(m.keys().cloned().collect())
                        })?;
                        global.set("listScripts", list_scripts)?;

                        let get_script = Function::new(ctx.clone(), |_c: rquickjs::Ctx<'_>, uri: String| -> Result<Option<String>, rquickjs::Error> {
                            Ok(repository::fetch_script(&uri))
                        })?;
                        global.set("getScript", get_script)?;

                        let regs_del = regs_for_worker.clone();
                        let delete_fn = Function::new(ctx.clone(), move |_c: rquickjs::Ctx<'_>, uri: String| -> Result<bool, rquickjs::Error> {
                            let existed = repository::delete_script(&uri);
                            if let Ok(mut g) = regs_del.lock() { g.retain(|_k, v| v.0 != uri); }
                            Ok(existed)
                        })?;
                        global.set("deleteScript", delete_fn)?;

                        let regs_up = regs_for_worker.clone();
                        let active_up = active_for_worker.clone();
                        let upsert = Function::new(ctx.clone(), move |_c: rquickjs::Ctx<'_>, uri: String, content: String| -> Result<(), rquickjs::Error> {
                            repository::upsert_script(&uri, &content);
                            if let Ok(mut a) = active_up.lock() { a.insert(uri.clone(), content.clone()); }

                            // transiently evaluate new script to collect registrations
                            if let Ok(rt2) = Runtime::new() {
                                if let Ok(ctx2) = Context::full(&rt2) {
                                    let collected = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
                                    let coll = collected.clone();
                                    let _ = ctx2.with(|inner| -> Result<(), rquickjs::Error> {
                                        let reg = Function::new(inner.clone(), move |_c: rquickjs::Ctx<'_>, p: String, h: String| -> Result<(), rquickjs::Error> {
                                            if let Ok(mut v) = coll.lock() { v.push((p, h)); }
                                            Ok(())
                                        })?;
                                        inner.globals().set("register", reg)?;
                                        let _ = inner.eval::<(), _>(content.as_str());
                                        Ok(())
                                    });

                                    if let Ok(col) = collected.lock() {
                                        if let Ok(mut g) = regs_up.lock() {
                                            g.retain(|_k, v| v.0 != uri);
                                            for (p, h) in col.iter() { g.insert(p.clone(), (uri.clone(), h.clone())); }
                                        }
                                    }
                                }
                            }

                            Ok(())
                        })?;
                        global.set("upsertScript", upsert)?;

                        Ok(())
                    }).map_err(|e| format!("install host fns: {}", e))?;

                    let owner_script = if let Ok(active) = active_for_worker.lock() {
                        if let Some(s) = active.get(&owner_uri_cl) {
                            s.clone()
                        } else if let Some(s) = repository::fetch_script(&owner_uri_cl) {
                            s
                        } else {
                            return Err(format!("no script for uri {}", owner_uri_cl));
                        }
                    } else {
                        if let Some(s) = repository::fetch_script(&owner_uri_cl) {
                            s
                        } else {
                            return Err(format!("no script for uri {}", owner_uri_cl));
                        }
                    };

                    ctx.with(|ctx| ctx.eval::<(), _>(owner_script.as_str())).map_err(|e| format!("owner eval: {}", e))?;

                    let (status, body) = ctx.with(|ctx| -> Result<(u16, String), String> {
                        let global = ctx.globals();
                        let func: Function = global.get::<_, Function>(handler_cl.clone()).map_err(|e| format!("no handler {}: {}", handler_cl, e))?;
                        let req_obj = rquickjs::Object::new(ctx).and_then(|o| o.set("method", method.clone()).map(|_| o)).map_err(|e| format!("make req obj: {}", e))?;
                        let val = func.call::<_, Value>((path.clone(), req_obj)).map_err(|e| format!("call error: {}", e))?;
                        let obj = val.as_object().ok_or_else(|| "expected object".to_string())?;
                        let status: i32 = obj.get("status").map_err(|e| format!("missing status: {}", e))?;
                        let body: String = obj.get("body").map_err(|e| format!("missing body: {}", e))?;
                        Ok((status as u16, body))
                    })?;

                    Ok((status, body))
                };

                let join = tokio::task::spawn_blocking(worker).await.map_err(|e| format!("join error: {}", e));

                let timed = match tokio::time::timeout(std::time::Duration::from_millis(2000), async { join }).await {
                    Ok(r) => r,
                    Err(_) => return (StatusCode::GATEWAY_TIMEOUT, "script timeout".to_string()).into_response(),
                };

                match timed {
                    Ok(Ok((status, body))) => (StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), body).into_response(),
                    Ok(Err(e)) => { eprintln!("script error for {}: {}", path_log, e); (StatusCode::INTERNAL_SERVER_ERROR, format!("script error: {}", e)).into_response() },
                    Err(e) => { eprintln!("task error for {}: {}", path_log, e); (StatusCode::INTERNAL_SERVER_ERROR, format!("task error: {}", e)).into_response() }
                }
            }
        }),
    );

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 4000));
    // record startup in logs so tests can observe server start
    repository::insert_log_message("server started");
    println!("listening on {}", addr);
    let svc = app.into_make_service();
    let server = Server::bind(addr).serve(svc);

    tokio::select! {
        res = server => { res? },
        _ = &mut shutdown_rx => { /* graceful shutdown: stop accepting new connections */ }
    }

    Ok(())
}

pub async fn start_server_without_shutdown() -> anyhow::Result<()> {
    let (_tx, rx) = tokio::sync::oneshot::channel::<()>();
    start_server(rx).await
}
