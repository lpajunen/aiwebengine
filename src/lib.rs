use axum::body::Body;
use axum::http::Request;
use axum::http::StatusCode;
use axum::{
    Router,
    routing::{any, get},
};
use axum::{extract::Path, response::IntoResponse};
use axum_server::Server;
use rquickjs::{Context, Function, Runtime, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub mod config;
pub mod repository;

/// Type alias for route registrations: (path, method) -> (script_uri, handler_name)
type RouteRegistry = Arc<Mutex<HashMap<(String, String), (String, String)>>>;

/// Simple health handler used by tests.
pub async fn root() -> (StatusCode, &'static str) {
    (StatusCode::OK, "Hello from axum!")
}

pub fn app() -> Router {
    Router::new().route("/", get(root))
}

/// Builds the route registry by loading all scripts and collecting their route registrations.
///
/// This function creates a QuickJS runtime for each script, evaluates it, and collects
/// any routes registered via the `register(path, handler)` function exposed to JavaScript.
fn build_registrations() -> anyhow::Result<RouteRegistry> {
    let regs = Arc::new(Mutex::new(HashMap::new()));

    let scripts = repository::fetch_scripts();
    println!("DEBUG: Found {} scripts to load", scripts.len());

    for (uri, content) in scripts.into_iter() {
        println!("DEBUG: Loading script {}", uri);
        if let Ok(rt) = Runtime::new() {
            if let Ok(ctx) = Context::full(&rt) {
                let regs_cl = regs.clone();
                let uri_cl = uri.clone();
                match ctx.with(|ctx| -> Result<(), rquickjs::Error> {
                    let global = ctx.globals();
                    let register = Function::new(
                        ctx.clone(),
                        move |_c: rquickjs::Ctx<'_>,
                              path: String,
                              handler: String,
                              method: Option<String>|
                              -> Result<(), rquickjs::Error> {
                            let method = method.unwrap_or_else(|| "GET".to_string());
                            println!(
                                "DEBUG: Registering route {} {} -> {} for script {}",
                                method, path, handler, uri_cl
                            );
                            if let Ok(mut g) = regs_cl.lock() {
                                g.insert((path, method), (uri_cl.clone(), handler));
                            }
                            Ok(())
                        },
                    )?;
                    global.set("register", register)?;
                    ctx.eval::<(), _>(content.as_str())?;
                    Ok(())
                }) {
                    Ok(_) => println!("DEBUG: Successfully loaded script {}", uri),
                    Err(e) => eprintln!("Failed to evaluate script {}: {}", uri, e),
                }
            } else {
                eprintln!("Failed to create QuickJS context for script {}", uri);
            }
        } else {
            eprintln!("Failed to create QuickJS runtime for script {}", uri);
        }
    }

    // Debug: print all registered routes
    if let Ok(regs_locked) = regs.lock() {
        println!("DEBUG: Final route registry: {:?}", *regs_locked);
    }

    Ok(regs)
}

/// Starts the web server with the given shutdown receiver.
///
/// This function:
/// 1. Builds the route registry from all available scripts
/// 2. Sets up the Axum router with dynamic route handling
/// 3. Starts the server on the configured address
/// 4. Listens for shutdown signal
pub async fn start_server(shutdown_rx: tokio::sync::oneshot::Receiver<()>) -> anyhow::Result<()> {
    start_server_with_config(config::Config::from_env(), shutdown_rx).await
}

/// Starts the web server with custom configuration
pub async fn start_server_with_config(
    config: config::Config,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    let registrations = Arc::new(build_registrations()?);

    let registrations_clone = Arc::clone(&registrations);

    let app = Router::new()
        .route(
            "/",
            any(move |req: Request<Body>| {
                let regs = Arc::clone(&registrations);
                async move {
                    let path = "/";
                    let request_method = req.method().to_string();

                    // Check if any route exists for this path
                    let path_exists = regs
                        .lock()
                        .ok()
                        .map(|g| g.keys().any(|(p, _)| p == path))
                        .unwrap_or(false);

                    let reg = regs
                        .lock()
                        .ok()
                        .and_then(|g| g.get(&(path.to_string(), request_method.clone())).cloned());
                    let (owner_uri, handler_name) = match reg {
                        Some(t) => t,
                        None => {
                            if path_exists {
                                return (
                                    StatusCode::METHOD_NOT_ALLOWED,
                                    "method not allowed".to_string(),
                                )
                                    .into_response();
                            } else {
                                return (StatusCode::NOT_FOUND, "not found".to_string())
                                    .into_response();
                            }
                        }
                    };
                    let owner_uri_cl = owner_uri.clone();
                    let handler_cl = handler_name.clone();
                    let path_log = path.to_string();
                    let query_string = req.uri().query().map(|s| s.to_string());

                    let worker = move || -> Result<(u16, String), String> {
                        let rt = Runtime::new().map_err(|e| format!("runtime new: {}", e))?;
                        let ctx =
                            Context::full(&rt).map_err(|e| format!("context create: {}", e))?;

                        ctx.with(|ctx| -> Result<(), rquickjs::Error> {
                            let global = ctx.globals();

                            let reg_noop = Function::new(
                                ctx.clone(),
                                |_c: rquickjs::Ctx<'_>,
                                 _p: String,
                                 _h: String|
                                 -> Result<(), rquickjs::Error> {
                                    Ok(())
                                },
                            )?;
                            global.set("register", reg_noop)?;

                            let write = Function::new(
                                ctx.clone(),
                                |_c: rquickjs::Ctx<'_>,
                                 msg: String|
                                 -> Result<(), rquickjs::Error> {
                                    repository::insert_log_message(&msg);
                                    Ok(())
                                },
                            )?;
                            global.set("writeLog", write)?;

                            let list_logs = Function::new(
                                ctx.clone(),
                                |_c: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                                    Ok(repository::fetch_log_messages())
                                },
                            )?;
                            global.set("listLogs", list_logs)?;

                            let list_scripts = Function::new(
                                ctx.clone(),
                                |_c: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                                    let m = repository::fetch_scripts();
                                    Ok(m.keys().cloned().collect())
                                },
                            )?;
                            global.set("listScripts", list_scripts)?;

                            Ok(())
                        })
                        .map_err(|e| format!("install host fns: {}", e))?;

                        let owner_script = repository::fetch_script(&owner_uri_cl)
                            .ok_or_else(|| format!("no script for uri {}", owner_uri_cl))?;

                        ctx.with(|ctx| ctx.eval::<(), _>(owner_script.as_str()))
                            .map_err(|e| format!("owner eval: {}", e))?;

                        let (status, body) = ctx.with(|ctx| -> Result<(u16, String), String> {
                            let global = ctx.globals();
                            let func: Function = global
                                .get::<_, Function>(handler_cl.clone())
                                .map_err(|e| format!("no handler {}: {}", handler_cl, e))?;
                            let req_obj = rquickjs::Object::new(ctx)
                                .map_err(|e| format!("make req obj: {}", e))?;
                            req_obj
                                .set("method", request_method.clone())
                                .map_err(|e| format!("set method: {}", e))?;
                            if let Some(qs) = &query_string {
                                req_obj
                                    .set("query", qs.clone())
                                    .map_err(|e| format!("set query: {}", e))?;
                            }
                            let val = func
                                .call::<_, Value>((path.to_string(), req_obj))
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
                    };

                    let join = tokio::task::spawn_blocking(worker)
                        .await
                        .map_err(|e| format!("join error: {}", e));

                    let timed = match tokio::time::timeout(
                        std::time::Duration::from_millis(config.script_timeout_ms),
                        async { join },
                    )
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
                            StatusCode::from_u16(status)
                                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
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
        )
        .route(
            "/{*path}",
            any(move |Path(path): Path<String>, req: Request<Body>| {
                let regs = Arc::clone(&registrations_clone);
                async move {
                    let full_path = if path.is_empty() {
                        "/".to_string()
                    } else {
                        format!("/{}", path)
                    };
                    let request_method = req.method().to_string();

                    // Check if any route exists for this path
                    let path_exists = regs
                        .lock()
                        .ok()
                        .map(|g| g.keys().any(|(p, _)| p == &full_path))
                        .unwrap_or(false);

                    let reg = regs
                        .lock()
                        .ok()
                        .and_then(|g| g.get(&(full_path.clone(), request_method.clone())).cloned());
                    let (owner_uri, handler_name) = match reg {
                        Some(t) => t,
                        None => {
                            if path_exists {
                                return (
                                    StatusCode::METHOD_NOT_ALLOWED,
                                    "method not allowed".to_string(),
                                )
                                    .into_response();
                            } else {
                                return (StatusCode::NOT_FOUND, "not found".to_string())
                                    .into_response();
                            }
                        }
                    };
                    let owner_uri_cl = owner_uri.clone();
                    let handler_cl = handler_name.clone();
                    let path_log = full_path.clone();
                    let query_string = req.uri().query().map(|s| s.to_string());

                    let worker = move || -> Result<(u16, String), String> {
                        let rt = Runtime::new().map_err(|e| format!("runtime new: {}", e))?;
                        let ctx =
                            Context::full(&rt).map_err(|e| format!("context create: {}", e))?;

                        ctx.with(|ctx| -> Result<(), rquickjs::Error> {
                            let global = ctx.globals();

                            let reg_noop = Function::new(
                                ctx.clone(),
                                |_c: rquickjs::Ctx<'_>,
                                 _p: String,
                                 _h: String|
                                 -> Result<(), rquickjs::Error> {
                                    Ok(())
                                },
                            )?;
                            global.set("register", reg_noop)?;

                            let write = Function::new(
                                ctx.clone(),
                                |_c: rquickjs::Ctx<'_>,
                                 msg: String|
                                 -> Result<(), rquickjs::Error> {
                                    repository::insert_log_message(&msg);
                                    Ok(())
                                },
                            )?;
                            global.set("writeLog", write)?;

                            let list_logs = Function::new(
                                ctx.clone(),
                                |_c: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                                    Ok(repository::fetch_log_messages())
                                },
                            )?;
                            global.set("listLogs", list_logs)?;

                            let list_scripts = Function::new(
                                ctx.clone(),
                                |_c: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                                    let m = repository::fetch_scripts();
                                    Ok(m.keys().cloned().collect())
                                },
                            )?;
                            global.set("listScripts", list_scripts)?;

                            Ok(())
                        })
                        .map_err(|e| format!("install host fns: {}", e))?;

                        let owner_script = repository::fetch_script(&owner_uri_cl)
                            .ok_or_else(|| format!("no script for uri {}", owner_uri_cl))?;

                        ctx.with(|ctx| ctx.eval::<(), _>(owner_script.as_str()))
                            .map_err(|e| format!("owner eval: {}", e))?;

                        let (status, body) = ctx.with(|ctx| -> Result<(u16, String), String> {
                            let global = ctx.globals();
                            let func: Function = global
                                .get::<_, Function>(handler_cl.clone())
                                .map_err(|e| format!("no handler {}: {}", handler_cl, e))?;
                            let req_obj = rquickjs::Object::new(ctx)
                                .map_err(|e| format!("make req obj: {}", e))?;
                            req_obj
                                .set("method", request_method.clone())
                                .map_err(|e| format!("set method: {}", e))?;
                            if let Some(qs) = &query_string {
                                req_obj
                                    .set("query", qs.clone())
                                    .map_err(|e| format!("set query: {}", e))?;
                            }
                            let val = func
                                .call::<_, Value>((full_path.clone(), req_obj))
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
                    };

                    let join = tokio::task::spawn_blocking(worker)
                        .await
                        .map_err(|e| format!("join error: {}", e));

                    let timed = match tokio::time::timeout(
                        std::time::Duration::from_millis(config.script_timeout_ms),
                        async { join },
                    )
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
                            StatusCode::from_u16(status)
                                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
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

    let addr = config
        .server_addr()
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid server address: {}", e))?;

    // record startup in logs so tests can observe server start
    repository::insert_log_message("server started");
    println!("listening on {}", addr);
    println!(
        "DEBUG: Server configuration - host: {}, port: {}",
        config.host, config.port
    );
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
