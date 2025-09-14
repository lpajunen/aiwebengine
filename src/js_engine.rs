use rquickjs::{Context, Function, Runtime};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

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
