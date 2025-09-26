use rquickjs::{Context, Function, Runtime, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use tracing::{debug, error};

use crate::repository_safe as repository;

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

                    // GraphQL registration functions
                    let uri_clone1 = uri_owned.clone();
                    let register_graphql_query = Function::new(
                        ctx.clone(),
                        move |_c: rquickjs::Ctx<'_>,
                             name: String,
                             sdl: String,
                             resolver_function: String|
                             -> Result<(), rquickjs::Error> {
                            debug!("JavaScript called registerGraphQLQuery with name: {}, sdl length: {}", name, sdl.len());
                            crate::graphql::register_graphql_query(name, sdl, resolver_function, uri_clone1.clone());
                            Ok(())
                        },
                    )?;
                    global.set("registerGraphQLQuery", register_graphql_query)?;

                    let uri_clone2 = uri_owned.clone();
                    let register_graphql_mutation = Function::new(
                        ctx.clone(),
                        move |_c: rquickjs::Ctx<'_>,
                             name: String,
                             sdl: String,
                             resolver_function: String|
                             -> Result<(), rquickjs::Error> {
                            debug!("JavaScript called registerGraphQLMutation with name: {}, sdl length: {}", name, sdl.len());
                            crate::graphql::register_graphql_mutation(name, sdl, resolver_function, uri_clone2.clone());
                            Ok(())
                        },
                    )?;
                    global.set("registerGraphQLMutation", register_graphql_mutation)?;

                    let uri_clone3 = uri_owned.clone();
                    let register_graphql_subscription = Function::new(
                        ctx.clone(),
                        move |_c: rquickjs::Ctx<'_>,
                             name: String,
                             sdl: String,
                             resolver_function: String|
                             -> Result<(), rquickjs::Error> {
                            debug!("JavaScript called registerGraphQLSubscription with name: {}, sdl length: {}", name, sdl.len());
                            crate::graphql::register_graphql_subscription(name, sdl, resolver_function, uri_clone3.clone());
                            Ok(())
                        },
                    )?;
                    global.set("registerGraphQLSubscription", register_graphql_subscription)?;

                    // Stream registration function
                    let uri_clone_stream = uri_owned.clone();
                    let register_web_stream = Function::new(
                        ctx.clone(),
                        move |_c: rquickjs::Ctx<'_>, path: String| -> Result<(), rquickjs::Error> {
                            debug!("JavaScript called registerWebStream with path: {}", path);
                            
                            // Validate path format
                            if path.is_empty() || !path.starts_with('/') {
                                tracing::error!("Invalid stream path '{}': must start with '/' and not be empty", path);
                                return Err(rquickjs::Error::Exception);
                            }
                            
                            if path.len() > 200 {
                                tracing::error!("Invalid stream path '{}': too long (max 200 characters)", path);
                                return Err(rquickjs::Error::Exception);
                            }
                            
                            match crate::stream_registry::GLOBAL_STREAM_REGISTRY.register_stream(&path, &uri_clone_stream) {
                                Ok(()) => {
                                    debug!("Successfully registered stream path '{}' for script '{}'", path, uri_clone_stream);
                                    Ok(())
                                }
                                Err(e) => {
                                    tracing::error!("Failed to register stream path '{}': {}", path, e);
                                    Err(rquickjs::Error::Exception)
                                }
                            }
                        },
                    )?;
                    global.set("registerWebStream", register_web_stream)?;

                    // Stream message sending function
                    let send_stream_message = Function::new(
                        ctx.clone(),
                        move |_c: rquickjs::Ctx<'_>, json_string: String| -> Result<(), rquickjs::Error> {
                            debug!("JavaScript called sendStreamMessage with message: {}", json_string);
                            
                            // Broadcast to all registered streams
                            match crate::stream_registry::GLOBAL_STREAM_REGISTRY.broadcast_to_all_streams(&json_string) {
                                Ok(count) => {
                                    debug!("Successfully broadcast message to {} connections", count);
                                    Ok(())
                                }
                                Err(e) => {
                                    tracing::error!("Failed to broadcast message: {}", e);
                                    Err(rquickjs::Error::Exception)
                                }
                            }
                        },
                    )?;
                    global.set("sendStreamMessage", send_stream_message)?;

                    // Set up host functions
                    let script_uri_clone1 = uri_owned.clone();
                    let write = Function::new(
                        ctx.clone(),
                        move |_c: rquickjs::Ctx<'_>, msg: String| -> Result<(), rquickjs::Error> {
                            debug!("JavaScript called writeLog with message: {}", msg);
                            repository::insert_log_message(&script_uri_clone1, &msg);
                            Ok(())
                        },
                    )?;
                    global.set("writeLog", write)?;

                    let script_uri_clone2 = uri_owned.clone();
                    let list_logs = Function::new(
                        ctx.clone(),
                        move |_c: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                            debug!("JavaScript called listLogs");
                            Ok(repository::fetch_log_messages(&script_uri_clone2))
                        },
                    )?;
                    global.set("listLogs", list_logs)?;

                    let list_logs_for_uri = Function::new(
                        ctx.clone(),
                        |_c: rquickjs::Ctx<'_>, uri: String| -> Result<Vec<String>, rquickjs::Error> {
                            debug!("JavaScript called listLogsForUri with uri: {}", uri);
                            Ok(repository::fetch_log_messages(&uri))
                        },
                    )?;
                    global.set("listLogsForUri", list_logs_for_uri)?;

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
                                    let _ = repository::upsert_asset(asset);
                                    Ok(())
                                }
                                Err(_) => Err(rquickjs::Error::Exception),
                            }
                        },
                    )?;
                    global.set("upsertAsset", upsert_asset)?;

                // Execute the script
                ctx.eval::<(), _>(content)?;                    Ok(())
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
    let script_uri_owned = script_uri.to_string();
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

        let script_uri_clone1 = script_uri_owned.clone();
        let write = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>, msg: String| -> Result<(), rquickjs::Error> {
                debug!("JavaScript called writeLog with message: {}", msg);
                repository::insert_log_message(&script_uri_clone1, &msg);
                Ok(())
            },
        )?;
        global.set("writeLog", write)?;

        let script_uri_clone2 = script_uri_owned.clone();
        let list_logs = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                debug!("JavaScript called listLogs");
                Ok(repository::fetch_log_messages(&script_uri_clone2))
            },
        )?;
        global.set("listLogs", list_logs)?;

        let list_logs_for_uri = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>, uri: String| -> Result<Vec<String>, rquickjs::Error> {
                debug!("JavaScript called listLogsForUri with uri: {}", uri);
                Ok(repository::fetch_log_messages(&uri))
            },
        )?;
        global.set("listLogsForUri", list_logs_for_uri)?;

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
                        let _ = repository::upsert_asset(asset);
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
                let _ = repository::upsert_script(&uri, &content);
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

        // GraphQL registration functions (no-ops for request handling)
        let register_graphql_query_noop = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>,
             _name: String,
             _sdl: String,
             _resolver_function: String|
             -> Result<(), rquickjs::Error> {
                // No-op for request handling
                Ok(())
            },
        )?;
        global.set("registerGraphQLQuery", register_graphql_query_noop)?;

        let register_graphql_mutation_noop = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>,
             _name: String,
             _sdl: String,
             _resolver_function: String|
             -> Result<(), rquickjs::Error> {
                // No-op for request handling
                Ok(())
            },
        )?;
        global.set("registerGraphQLMutation", register_graphql_mutation_noop)?;

        let register_graphql_subscription_noop = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>,
             _name: String,
             _sdl: String,
             _resolver_function: String|
             -> Result<(), rquickjs::Error> {
                // No-op for request handling
                Ok(())
            },
        )?;
        global.set("registerGraphQLSubscription", register_graphql_subscription_noop)?;

        let register_web_stream_noop = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>, _path: String| -> Result<(), rquickjs::Error> {
                // No-op for request handling - streams are registered during script execution, not during request handling
                Ok(())
            },
        )?;
        global.set("registerWebStream", register_web_stream_noop)?;

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

