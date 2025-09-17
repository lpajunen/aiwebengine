use rquickjs::{Context, Function, Runtime, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use tracing::{debug, error};

use crate::repository;

/// Represents the result of executing a JavaScript script
#[derive(Debug, Clone)]
pub struct ScriptExecutionResult {
    /// The registrations made by the script via register() calls
    pub registrations: HashMap<(String, String), String>,
    /// Whether the script executed successfully
    pub success: bool,
    /// Error message if execution failed
    pub error: Option<String>,
}

/// Executes a JavaScript script and captures any register() method calls
///
/// This function creates a QuickJS runtime, sets up the register function,
/// executes the script, and returns information about the registrations made.
pub fn execute_script(uri: &str, content: &str) -> ScriptExecutionResult {
    let registrations = Rc::new(RefCell::new(HashMap::new()));
    let uri_owned = uri.to_string();

    match Runtime::new() {
        Ok(rt) => match Context::full(&rt) {
            Ok(ctx) => {
                let result = ctx.with(|ctx| -> Result<(), rquickjs::Error> {
                    let global = ctx.globals();

                    // Create the register function that captures registrations
                    let regs_clone = Rc::clone(&registrations);
                    let uri_clone = uri_owned.clone();
                    let register = Function::new(
                        ctx.clone(),
                        move |_c: rquickjs::Ctx<'_>,
                              path: String,
                              handler: String,
                              method: Option<String>|
                              -> Result<(), rquickjs::Error> {
                            let method = method.unwrap_or_else(|| "GET".to_string());
                            debug!(
                                "Registering route {} {} -> {} for script {}",
                                method, path, handler, uri_clone
                            );
                            if let Ok(mut regs) = regs_clone.try_borrow_mut() {
                                regs.insert((path, method), handler);
                            }
                            Ok(())
                        },
                    )?;

                    global.set("register", register)?;

                    // Execute the script
                    ctx.eval::<(), _>(content)?;

                    Ok(())
                });

                match result {
                    Ok(_) => {
                        debug!("Successfully executed script {}", uri_owned);
                        let final_regs = registrations.borrow().clone();
                        ScriptExecutionResult {
                            registrations: final_regs,
                            success: true,
                            error: None,
                        }
                    }
                    Err(e) => {
                        error!("Failed to execute script {}: {}", uri_owned, e);
                        ScriptExecutionResult {
                            registrations: HashMap::new(),
                            success: false,
                            error: Some(format!("Script evaluation error: {}", e)),
                        }
                    }
                }
            }
            Err(e) => {
                error!(
                    "Failed to create QuickJS context for script {}: {}",
                    uri_owned, e
                );
                ScriptExecutionResult {
                    registrations: HashMap::new(),
                    success: false,
                    error: Some(format!("Context creation error: {}", e)),
                }
            }
        },
        Err(e) => {
            error!(
                "Failed to create QuickJS runtime for script {}: {}",
                uri_owned, e
            );
            ScriptExecutionResult {
                registrations: HashMap::new(),
                success: false,
                error: Some(format!("Runtime creation error: {}", e)),
            }
        }
    }
}

/// Executes a JavaScript script for an HTTP request
///
/// This function creates a QuickJS runtime, sets up host functions,
/// executes the script, calls the specified handler with request parameters,
/// and returns the response.
pub fn execute_script_for_request(
    script_uri: &str,
    handler_name: &str,
    path: &str,
    method: &str,
    query_params: Option<&std::collections::HashMap<String, String>>,
    form_data: Option<&std::collections::HashMap<String, String>>,
    raw_body: Option<String>,
) -> Result<(u16, String, Option<String>), String> {
    let rt = Runtime::new().map_err(|e| format!("runtime new: {}", e))?;
    let ctx = Context::full(&rt).map_err(|e| format!("context create: {}", e))?;

    ctx.with(|ctx| -> Result<(), rquickjs::Error> {
        let global = ctx.globals();

        // Set up host functions
        let reg_noop = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>, _p: String, _h: String| -> Result<(), rquickjs::Error> {
                Ok(())
            },
        )?;
        global.set("register", reg_noop)?;

        let write = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>, msg: String| -> Result<(), rquickjs::Error> {
                debug!("JavaScript called writeLog with message: {}", msg);
                repository::insert_log_message(&msg);
                Ok(())
            },
        )?;
        global.set("writeLog", write)?;

        let list_logs = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                debug!("JavaScript called listLogs");
                Ok(repository::fetch_log_messages())
            },
        )?;
        global.set("listLogs", list_logs)?;

        let list_scripts = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                debug!("JavaScript called listScripts");
                let m = repository::fetch_scripts();
                Ok(m.keys().cloned().collect())
            },
        )?;
        global.set("listScripts", list_scripts)?;

        let list_assets = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                debug!("JavaScript called listAssets");
                let m = repository::fetch_assets();
                Ok(m.keys().cloned().collect())
            },
        )?;
        global.set("listAssets", list_assets)?;

        let fetch_asset = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>, public_path: String| -> Result<String, rquickjs::Error> {
                debug!("JavaScript called fetchAsset with public_path: {}", public_path);
                if let Some(asset) = repository::fetch_asset(&public_path) {
                    let content_b64 = base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        &asset.content,
                    );
                    let asset_json = serde_json::json!({
                        "publicPath": asset.public_path,
                        "mimetype": asset.mimetype,
                        "content": content_b64
                    });
                    Ok(asset_json.to_string())
                } else {
                    Ok("null".to_string())
                }
            },
        )?;
        global.set("fetchAsset", fetch_asset)?;

        let upsert_asset = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>,
             public_path: String,
             mimetype: String,
             content_b64: String|
             -> Result<(), rquickjs::Error> {
                debug!("JavaScript called upsertAsset with public_path: {}, mimetype: {}, content_b64 length: {}", 
                       public_path, mimetype, content_b64.len());
                match base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    &content_b64,
                ) {
                    Ok(content) => {
                        let asset = repository::Asset {
                            public_path,
                            mimetype,
                            content,
                        };
                        repository::upsert_asset(asset);
                        Ok(())
                    }
                    Err(_) => Err(rquickjs::Error::Exception),
                }
            },
        )?;
        global.set("upsertAsset", upsert_asset)?;

        let delete_asset = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>, public_path: String| -> Result<bool, rquickjs::Error> {
                debug!("JavaScript called deleteAsset with public_path: {}", public_path);
                Ok(repository::delete_asset(&public_path))
            },
        )?;
        global.set("deleteAsset", delete_asset)?;

        let get_script = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>, uri: String| -> Result<String, rquickjs::Error> {
                debug!("JavaScript called getScript with uri: {}", uri);
                match repository::fetch_script(&uri) {
                    Some(content) => Ok(content),
                    None => Ok("".to_string()),
                }
            },
        )?;
        global.set("getScript", get_script)?;

        let upsert_script = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>, uri: String, content: String| -> Result<(), rquickjs::Error> {
                debug!("JavaScript called upsertScript with uri: {}, content length: {}", uri, content.len());
                repository::upsert_script(&uri, &content);
                Ok(())
            },
        )?;
        global.set("upsertScript", upsert_script)?;

        let delete_script = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>, uri: String| -> Result<bool, rquickjs::Error> {
                debug!("JavaScript called deleteScript with uri: {}", uri);
                Ok(repository::delete_script(&uri))
            },
        )?;
        global.set("deleteScript", delete_script)?;

        Ok(())
    })
    .map_err(|e| format!("install host fns: {}", e))?;

    let owner_script = repository::fetch_script(script_uri)
        .ok_or_else(|| format!("no script for uri {}", script_uri))?;

    ctx.with(|ctx| ctx.eval::<(), _>(owner_script.as_str()))
        .map_err(|e| format!("owner eval: {}", e))?;

    let (status, body, content_type) =
        ctx.with(|ctx| -> Result<(u16, String, Option<String>), String> {
            let global = ctx.globals();
            let func: Function = global
                .get::<_, Function>(handler_name)
                .map_err(|e| format!("no handler {}: {}", handler_name, e))?;

            let req_obj =
                rquickjs::Object::new(ctx.clone()).map_err(|e| format!("make req obj: {}", e))?;

            req_obj
                .set("method", method)
                .map_err(|e| format!("set method: {}", e))?;

            req_obj
                .set("path", path)
                .map_err(|e| format!("set path: {}", e))?;

            if let Some(qp) = query_params {
                let query_obj = rquickjs::Object::new(ctx.clone())
                    .map_err(|e| format!("make query obj: {}", e))?;
                for (key, value) in qp {
                    query_obj
                        .set(key, value)
                        .map_err(|e| format!("set query param {}: {}", key, e))?;
                }
                req_obj
                    .set("query", query_obj)
                    .map_err(|e| format!("set query: {}", e))?;
            }

            if let Some(fd) = form_data {
                let form_obj = rquickjs::Object::new(ctx.clone())
                    .map_err(|e| format!("make form obj: {}", e))?;
                for (key, value) in fd {
                    form_obj
                        .set(key, value)
                        .map_err(|e| format!("set form param {}: {}", key, e))?;
                }
                req_obj
                    .set("form", form_obj)
                    .map_err(|e| format!("set form: {}", e))?;
            }

            if let Some(rb) = raw_body {
                req_obj
                    .set("body", rb)
                    .map_err(|e| format!("set body: {}", e))?;
            }

            let val = func
                .call::<_, Value>((req_obj,))
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

            // Extract optional contentType field
            let content_type: Option<String> = obj.get("contentType").ok(); // This will be None if the field doesn't exist

            Ok((status as u16, body, content_type))
        })?;

    Ok((status, body, content_type))
}
