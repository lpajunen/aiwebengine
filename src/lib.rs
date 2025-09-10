use axum::body::Body;
use axum::http::Request;
use axum::http::StatusCode;
use axum::{Router, routing::get};
use axum::{extract::Path, response::IntoResponse};
use rquickjs::Value;
use rquickjs::{Context, Function, Runtime};
use std::sync::{Arc, mpsc};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

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
            Ok(r) => std::rc::Rc::new(r),
            Err(e) => {
                eprintln!("js runtime init error: {}", e);
                return;
            }
        };
        // registrations: path -> (uri, handler_name)
        let registrations: Rc<RefCell<HashMap<String, (String, String)>>> =
            Rc::new(RefCell::new(HashMap::new()));

        // contexts: uri -> Context
        let contexts: Rc<RefCell<HashMap<String, Context>>> = Rc::new(RefCell::new(HashMap::new()));

        // upsert job channel: host functions send (uri, content) here and the
        // worker thread processes them outside of the JS callback to avoid
        // nested QuickJS context borrows.
        let (upsert_tx, upsert_rx) = std::sync::mpsc::channel::<(String, String)>();

        // create a Context per script and evaluate it
        for (uri, script) in scripts.iter() {
            let uri = uri.clone();
            let ctx_for_script = match Context::full(rt.as_ref()) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("js context error for {}: {}", uri, e);
                    return;
                }
            };
            if let Err(e) = ctx_for_script.with(|ctx| -> Result<(), rquickjs::Error> {
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

                // register(path, handlerName) host function records mapping with this uri
                let regs = registrations.clone();
                let uri_for_reg = uri.clone();
                let register_fn = Function::new(
                    ctx.clone(),
                    move |_ctx: rquickjs::Ctx<'_>,
                          path: String,
                          handler_name: String|
                          -> Result<(), rquickjs::Error> {
                        regs.borrow_mut()
                            .insert(path, (uri_for_reg.clone(), handler_name));
                        Ok(())
                    },
                )?;
                global.set("register", register_fn)?;

                // upsertScript enqueues a job; the worker will process jobs from
                // `upsert_rx` later in the main loop.
                let upsert_tx_cl = upsert_tx.clone();
                let upsert_fn = Function::new(
                    ctx.clone(),
                    move |_ctx: rquickjs::Ctx<'_>,
                          uri: String,
                          content: String|
                          -> Result<(), rquickjs::Error> {
                        if let Err(e) = upsert_tx_cl.send((uri, content)) {
                            eprintln!("failed to enqueue upsert job: {:?}", e);
                        }
                        Ok(())
                    },
                )?;
                global.set("upsertScript", upsert_fn)?;

                let get_fn = Function::new(
                    ctx.clone(),
                    |_ctx: rquickjs::Ctx<'_>,
                     uri: String|
                     -> Result<Option<String>, rquickjs::Error> {
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
                eprintln!("failed to install host functions for {}: {:?}", uri, e);
                return;
            }

            // Evaluate the script in its own context
            if let Err(e) = ctx_for_script.with(|ctx| ctx.eval::<(), _>(script.as_str())) {
                eprintln!(
                    "script eval error for {}: {:?}\n--- script start ---\n{}\n--- script end ---",
                    uri, e, script
                );
                return;
            }

            // store the context for later invocation
            contexts.borrow_mut().insert(uri.clone(), ctx_for_script);
        }

        // dispatch loop: look up registration and invoke handler in owning context
        while let Ok(req) = rx.recv() {
            // process pending upsert jobs first
            while let Ok((u_uri, u_content)) = upsert_rx.try_recv() {
                // evaluate new script in a fresh context and commit registrations
                let new_ctx = match Context::full(rt.as_ref()) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("failed to create context for upsert {}: {}", u_uri, e);
                        continue;
                    }
                };
                let collected: Rc<RefCell<Vec<(String, String)>>> =
                    Rc::new(RefCell::new(Vec::new()));
                let coll = collected.clone();
                if let Err(e) = new_ctx.with(|inner_ctx| -> Result<(), rquickjs::Error> {
                    let global = inner_ctx.globals();
                    let write_fn = Function::new(
                        inner_ctx.clone(),
                        |_ctx: rquickjs::Ctx<'_>, msg: String| -> Result<(), rquickjs::Error> {
                            repository::insert_log_message(&msg);
                            Ok(())
                        },
                    )?;
                    global.set("writeLog", write_fn)?;
                    let list_fn = Function::new(
                        inner_ctx.clone(),
                        |_ctx: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                            Ok(repository::fetch_log_messages())
                        },
                    )?;
                    global.set("listLogs", list_fn)?;
                    let reg_fn = Function::new(
                        inner_ctx.clone(),
                        move |_ctx: rquickjs::Ctx<'_>,
                              path: String,
                              handler_name: String|
                              -> Result<(), rquickjs::Error> {
                            coll.borrow_mut().push((path, handler_name));
                            Ok(())
                        },
                    )?;
                    global.set("register", reg_fn)?;
                    inner_ctx.eval::<(), _>(u_content.as_str())?;
                    Ok(())
                }) {
                    eprintln!("error evaluating upsert {}: {:?}", u_uri, e);
                    continue;
                }
                // commit collected registrations and store context
                {
                    let mut regs_mut = registrations.borrow_mut();
                    regs_mut.retain(|_k, v| v.0 != u_uri);
                    for (path, handler_name) in collected.borrow().iter() {
                        regs_mut.insert(path.clone(), (u_uri.clone(), handler_name.clone()));
                    }
                }
                contexts.borrow_mut().insert(u_uri.clone(), new_ctx);
                repository::upsert_script(&u_uri, &u_content);
            }
            let registrations = registrations.clone();
            let contexts = contexts.clone();
            let reply = match registrations.borrow().get(&req.path).cloned() {
                Some((owner_uri, handler_name)) => {
                    // find owning context
                    let ctx_opt = contexts.borrow().get(&owner_uri).cloned();
                    match ctx_opt {
                        Some(ctx_obj) => {
                            // call handler_name in ctx_obj
                            ctx_obj.with(|ctx| {
                                let global = ctx.globals();
                                let func: Function = match global
                                    .get::<_, Function>(handler_name.clone())
                                {
                                    Ok(f) => f,
                                    Err(e) => {
                                        return Err(format!("no handler {}: {}", handler_name, e));
                                    }
                                };
                                let method = req.method.clone();
                                let res: Result<Value, rquickjs::Error> = func.call((
                                    req.path.clone(),
                                    rquickjs::Object::new(ctx)
                                        .and_then(|o| o.set("method", method.clone()).map(|_| o)),
                                ));
                                match res {
                                    Ok(val) => {
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
                            })
                        }
                        None => Err(format!("no context for uri {}", owner_uri)),
                    }
                }
                None => Err("not found".to_string()),
            };

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