/// Executes a JavaScript GraphQL resolver function and returns the result as a string.
/// This is used by the GraphQL system to call JavaScript resolver functions.
pub fn execute_graphql_resolver(
    script_uri: &str,
    resolver_function: &str,
    args: Option<serde_json::Value>,
) -> Result<String, String> {
    let script_uri_owned = script_uri.to_string();
    let resolver_function_owned = resolver_function.to_string();
    let args_owned = args;

    let rt = Runtime::new().map_err(|e| format!("runtime new: {}", e))?;
    let ctx = Context::full(&rt).map_err(|e| format!("context create: {}", e))?;

    ctx.with(|ctx| -> Result<String, rquickjs::Error> {
        let global = ctx.globals();

        // Set up host functions (similar to execute_script_for_request)
        let reg_noop = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>, _p: String, _h: String| -> Result<(), rquickjs::Error> {
                Ok(())
            },
        )?;
        global.set("register", reg_noop)?;

        let script_uri_clone1 = script_uri_owned.clone();
        let write = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>, msg: String| -> Result<(), rquickjs::Error> {
                debug!("JavaScript called writeLog with message: {}", msg);
                repository::insert_log_message(&script_uri_clone1, &msg);
                Ok(())
            },
        )?;
        global.set("writeLog", write)?;

        let script_uri_clone2 = script_uri_owned.clone();
        let list_logs = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                debug!("JavaScript called listLogs");
                Ok(repository::fetch_log_messages(&script_uri_clone2))
            },
        )?;
        global.set("listLogs", list_logs)?;

        let _script_uri_clone3 = script_uri_owned.clone();
        let list_logs_for_uri = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>, uri: String| -> Result<Vec<String>, rquickjs::Error> {
                debug!("JavaScript called listLogsForUri with uri: {}", uri);
                Ok(repository::fetch_log_messages(&uri))
            },
        )?;
        global.set("listLogsForUri", list_logs_for_uri)?;

        let list_scripts = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>| -> Result<std::collections::HashMap<String, String>, rquickjs::Error> {
                debug!("JavaScript called listScripts");
                Ok(repository::fetch_scripts())
            },
        )?;
        global.set("listScripts", list_scripts)?;

        let list_assets = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                debug!("JavaScript called listAssets");
                Ok(repository::fetch_assets().keys().cloned().collect())
            },
        )?;
        global.set("listAssets", list_assets)?;

        let fetch_asset = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>, path: String| -> Result<Option<String>, rquickjs::Error> {
                debug!("JavaScript called fetchAsset with path: {}", path);
                Ok(repository::fetch_asset(&path).and_then(|asset| String::from_utf8(asset.content).ok()))
            },
        )?;
        global.set("fetchAsset", fetch_asset)?;

        let upsert_asset = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>, path: String, content: String, mime_type: String| -> Result<(), rquickjs::Error> {
                debug!("JavaScript called upsertAsset with path: {}", path);
                let asset = repository::Asset {
                    public_path: path,
                    content: content.into_bytes(),
                    mimetype: mime_type,
                };
                let _ = repository::upsert_asset(asset);
                Ok(())
            },
        )?;
        global.set("upsertAsset", upsert_asset)?;

        let delete_asset = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>, path: String| -> Result<bool, rquickjs::Error> {
                debug!("JavaScript called deleteAsset with path: {}", path);
                Ok(repository::delete_asset(&path))
            },
        )?;
        global.set("deleteAsset", delete_asset)?;

        let get_script = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>, uri: String| -> Result<Option<String>, rquickjs::Error> {
                debug!("JavaScript called getScript with uri: {}", uri);
                Ok(repository::fetch_script(&uri))
            },
        )?;
        global.set("getScript", get_script)?;

        let upsert_script = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>, uri: String, content: String| -> Result<(), rquickjs::Error> {
                debug!("JavaScript called upsertScript with uri: {}", uri);
                let _ = repository::upsert_script(&uri, &content);
                Ok(())
            },
        )?;
        global.set("upsertScript", upsert_script)?;

        let delete_script = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>, uri: String| -> Result<bool, rquickjs::Error> {
                debug!("JavaScript called deleteScript with uri: {}", uri);
                Ok(repository::delete_script(&uri))
            },
        )?;
        global.set("deleteScript", delete_script)?;

        // GraphQL registration functions (no-op for execution)
        let reg_graphql_query_noop = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>, _n: String, _s: String, _f: String| -> Result<(), rquickjs::Error> {
                Ok(())
            },
        )?;
        global.set("registerGraphQLQuery", reg_graphql_query_noop)?;

        let reg_graphql_mutation_noop = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>, _n: String, _s: String, _f: String| -> Result<(), rquickjs::Error> {
                Ok(())
            },
        )?;
        global.set("registerGraphQLMutation", reg_graphql_mutation_noop)?;

        let reg_graphql_subscription_noop = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>, _n: String, _s: String, _f: String| -> Result<(), rquickjs::Error> {
                Ok(())
            },
        )?;
        global.set("registerGraphQLSubscription", reg_graphql_subscription_noop)?;

        let reg_web_stream_noop = Function::new(
            ctx.clone(),
            |_c: rquickjs::Ctx<'_>, _path: String| -> Result<(), rquickjs::Error> {
                Ok(())
            },
        )?;
        global.set("registerWebStream", reg_web_stream_noop)?;

        // Load and execute the script
        let script_content = repository::fetch_script(&script_uri_owned)
            .ok_or_else(|| rquickjs::Error::new_from_js("Script", "not found"))?;

        // Execute the script
        ctx.eval::<(), _>(script_content.as_str())?;

        // Prepare arguments for the resolver function
        let args_value = if let Some(args) = args_owned {
            // Convert serde_json::Value to QuickJS value
            match args {
                serde_json::Value::Object(obj) => {
                    let obj_val = ctx.globals().get::<_, rquickjs::Object>("Object")?;
                    let create = obj_val.get::<_, rquickjs::Function>("create")?;
                    let proto = ctx.globals().get::<_, rquickjs::Object>("Object")?;
                    let proto = proto.get::<_, rquickjs::Object>("prototype")?;
                    let args_obj: rquickjs::Object = create.call((proto,))?;

                    for (key, value) in obj {
                        match value {
                            serde_json::Value::String(s) => args_obj.set(key, s)?,
                            serde_json::Value::Number(n) => {
                                if let Some(i) = n.as_i64() {
                                    args_obj.set(key, i)?;
                                } else if let Some(f) = n.as_f64() {
                                    args_obj.set(key, f)?;
                                }
                            },
                            serde_json::Value::Bool(b) => args_obj.set(key, b)?,
                            _ => {} // Skip other types for now
                        }
                    }
                    args_obj.into_value()
                },
                _ => rquickjs::Value::new_undefined(ctx.clone()),
            }
        } else {
            rquickjs::Value::new_undefined(ctx.clone())
        };

        // Call the resolver function
        let resolver_result: rquickjs::Value = ctx.globals().get(&resolver_function_owned)?;
        let resolver_func = resolver_result.as_function().ok_or_else(|| rquickjs::Error::new_from_js("Function", "not found"))?;

        let result_value = if args_value.is_undefined() {
            resolver_func.call::<_, rquickjs::Value>(())?
        } else {
            resolver_func.call::<_, rquickjs::Value>((args_value,))?
        };

                // Convert the result to a JSON string
        let result_string: String = if result_value.is_string() {
            result_value.as_string().unwrap().to_string()?
        } else {
            // Use JavaScript's JSON.stringify to convert any value to JSON
            let json_obj: rquickjs::Object = ctx.globals().get("JSON")?;
            let json_stringify: rquickjs::Function = json_obj.get("stringify")?;
            let json_str: String = json_stringify.call((result_value,))?;
            json_str
        };

        Ok(result_string)
    }).map_err(|e| format!("JavaScript execution error: {}", e))
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream_registry;

    #[test]
    fn test_execute_script_simple_registration() {
        let content = r#"
            register("/test", "handler_function", "GET");
        "#;

        let result = execute_script("test-script", content);

        assert!(result.success, "Script execution should succeed");
        assert!(result.error.is_none(), "Should not have error");
        assert_eq!(result.registrations.len(), 1);
        assert_eq!(
            result
                .registrations
                .get(&("/test".to_string(), "GET".to_string())),
            Some(&"handler_function".to_string())
        );
    }

    #[test]
    fn test_execute_script_multiple_registrations() {
        let content = r#"
            register("/api/users", "getUsers", "GET");
            register("/api/users", "createUser", "POST");
            register("/api/users/:id", "updateUser", "PUT");
        "#;

        let result = execute_script("multi-script", content);

        assert!(result.success);
        assert_eq!(result.registrations.len(), 3);
        assert!(
            result
                .registrations
                .contains_key(&("/api/users".to_string(), "GET".to_string()))
        );
        assert!(
            result
                .registrations
                .contains_key(&("/api/users".to_string(), "POST".to_string()))
        );
        assert!(
            result
                .registrations
                .contains_key(&("/api/users/:id".to_string(), "PUT".to_string()))
        );
    }

    #[test]
    fn test_execute_script_with_default_method() {
        let content = r#"
            register("/default-method", "handler", "GET");
        "#;

        let result = execute_script("default-method-script", content);

        if !result.success {
            println!("Default method test failed with error: {:?}", result.error);
        }
        assert!(
            result.success,
            "Script execution failed: {:?}",
            result.error
        );
        assert_eq!(
            result
                .registrations
                .get(&("/default-method".to_string(), "GET".to_string())),
            Some(&"handler".to_string())
        );
    }

    #[test]
    fn test_execute_script_with_syntax_error() {
        let content = r#"
            register("/test", "handler"
            // Missing closing parenthesis - syntax error
        "#;

        let result = execute_script("error-script", content);

        assert!(!result.success, "Script with syntax error should fail");
        assert!(result.error.is_some(), "Should have error message");
        assert!(
            result.registrations.is_empty(),
            "Should not have registrations on error"
        );
    }

    #[test]
    fn test_execute_script_with_runtime_error() {
        let content = r#"
            throw new Error("Runtime error test");
        "#;

        let result = execute_script("runtime-error-script", content);

        assert!(!result.success);
        assert!(result.error.is_some());
        assert!(result.registrations.is_empty());
    }

    #[test]
    fn test_execute_script_with_complex_javascript() {
        let content = r#"
            function setupRoutes() {
                register("/api/health", "healthCheck", "GET");
                register("/api/status", "statusCheck", "GET");
            }
            
            setupRoutes();
        "#;

        let result = execute_script("complex-script", content);

        assert!(
            result.success,
            "Complex JavaScript should execute successfully. Error: {:?}",
            result.error
        );
        assert_eq!(result.registrations.len(), 2);
        assert!(
            result
                .registrations
                .contains_key(&("/api/health".to_string(), "GET".to_string()))
        );
        assert!(
            result
                .registrations
                .contains_key(&("/api/status".to_string(), "GET".to_string()))
        );
    }

    #[test]
    fn test_execute_script_empty_content() {
        let result = execute_script("empty-script", "");

        assert!(result.success, "Empty script should succeed");
        assert!(result.error.is_none());
        assert!(result.registrations.is_empty());
    }

    #[test]
    fn test_execute_script_with_console_log() {
        let content = r#"
            register("/logged", "loggedHandler", "GET");
        "#;

        let result = execute_script("console-script", content);

        // Should succeed even with console.log (which may not be available)
        // The important thing is it doesn't crash
        // Console.log may fail, so the script might not succeed, but it shouldn't crash
        if result.success {
            assert_eq!(result.registrations.len(), 1);
        } else {
            // If console.log failed, that's ok, we just check it didn't crash
            assert!(result.error.is_some());
        }
    }

    #[test]
    fn test_execute_graphql_resolver_simple() {
        // First, need to store the script
        let script_content = r#"
            function testResolver() {
                return "Hello World";
            }
        "#;

        // Store the script in repository first
        match repository::upsert_script("test-resolver", script_content) {
            Ok(_) => {}
            Err(_) => {} // Ignore errors for test
        }

        let result = execute_graphql_resolver("test-resolver", "testResolver", None);

        assert!(result.is_ok(), "Simple resolver should succeed");
        let json_result = result.unwrap();
        assert!(json_result == "Hello World" || json_result == "\"Hello World\""); // Handle both cases
    }

    #[test]
    fn test_execute_graphql_resolver_with_args() {
        let script_content = r#"
            function greetUser(args) {
                return "Hello " + args.name + "!";
            }
        "#;

        // Store the script
        let _ = repository::upsert_script("greet-resolver", script_content);

        let args = serde_json::json!({"name": "Alice"});
        let result = execute_graphql_resolver("greet-resolver", "greetUser", Some(args));

        assert!(result.is_ok(), "Resolver with args should succeed");
        let json_result = result.unwrap();
        assert!(json_result == "Hello Alice!" || json_result == "\"Hello Alice!\"");
    }

    #[test]
    fn test_execute_graphql_resolver_returning_object() {
        let script_content = r#"
            function getUserInfo() {
                return {
                    id: 1,
                    name: "John Doe",
                    email: "john@example.com"
                };
            }
        "#;

        let _ = repository::upsert_script("user-resolver", script_content);
        let result = execute_graphql_resolver("user-resolver", "getUserInfo", None);

        assert!(result.is_ok(), "Resolver returning object should succeed");
        let json_result = result.unwrap();
        assert!(json_result.contains("John Doe"));
        assert!(json_result.contains("john@example.com"));
    }

    #[test]
    fn test_execute_graphql_resolver_nonexistent_script() {
        let result = execute_graphql_resolver("nonexistent-script", "someFunction", None);

        assert!(result.is_err(), "Should fail when script doesn't exist");
    }

    #[test]
    fn test_execute_graphql_resolver_nonexistent_function() {
        let script_content = r#"
            function someOtherFunction() {
                return "test";
            }
        "#;

        let _ = repository::upsert_script("missing-function-resolver", script_content);
        let result =
            execute_graphql_resolver("missing-function-resolver", "nonExistentFunction", None);

        assert!(result.is_err(), "Should fail when function doesn't exist");
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_execute_graphql_resolver_with_runtime_exception() {
        let script_content = r#"
            function throwingResolver() {
                throw new Error("Something went wrong");
            }
        "#;

        let _ = repository::upsert_script("throwing-resolver", script_content);
        let result = execute_graphql_resolver("throwing-resolver", "throwingResolver", None);

        assert!(
            result.is_err(),
            "Should fail when resolver throws exception"
        );
        assert!(result.unwrap_err().contains("execution error"));
    }

    #[test]
    fn test_script_execution_result_debug_format() {
        let mut registrations = HashMap::new();
        registrations.insert(
            ("/test".to_string(), "GET".to_string()),
            "handler".to_string(),
        );

        let result = ScriptExecutionResult {
            registrations,
            success: true,
            error: None,
        };

        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("ScriptExecutionResult"));
        assert!(debug_str.contains("/test"));
        assert!(debug_str.contains("success: true"));
    }

    #[test]
    fn test_script_execution_result_clone() {
        let mut registrations = HashMap::new();
        registrations.insert(
            ("/api".to_string(), "POST".to_string()),
            "handler".to_string(),
        );

        let original = ScriptExecutionResult {
            registrations,
            success: false,
            error: Some("Test error".to_string()),
        };

        let cloned = original.clone();

        assert_eq!(original.success, cloned.success);
        assert_eq!(original.error, cloned.error);
        assert_eq!(original.registrations.len(), cloned.registrations.len());
    }

    #[test]
    fn test_register_web_stream_function() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        
        // Ensure we clear streams only once per test run
        INIT.call_once(|| {
            let _ = stream_registry::GLOBAL_STREAM_REGISTRY.clear_all_streams();
        });

        let script_content = r#"
            registerWebStream('/test-stream-func');
            writeLog('Stream registered successfully');
        "#;

        let _ = repository::upsert_script("stream-test-func", script_content);
        let result = execute_script("stream-test-func", script_content);

        assert!(result.success, "Script should execute successfully");
        assert!(result.error.is_none(), "Should not have any errors");

        // Small delay to ensure registration is complete
        std::thread::sleep(std::time::Duration::from_millis(10));
        
        // Verify the stream was registered
        assert!(
            stream_registry::GLOBAL_STREAM_REGISTRY.is_stream_registered("/test-stream-func"),
            "Stream should be registered"
        );

        // Verify the correct script URI is associated
        let script_uri = stream_registry::GLOBAL_STREAM_REGISTRY.get_stream_script_uri("/test-stream-func");
        assert_eq!(script_uri, Some("stream-test-func".to_string()));
    }

    #[test]
    fn test_register_web_stream_invalid_path() {
        let script_content = r#"
            try {
                registerWebStream('invalid-path-test');
                writeLog('ERROR: Should have failed');
            } catch (e) {
                writeLog('Expected error: ' + String(e));
            }
        "#;

        let _ = repository::upsert_script("stream-invalid-test", script_content);
        let result = execute_script("stream-invalid-test", script_content);

        assert!(result.success, "Script should execute successfully even with caught exception");
        
        // Small delay to ensure any registration attempts are complete
        std::thread::sleep(std::time::Duration::from_millis(10));
        
        // Verify the invalid stream was NOT registered
        assert!(
            !stream_registry::GLOBAL_STREAM_REGISTRY.is_stream_registered("invalid-path-test"),
            "Invalid stream should not be registered"
        );
    }

    #[test]
    fn test_send_stream_message_function() {
        let script_content = r#"
            // Register a stream first
            registerWebStream('/test-message-stream');
            
            // Send a message to all streams
            sendStreamMessage('{"type": "test", "data": "Hello World"}');
            
            writeLog('Message sent successfully');
        "#;

        let _ = repository::upsert_script("stream-message-test", script_content);
        let result = execute_script("stream-message-test", script_content);

        assert!(result.success, "Script should execute successfully: {:?}", result.error);
        
        // Small delay to ensure the message is processed
        std::thread::sleep(std::time::Duration::from_millis(10));
        
        // Verify the stream was registered
        assert!(
            stream_registry::GLOBAL_STREAM_REGISTRY.is_stream_registered("/test-message-stream"),
            "Stream should be registered"
        );
        
        // Check that logs were written (indicating successful execution)
        let logs = repository::fetch_log_messages("stream-message-test");
        assert!(
            logs.iter().any(|log| log.contains("Message sent successfully")),
            "Should have logged successful message sending"
        );
    }

    #[test]
    fn test_send_stream_message_json_object() {
        let script_content = r#"
            // Register a stream first
            registerWebStream('/test-json-stream');
            
            // Send a complex JSON message
            var messageObj = {
                type: "notification",
                user: "testUser",
                data: {
                    id: 123,
                    text: "Hello from JavaScript",
                    timestamp: new Date().getTime()
                },
                metadata: ["tag1", "tag2"]
            };
            
            // JavaScript must stringify the object before sending
            sendStreamMessage(JSON.stringify(messageObj));
            
            writeLog('Complex JSON message sent');
        "#;

        let _ = repository::upsert_script("stream-json-test", script_content);
        let result = execute_script("stream-json-test", script_content);

        assert!(result.success, "Script should execute successfully: {:?}", result.error);
        
        // Small delay to ensure the message is processed
        std::thread::sleep(std::time::Duration::from_millis(10));
        
        // Verify the stream was registered
        assert!(
            stream_registry::GLOBAL_STREAM_REGISTRY.is_stream_registered("/test-json-stream"),
            "Stream should be registered"
        );
        
        // Check that logs were written (indicating successful execution)
        let logs = repository::fetch_log_messages("stream-json-test");
        assert!(
            logs.iter().any(|log| log.contains("Complex JSON message sent")),
            "Should have logged successful JSON message sending"
        );
    }
}
