use rquickjs::{Context, Function, Runtime, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

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
                            println!(
                                "DEBUG: Registering route {} {} -> {} for script {}",
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
                        println!("DEBUG: Successfully executed script {}", uri_owned);
                        let final_regs = registrations.borrow().clone();
                        ScriptExecutionResult {
                            registrations: final_regs,
                            success: true,
                            error: None,
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to execute script {}: {}", uri_owned, e);
                        ScriptExecutionResult {
                            registrations: HashMap::new(),
                            success: false,
                            error: Some(format!("Script evaluation error: {}", e)),
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!(
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
            eprintln!(
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
    query_string: Option<&str>,
) -> Result<(u16, String), String> {
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

    let owner_script = repository::fetch_script(script_uri)
        .ok_or_else(|| format!("no script for uri {}", script_uri))?;

    ctx.with(|ctx| ctx.eval::<(), _>(owner_script.as_str()))
        .map_err(|e| format!("owner eval: {}", e))?;

    let (status, body) = ctx.with(|ctx| -> Result<(u16, String), String> {
        let global = ctx.globals();
        let func: Function = global
            .get::<_, Function>(handler_name)
            .map_err(|e| format!("no handler {}: {}", handler_name, e))?;

        let req_obj = rquickjs::Object::new(ctx).map_err(|e| format!("make req obj: {}", e))?;

        req_obj
            .set("method", method)
            .map_err(|e| format!("set method: {}", e))?;

        req_obj
            .set("path", path)
            .map_err(|e| format!("set path: {}", e))?;

        if let Some(qs) = query_string {
            req_obj
                .set("query", qs)
                .map_err(|e| format!("set query: {}", e))?;
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

        Ok((status as u16, body))
    })?;

    Ok((status, body))
}
