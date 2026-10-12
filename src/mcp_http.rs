//! The `/mcp` endpoint: one JSON-RPC request in, one answer out.
//!
//! What a method does lives in `mcp.rs`, `mcp_tasks.rs` and
//! `mcp_elicitation.rs`; this is the dispatch in front of them — the era a
//! request speaks, who is asking, which method, and the reply's envelope. The
//! caller's session is read from the request's extensions, where
//! `auth::mcp_auth_middleware` put it.

use super::*;
use serde::Deserialize;

/// Answer one `/mcp` request.
pub(crate) async fn handle(
    req: axum::http::Request<axum::body::Body>,
    max_request_body: usize,
) -> axum::response::Json<serde_json::Value> {
    let mcp_session = req
        .extensions()
        .get::<auth::McpAuthSession>()
        .map(|auth_session| auth_session.session.clone());
    // Which host's script registrations this client sees
    let raw_host = request_host(req.headers());
    let canonical_host = hosts::canonical_host(raw_host.as_deref());
    // The engine's own tools follow `server.management_hosts`, matched
    // against the Host header itself rather than the canonicalised host —
    // a management host need not be one of the hosts scripts publish on.
    let native_tools_allowed = engine_api::is_management_host(raw_host.as_deref());
    // `Mcp-Method` lets a gateway route and meter without parsing the body.
    // Read it before the body is taken, so the two can be compared.
    let routed_method = req
        .headers()
        .get(mcp::HEADER_MCP_METHOD)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    let body_bytes = match axum::body::to_bytes(req.into_body(), max_request_body).await {
        Ok(bytes) => bytes,
        Err(e) => {
            error!("MCP: Failed to read request body: {}", e);
            return axum::response::Json(serde_json::json!({
                "jsonrpc": "2.0",
                "error": {
                    "code": -32700,
                    "message": "Parse error: request body too large or unreadable"
                },
                "id": null
            }));
        }
    };

    #[derive(Deserialize)]
    struct JsonRpcRequest {
        jsonrpc: String,
        id: Option<serde_json::Value>,
        method: String,
        params: Option<serde_json::Value>,
    }

    let rpc_request: JsonRpcRequest = match serde_json::from_slice(&body_bytes) {
        Ok(req) => req,
        Err(e) => {
            error!("MCP: Invalid JSON-RPC request: {}", e);
            return axum::response::Json(serde_json::json!({
                "jsonrpc": "2.0",
                "error": {
                    "code": -32700,
                    "message": format!("Parse error: {}", e)
                },
                "id": null
            }));
        }
    };

    // Validate JSON-RPC version
    if rpc_request.jsonrpc != "2.0" {
        return axum::response::Json(serde_json::json!({
            "jsonrpc": "2.0",
            "error": {
                "code": -32600,
                "message": "Invalid Request: jsonrpc must be 2.0"
            },
            "id": rpc_request.id
        }));
    }

    // A gateway that routed on `Mcp-Method` and a body that says something
    // else cannot both be right, and guessing which to believe is how a
    // request gets metered as one thing and executed as another.
    if let Some(routed) = routed_method
        .as_deref()
        .filter(|routed| *routed != rpc_request.method)
    {
        return axum::response::Json(serde_json::json!({
            "jsonrpc": "2.0",
            "error": {
                "code": mcp::ERROR_HEADER_MISMATCH,
                "message": format!(
                    "Mcp-Method header says {routed}, body says {}",
                    rpc_request.method
                )
            },
            "id": rpc_request.id
        }));
    }

    // Which era this request is speaking. `server/discover` is exempt from
    // the version check below: its entire purpose is to answer "what do you
    // speak", and refusing to say so because the asker guessed wrong would
    // make a client probe for an answer it came to be told. Every other
    // modern method is held to the version it named.
    let era = mcp::classify_era(rpc_request.params.as_ref());
    // Kept for `tools/call`, which has to know whether this client can be
    // asked anything before it lets a handler try.
    let client_capabilities = rpc_request
        .params
        .as_ref()
        .and_then(|params| params.get("_meta"))
        .and_then(|meta| meta.get(mcp::META_CLIENT_CAPABILITIES))
        .cloned();
    let era_is_modern = matches!(era, mcp::Era::Modern { .. });
    if let mcp::Era::Modern { version } = &era {
        if rpc_request.method != "server/discover" {
            if !mcp::supports_modern_version(version) {
                return axum::response::Json(mcp::unsupported_version_error(
                    rpc_request.id,
                    version,
                ));
            }
            // Required on every modern request. A server must not rely on a
            // capability the client never declared, and "declared none" and
            // "did not say" are different claims — so this checks that the
            // field is there, not what is in it.
            if !mcp::declares_client_capabilities(rpc_request.params.as_ref()) {
                return axum::response::Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32602,
                        "message": format!(
                            "Invalid params: {} is required on every {version} request",
                            mcp::META_CLIENT_CAPABILITIES
                        )
                    },
                    "id": rpc_request.id
                }));
            }
        }
        if let Some(name) = mcp::client_name(rpc_request.params.as_ref()) {
            debug!("MCP: {name} speaking {version}");
        }
    }

    match rpc_request.method.as_str() {
        "server/discover" => {
            // Servers MUST implement this. It is also the probe a dual-era
            // client uses to find out the engine is not a legacy server.
            axum::response::Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": rpc_request.id,
                "result": mcp::discover_result(native_tools_allowed)
            }))
        }
        "initialize" => {
            // The legacy handshake, kept because the engine is a dual-era
            // server: `2026-07-28` removed `initialize`, and a client that
            // sends one is telling us it speaks a revision that still has
            // it. Answered under that revision's rules throughout — which
            // is why this is the one arm whose result is not run through
            // `mcp::complete`: `resultType` belongs to an era that has no
            // `initialize`, and `serverInfo` already has a home here.
            info!("MCP: Initialize request received");

            // Answer with the client's own version when we speak it, so a
            // client is not told the engine predates its transport.
            let params = rpc_request.params.unwrap_or(serde_json::json!({}));
            let negotiated = mcp::negotiate_protocol_version(
                params.get("protocolVersion").and_then(|v| v.as_str()),
            );

            axum::response::Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": rpc_request.id,
                "result": {
                    "protocolVersion": negotiated,
                    "capabilities": {
                        // `listChanged` is false because there is nothing
                        // that could ever send the notification: it travels
                        // server-to-client, `/mcp` answers a POST and holds
                        // no stream, and `grep` finds no emitter. Claiming
                        // it told a conforming client to cache the tool list
                        // and wait to be told otherwise — which is exactly
                        // wrong here, since scripts register tools at
                        // runtime and the list genuinely does change. False
                        // makes a client re-list instead of trusting a
                        // promise the engine cannot keep. The modern era
                        // answers this properly rather than by omission:
                        // `mcp::LIST_CACHE_TTL_MS` says how long a list
                        // stays good, which is the thing a client can act
                        // on without anyone having to push.
                        "tools": {
                            "listChanged": false
                        },
                        "prompts": {
                            "listChanged": false
                        },
                        // `subscribe` is false for the same reason
                        // `listChanged` is: a subscription is a thing the
                        // server pushes on, and a POST response has
                        // nothing to push down. Resources here are
                        // asset-backed, so a client that wants to know
                        // whether one moved re-reads it.
                        "resources": {
                            "subscribe": false,
                            "listChanged": false
                        },
                        // Honest: `completion/complete` is handled below.
                        "completions": {}
                        // No `extensions` here, and the tasks extension in
                        // particular. Its negotiation is per-request — a
                        // client declares it in `_meta` on every call — and
                        // a legacy client has no `_meta` to declare it in.
                        // Advertising it would promise a legacy client
                        // something it has no way to take up, and the
                        // engine would never hand it a handle: the check
                        // that gates one reads the field it cannot send.
                    },
                    "serverInfo": {
                        "name": "aiwebengine",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "instructions": mcp::instructions(native_tools_allowed)
                }
            }))
        }
        "notifications/initialized" => {
            // Client signals it's ready after initialization
            info!("MCP: Client initialized notification received");

            // This is a notification, no response needed
            // But we return empty success for compatibility
            axum::response::Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": rpc_request.id
            }))
        }
        "tools/list" => {
            let tools = mcp::list_tools_for_host(&canonical_host, native_tools_allowed).await;

            let tools_list: Vec<serde_json::Value> = tools
                .iter()
                .map(|tool| {
                    serde_json::json!({
                        "name": tool.name,
                        "description": tool.description,
                        "inputSchema": tool.input_schema
                    })
                })
                .collect();

            axum::response::Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": rpc_request.id,
                "result": mcp::complete(serde_json::json!({
                    "tools": tools_list,
                    "ttlMs": mcp::LIST_CACHE_TTL_MS,
                    "cacheScope": mcp::LIST_CACHE_SCOPE
                }))
            }))
        }
        "tools/call" => {
            #[derive(Deserialize)]
            struct ToolCallParams {
                name: String,
                arguments: Option<serde_json::Value>,
                /// What the person answered, keyed as the `inputRequests`
                /// that asked for it. Present only on a retry.
                #[serde(rename = "inputResponses")]
                input_responses: Option<serde_json::Value>,
                /// The sealed state the engine minted last time round. The
                /// client echoes it without being able to read it.
                #[serde(rename = "requestState")]
                request_state: Option<String>,
            }

            let params: ToolCallParams = match rpc_request.params {
                Some(p) => match serde_json::from_value(p) {
                    Ok(params) => params,
                    Err(e) => {
                        error!("MCP tools/call: Invalid params: {}", e);
                        return axum::response::Json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "error": {
                                "code": -32602,
                                "message": format!("Invalid params: {}", e)
                            },
                            "id": rpc_request.id
                        }));
                    }
                },
                None => {
                    return axum::response::Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "error": {
                            "code": -32602,
                            "message": "Invalid params: missing required params"
                        },
                        "id": rpc_request.id
                    }));
                }
            };

            let arguments = params.arguments.unwrap_or(serde_json::json!({}));
            let auth_context = mcp_session
                .as_ref()
                .map(|session| create_js_auth_context_from_session(Some(session)));
            let user_context = create_user_context_from_session(mcp_session.as_ref());

            // Whether this caller can be asked a question mid-call.
            //
            // The era gate is not a formality: `input_required` is a
            // `resultType`, and `resultType` does not exist before
            // `2026-07-28`. A legacy client would be handed a result it has
            // no case for, so a handler running for one must find
            // `mcp.canAsk()` false and take its other path.
            let principal = mcp_session
                .as_ref()
                .map(|session| session.user_id.clone())
                .unwrap_or_else(|| "anonymous".to_string());
            let can_ask =
                era_is_modern && mcp_elicitation::client_can_elicit(client_capabilities.as_ref());
            // Modern-era only for the same reason `can_ask` is: the
            // extension is declared per request in `_meta`, which a legacy
            // client does not send. A handle given to a client that cannot
            // read `resultType: "task"` is a tool call answered with an
            // object full of fields it never asked for.
            let can_hand_off =
                era_is_modern && mcp_tasks::client_accepts_tasks(client_capabilities.as_ref());
            let args_digest = mcp_elicitation::digest_arguments(&params.name, &arguments);

            // A presented request state is attacker-controlled, and is
            // refused unless it was minted for this account, on this host,
            // for this call, and has not lapsed. Every refusal answers the
            // same way: naming which check failed tells whoever is probing
            // how to pass it.
            let exchange = match mcp_elicitation::exchange_for(
                params.request_state.as_deref(),
                params.input_responses.as_ref(),
                &principal,
                &canonical_host,
                &params.name,
                &args_digest,
                can_ask,
            )
            .map(|exchange| exchange.with_task_handoff(can_hand_off))
            {
                Ok(exchange) => exchange,
                Err(reason) => {
                    warn!(
                        "MCP tool '{}': refusing the request state presented with it ({:?})",
                        params.name, reason
                    );
                    return axum::response::Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "error": {
                            "code": -32602,
                            "message": "Invalid params: requestState is not valid for this request"
                        },
                        "id": rpc_request.id
                    }));
                }
            };

            // A tool runs JavaScript to completion — a script-registered
            // handler, or the engine's own test runner — which is CPU-bound
            // work with a budget measured in seconds. On the async thread
            // that blocks a worker for the whole run, so it goes to the
            // blocking pool like every other script execution path.
            let tool_name = params.name.clone();

            // Repeat the listing's host filter at dispatch, so naming a
            // tool that is not published here does not reach its script.
            if !mcp::tool_is_available_on_host(&tool_name, &canonical_host, native_tools_allowed)
                .await
            {
                warn!(
                    "MCP tool '{}' is not published on host {}; refusing the call",
                    tool_name, canonical_host
                );
                return axum::response::Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": rpc_request.id,
                    "error": {
                        "code": -32601,
                        "message": format!("Tool not found: {}", tool_name)
                    }
                }));
            }

            // A tool's own budget is enforced twice over — by the
            // JavaScript interrupt handler between bytecode instructions,
            // and by the host-call budget across a call that has left
            // JavaScript to wait on the database or the network. This
            // backstop covers what neither reaches: a wait inside the
            // engine itself. The blocking thread is still lost when it
            // fires; what it recovers is the response.
            let backstop = std::time::Duration::from_millis(mcp::tool_call_backstop_ms(&tool_name));
            let tool_name_for_error = tool_name.clone();
            let (ticket, watch) = worker_census::watch(format!("mcp tool {}", tool_name));
            let execution = match tokio::time::timeout(backstop, async move {
                let permit = execution_slots::acquire().await;
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    let _ticket = ticket;
                    mcp::execute_mcp_tool(
                        &tool_name,
                        arguments,
                        auth_context,
                        user_context,
                        exchange,
                    )
                })
                .await
            })
            .await
            {
                Ok(joined) => joined
                    .unwrap_or_else(|join_error| Err(format!("tool task failed: {}", join_error))),
                Err(_) => {
                    watch.abandon();
                    error!(
                        "MCP tool '{}' did not answer within {:?}; abandoning the call",
                        tool_name_for_error, backstop
                    );
                    Err(format!(
                        "Tool '{}' did not answer within {}ms and was abandoned. It is \
                        blocked in a host call — a fetch, a database query, an MCP call — \
                        which the engine cannot interrupt, rather than in JavaScript. Look \
                        for an unbounded call: a request with no timeout, or a query without \
                        a limit.",
                        tool_name_for_error,
                        backstop.as_millis()
                    ))
                }
            };

            match execution {
                Ok(mcp::ToolOutcome::Handed(create_result)) => {
                    // The handler queued its work. The handle is already
                    // recorded — `mcp.task` writes it before the execution
                    // ends, because a client holding a `taskId` that names
                    // nothing is worse than a slow answer — so there is
                    // nothing to decide here but to pass it on.
                    debug!("MCP '{}' handed its work to a task", params.name);
                    let mut result = create_result;
                    if let Some(object) = result.as_object_mut() {
                        object.insert(
                            "_meta".to_string(),
                            serde_json::json!({ mcp::META_SERVER_INFO: mcp::server_info() }),
                        );
                    }
                    axum::response::Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": rpc_request.id,
                        "result": result
                    }))
                }
                Ok(mcp::ToolOutcome::InputRequired(asked)) => {
                    // The handler asked for something. This call is over —
                    // the client gathers the answers and starts a new one,
                    // which runs the handler again from the top with them
                    // in hand. Nothing is held open here.
                    debug!(
                        "MCP tool '{}' is waiting on {} input request(s)",
                        params.name,
                        asked.requests.len()
                    );

                    match mcp_elicitation::seal(
                        &principal,
                        &canonical_host,
                        &params.name,
                        &args_digest,
                        asked.answers,
                        asked.memo,
                    ) {
                        Ok(request_state) => axum::response::Json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": rpc_request.id,
                            "result": {
                                "resultType": "input_required",
                                "inputRequests": asked.requests,
                                "requestState": request_state,
                                "_meta": {
                                    mcp::META_SERVER_INFO: mcp::server_info()
                                }
                            }
                        })),
                        Err(e) => {
                            // Without state there is nothing to carry the
                            // answers back on, so the exchange cannot
                            // continue and saying so beats asking a
                            // question whose answer we could not use.
                            error!(
                                "MCP tool '{}' asked for input but its state could not be \
                                 sealed: {}",
                                params.name, e
                            );
                            axum::response::Json(serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": rpc_request.id,
                                "error": {
                                    "code": -32603,
                                    "message": format!("Could not carry the exchange: {}", e)
                                }
                            }))
                        }
                    }
                }
                Ok(mcp::ToolOutcome::Complete(result)) => {
                    debug!("MCP tool '{}' executed successfully", params.name);

                    // Parsed and re-serialised rather than passed through:
                    // a handler whose result is not JSON is a broken handler,
                    // and it should fail here rather than put whatever it
                    // returned into content as though it were data.
                    let content = match serde_json::from_str::<serde_json::Value>(&result) {
                        Ok(parsed) => vec![serde_json::json!({
                            "type": "text",
                            "text": serde_json::to_string(&parsed)
                                .unwrap_or_else(|_| "{}".to_string())
                        })],
                        Err(e) => {
                            error!(
                                "MCP tool '{}' returned something that is not JSON: {}",
                                params.name, e
                            );
                            return axum::response::Json(serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": rpc_request.id,
                                "error": {
                                    "code": -32603,
                                    "message": format!(
                                        "Failed to parse tool result as JSON: {}", e
                                    )
                                }
                            }));
                        }
                    };

                    axum::response::Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": rpc_request.id,
                        "result": mcp::complete(serde_json::json!({
                            "content": content,
                            "isError": false
                        }))
                    }))
                }
                Err(e) => {
                    error!("MCP tool '{}' execution failed: {}", params.name, e);

                    // Check if it's a "tool not found" error
                    if e.contains("not found") {
                        axum::response::Json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "error": {
                                "code": -32602,
                                "message": format!("Unknown tool: {}", params.name)
                            },
                            "id": rpc_request.id
                        }))
                    } else {
                        // Return as tool execution error (not protocol error)
                        let content = vec![serde_json::json!({
                            "type": "text",
                            "text": format!("Tool execution failed: {}", e)
                        })];

                        axum::response::Json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": rpc_request.id,
                            "result": mcp::complete(serde_json::json!({
                                "content": content,
                                "isError": true
                            }))
                        }))
                    }
                }
            }
        }
        "tasks/get" => {
            #[derive(Deserialize)]
            struct TaskParams {
                #[serde(rename = "taskId")]
                task_id: String,
            }

            let params: TaskParams = match rpc_request.params.clone() {
                Some(p) => match serde_json::from_value(p) {
                    Ok(params) => params,
                    Err(e) => {
                        return axum::response::Json(mcp_task_bad_params(rpc_request.id, e));
                    }
                },
                None => {
                    return axum::response::Json(mcp_task_missing_id(rpc_request.id));
                }
            };

            let Ok(task_id) = uuid::Uuid::parse_str(&params.task_id) else {
                return axum::response::Json(mcp_task_unknown(rpc_request.id, &params.task_id));
            };

            match mcp_tasks::get(task_id).await {
                Ok(Some(task)) => axum::response::Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": rpc_request.id,
                    "result": mcp::complete(task.detailed_result())
                })),
                // An expired handle and one that never existed answer the
                // same way, and the client's move is the same either way:
                // the work it was waiting on is not reachable through this
                // id any more.
                Ok(None) => axum::response::Json(mcp_task_unknown(rpc_request.id, &params.task_id)),
                Err(e) => {
                    error!("MCP tasks/get failed: {}", e);
                    axum::response::Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": rpc_request.id,
                        "error": { "code": -32603, "message": "Could not read the task" }
                    }))
                }
            }
        }
        "tasks/update" => {
            // Acknowledged and otherwise ignored, which is what the
            // extension asks of a server with no outstanding
            // `inputRequests`: responses for keys that are not outstanding
            // are dropped. Nothing here reaches `input_required` yet — a
            // queued run asking a person a question is a design question
            // rather than a plumbing one, since a task runs in script
            // context or as somebody who is by definition away — so every
            // key is that case. Served rather than refused because the
            // method exists and a client is entitled to call it.
            axum::response::Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": rpc_request.id,
                "result": mcp::complete(serde_json::json!({}))
            }))
        }
        "tasks/cancel" => {
            #[derive(Deserialize)]
            struct TaskParams {
                #[serde(rename = "taskId")]
                task_id: String,
            }

            let params: TaskParams = match rpc_request.params.clone() {
                Some(p) => match serde_json::from_value(p) {
                    Ok(params) => params,
                    Err(e) => {
                        return axum::response::Json(mcp_task_bad_params(rpc_request.id, e));
                    }
                },
                None => {
                    return axum::response::Json(mcp_task_missing_id(rpc_request.id));
                }
            };

            let Ok(task_id) = uuid::Uuid::parse_str(&params.task_id) else {
                return axum::response::Json(mcp_task_unknown(rpc_request.id, &params.task_id));
            };

            // Cooperative, which the extension says plainly: a handler a
            // worker has already claimed runs to its own conclusion. The
            // acknowledgement is the same either way, because a client told
            // "cancelled" that then got a result would have been lied to by
            // a more confident answer.
            match mcp_tasks::cancel(task_id).await {
                Ok(true) => axum::response::Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": rpc_request.id,
                    "result": mcp::complete(serde_json::json!({}))
                })),
                Ok(false) => {
                    axum::response::Json(mcp_task_unknown(rpc_request.id, &params.task_id))
                }
                Err(e) => {
                    error!("MCP tasks/cancel failed: {}", e);
                    axum::response::Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": rpc_request.id,
                        "error": { "code": -32603, "message": "Could not cancel the task" }
                    }))
                }
            }
        }
        "resources/list" => {
            let resources = mcp::list_resources_for_host(&canonical_host).await;

            let resources_list: Vec<serde_json::Value> = resources
                .iter()
                .map(|resource| {
                    let mut entry = serde_json::json!({
                        "uri": resource.uri,
                        "name": resource.name,
                        "description": resource.description
                    });
                    // Omitted rather than guessed when the script did not
                    // say: a client is entitled to sniff, and a wrong
                    // `mimeType` is worse than an absent one.
                    if let (Some(mime_type), Some(object)) =
                        (&resource.mime_type, entry.as_object_mut())
                    {
                        object.insert(
                            "mimeType".to_string(),
                            serde_json::Value::String(mime_type.clone()),
                        );
                    }
                    entry
                })
                .collect();

            axum::response::Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": rpc_request.id,
                "result": mcp::complete(serde_json::json!({
                    "resources": resources_list,
                    "ttlMs": mcp::LIST_CACHE_TTL_MS,
                    "cacheScope": mcp::LIST_CACHE_SCOPE
                }))
            }))
        }
        "resources/read" => {
            #[derive(Deserialize)]
            struct ResourceReadParams {
                uri: String,
            }

            let params: ResourceReadParams = match rpc_request.params {
                Some(p) => match serde_json::from_value(p) {
                    Ok(params) => params,
                    Err(e) => {
                        error!("MCP resources/read: Invalid params: {}", e);
                        return axum::response::Json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "error": {
                                "code": -32602,
                                "message": format!("Invalid params: {}", e)
                            },
                            "id": rpc_request.id
                        }));
                    }
                },
                None => {
                    return axum::response::Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "error": {
                            "code": -32602,
                            "message": "Invalid params: uri is required"
                        },
                        "id": rpc_request.id
                    }));
                }
            };

            // One refusal for "no such resource" and for "not published on
            // this host", so that a URI cannot be used to find out what
            // other hosts serve.
            let Some(resource) = mcp::resource_for_host(&params.uri, &canonical_host).await else {
                return axum::response::Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32602,
                        "message": format!("Resource not found: {}", params.uri)
                    },
                    "id": rpc_request.id
                }));
            };

            // Read at call time rather than at registration, so the answer
            // is what the asset says now. Gone means gone: a resource
            // whose asset has been deleted since `init()` is a dangling
            // registration, and answering with stale bytes would be worse
            // than saying so.
            let Some(asset) =
                repository::fetch_asset_async(&resource.script_uri, &resource.asset_name).await
            else {
                return axum::response::Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32602,
                        "message": format!(
                            "Resource '{}' is registered but its asset '{}' no longer exists",
                            resource.uri, resource.asset_name
                        )
                    },
                    "id": rpc_request.id
                }));
            };

            let mime_type = resource
                .mime_type
                .clone()
                .unwrap_or_else(|| asset.mimetype.clone());

            // `text` or `blob`, decided by whether the bytes are text —
            // not by the MIME type, which is a claim rather than a fact
            // and is wrong often enough to matter on an asset store
            // anybody can write to. An image declared `text/plain` still
            // has to travel as base64 or the JSON is not valid.
            let mut contents = serde_json::json!({
                "uri": resource.uri,
                "mimeType": mime_type
            });
            if let Some(object) = contents.as_object_mut() {
                match String::from_utf8(asset.content.clone()) {
                    Ok(text) => {
                        object.insert("text".to_string(), serde_json::Value::String(text));
                    }
                    Err(_) => {
                        use base64::Engine as _;
                        let encoded =
                            base64::engine::general_purpose::STANDARD.encode(&asset.content);
                        object.insert("blob".to_string(), serde_json::Value::String(encoded));
                    }
                }
            }

            axum::response::Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": rpc_request.id,
                "result": mcp::complete(serde_json::json!({
                    "contents": [contents]
                }))
            }))
        }
        "prompts/list" => {
            let prompts = mcp::list_prompts_for_host(&canonical_host).await;

            let prompts_list: Vec<serde_json::Value> = prompts
                .iter()
                .map(|prompt| {
                    serde_json::json!({
                        "name": prompt.name,
                        "description": prompt.description,
                        "arguments": prompt.arguments
                    })
                })
                .collect();

            axum::response::Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": rpc_request.id,
                "result": mcp::complete(serde_json::json!({
                    "prompts": prompts_list,
                    "ttlMs": mcp::LIST_CACHE_TTL_MS,
                    "cacheScope": mcp::LIST_CACHE_SCOPE
                }))
            }))
        }
        "prompts/get" => {
            #[derive(Deserialize)]
            struct PromptGetParams {
                name: String,
                arguments: Option<serde_json::Value>,
                #[serde(rename = "inputResponses")]
                input_responses: Option<serde_json::Value>,
                #[serde(rename = "requestState")]
                request_state: Option<String>,
            }

            let params: PromptGetParams = match rpc_request.params {
                Some(p) => match serde_json::from_value(p) {
                    Ok(params) => params,
                    Err(e) => {
                        error!("MCP prompts/get: Invalid params: {}", e);
                        return axum::response::Json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "error": {
                                "code": -32602,
                                "message": format!("Invalid params: {}", e)
                            },
                            "id": rpc_request.id
                        }));
                    }
                },
                None => {
                    return axum::response::Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "error": {
                            "code": -32602,
                            "message": "Invalid params: missing required params"
                        },
                        "id": rpc_request.id
                    }));
                }
            };

            // Repeat the listing's host filter at dispatch, as `tools/call`
            // does, so naming a prompt that is not published here does not
            // reach its script.
            if !mcp::prompt_is_available_on_host(&params.name, &canonical_host).await {
                warn!(
                    "MCP prompt '{}' is not published on host {}; refusing it",
                    params.name, canonical_host
                );
                return axum::response::Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32602,
                        "message": format!("Prompt '{}' not found", params.name)
                    },
                    "id": rpc_request.id
                }));
            }

            let arguments = params.arguments.unwrap_or(serde_json::json!({}));

            // The caller's own identity, built the same way the tools
            // branch builds it. A prompt handler is script code answering
            // a request, so it runs as whoever made it.
            let user_context = create_user_context_from_session(mcp_session.as_ref());

            // A prompt may ask too: the specification permits
            // `input_required` on `prompts/get` as well as `tools/call`,
            // and a prompt that needs a parameter from the person has the
            // same problem a tool does.
            let principal = mcp_session
                .as_ref()
                .map(|session| session.user_id.clone())
                .unwrap_or_else(|| "anonymous".to_string());
            let can_ask =
                era_is_modern && mcp_elicitation::client_can_elicit(client_capabilities.as_ref());
            // Modern-era only for the same reason `can_ask` is: the
            // extension is declared per request in `_meta`, which a legacy
            // client does not send. A handle given to a client that cannot
            // read `resultType: "task"` is a tool call answered with an
            // object full of fields it never asked for.
            let can_hand_off =
                era_is_modern && mcp_tasks::client_accepts_tasks(client_capabilities.as_ref());
            let args_digest = mcp_elicitation::digest_arguments(&params.name, &arguments);

            let exchange = match mcp_elicitation::exchange_for(
                params.request_state.as_deref(),
                params.input_responses.as_ref(),
                &principal,
                &canonical_host,
                &params.name,
                &args_digest,
                can_ask,
            )
            .map(|exchange| exchange.with_task_handoff(can_hand_off))
            {
                Ok(exchange) => exchange,
                Err(reason) => {
                    warn!(
                        "MCP prompt '{}': refusing the request state presented with it ({:?})",
                        params.name, reason
                    );
                    return axum::response::Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "error": {
                            "code": -32602,
                            "message": "Invalid params: requestState is not valid for this request"
                        },
                        "id": rpc_request.id
                    }));
                }
            };

            match mcp::execute_mcp_prompt(&params.name, arguments, user_context, exchange) {
                Ok(mcp::Outcome::Handed(create_result)) => {
                    // The handler queued its work. The handle is already
                    // recorded — `mcp.task` writes it before the execution
                    // ends, because a client holding a `taskId` that names
                    // nothing is worse than a slow answer — so there is
                    // nothing to decide here but to pass it on.
                    debug!("MCP '{}' handed its work to a task", params.name);
                    let mut result = create_result;
                    if let Some(object) = result.as_object_mut() {
                        object.insert(
                            "_meta".to_string(),
                            serde_json::json!({ mcp::META_SERVER_INFO: mcp::server_info() }),
                        );
                    }
                    axum::response::Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": rpc_request.id,
                        "result": result
                    }))
                }
                Ok(mcp::Outcome::InputRequired(asked)) => {
                    match mcp_elicitation::seal(
                        &principal,
                        &canonical_host,
                        &params.name,
                        &args_digest,
                        asked.answers,
                        asked.memo,
                    ) {
                        Ok(request_state) => axum::response::Json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": rpc_request.id,
                            "result": {
                                "resultType": "input_required",
                                "inputRequests": asked.requests,
                                "requestState": request_state,
                                "_meta": {
                                    mcp::META_SERVER_INFO: mcp::server_info()
                                }
                            }
                        })),
                        Err(e) => {
                            error!(
                                "MCP prompt '{}' asked for input but its state could not be \
                                 sealed: {}",
                                params.name, e
                            );
                            axum::response::Json(serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": rpc_request.id,
                                "error": {
                                    "code": -32603,
                                    "message": format!("Could not carry the exchange: {}", e)
                                }
                            }))
                        }
                    }
                }
                Ok(mcp::Outcome::Complete(result)) => {
                    // The handler should return an object with a "messages" array
                    axum::response::Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": rpc_request.id,
                        "result": mcp::complete(result)
                    }))
                }
                Err(e) => {
                    error!("MCP prompts/get error: {}", e);
                    axum::response::Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "error": {
                            "code": -32602,
                            "message": e
                        },
                        "id": rpc_request.id
                    }))
                }
            }
        }
        "completion/complete" => {
            #[derive(Deserialize)]
            struct CompletionRef {
                #[serde(rename = "type")]
                ref_type: String,
                name: Option<String>,
            }

            #[derive(Deserialize)]
            struct CompletionArgument {
                name: String,
                value: String,
            }

            #[derive(Deserialize)]
            struct CompletionContext {
                arguments: Option<serde_json::Value>,
            }

            #[derive(Deserialize)]
            struct CompletionParams {
                #[serde(rename = "ref")]
                reference: CompletionRef,
                argument: CompletionArgument,
                context: Option<CompletionContext>,
            }

            let params: CompletionParams = match rpc_request.params {
                Some(p) => match serde_json::from_value(p) {
                    Ok(params) => params,
                    Err(e) => {
                        error!("MCP completion/complete: Invalid params: {}", e);
                        return axum::response::Json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "error": {
                                "code": -32602,
                                "message": format!("Invalid params: {}", e)
                            },
                            "id": rpc_request.id
                        }));
                    }
                },
                None => {
                    return axum::response::Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "error": {
                            "code": -32602,
                            "message": "Invalid params: missing required params"
                        },
                        "id": rpc_request.id
                    }));
                }
            };

            // Only support ref/prompt type for now
            if params.reference.ref_type != "ref/prompt" {
                return axum::response::Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32602,
                        "message": format!("Unsupported reference type: {}", params.reference.ref_type)
                    },
                    "id": rpc_request.id
                }));
            }

            let prompt_name = match params.reference.name {
                Some(name) => name,
                None => {
                    return axum::response::Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "error": {
                            "code": -32602,
                            "message": "Missing prompt name in reference"
                        },
                        "id": rpc_request.id
                    }));
                }
            };

            // A completion runs the prompt's handler, so it is reachable
            // exactly where the prompt is.
            if !mcp::prompt_is_available_on_host(&prompt_name, &canonical_host).await {
                warn!(
                    "MCP prompt '{}' is not published on host {}; refusing it",
                    prompt_name, canonical_host
                );
                return axum::response::Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32602,
                        "message": format!("Prompt '{}' not found", prompt_name)
                    },
                    "id": rpc_request.id
                }));
            }

            let context_arguments = params.context.and_then(|c| c.arguments);

            let user_context = create_user_context_from_session(mcp_session.as_ref());

            match mcp::execute_mcp_completion(
                &prompt_name,
                &params.argument.name,
                &params.argument.value,
                context_arguments,
                user_context,
            ) {
                Ok(result) => {
                    // The handler should return an object with "values", optional "total", and optional "hasMore"
                    axum::response::Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": rpc_request.id,
                        "result": mcp::complete(serde_json::json!({
                            "completion": result
                        }))
                    }))
                }
                Err(e) => {
                    error!("MCP completion/complete error: {}", e);
                    axum::response::Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "error": {
                            "code": -32602,
                            "message": e
                        },
                        "id": rpc_request.id
                    }))
                }
            }
        }
        _ => axum::response::Json(serde_json::json!({
            "jsonrpc": "2.0",
            "error": {
                "code": -32601,
                "message": format!("Method not found: {}", rpc_request.method)
            },
            "id": rpc_request.id
        })),
    }
}

/// A `tasks/*` request whose `taskId` names nothing we can answer for.
///
/// One reply for "never existed" and "expired", because the client's move is
/// the same either way — the work is not reachable through this id any more —
/// and distinguishing them would let a caller probe which ids have ever been
/// issued.
fn mcp_task_unknown(id: Option<serde_json::Value>, task_id: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32602,
            "message": format!("Unknown or expired task: {}", task_id)
        }
    })
}

fn mcp_task_bad_params(
    id: Option<serde_json::Value>,
    error: serde_json::Error,
) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": -32602, "message": format!("Invalid params: {}", error) }
    })
}

fn mcp_task_missing_id(id: Option<serde_json::Value>) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": -32602, "message": "Invalid params: taskId is required" }
    })
}

fn create_js_auth_context_from_session(session: Option<&auth::AuthSession>) -> auth::JsAuthContext {
    match session {
        Some(session) => auth::JsAuthContext {
            elevation: session.elevation.clone(),
            via_token: session.via_token,
            ..auth::JsAuthContext::authenticated(
                session.user_id.clone(),
                session.email.clone(),
                session.name.clone(),
                session.provider.clone(),
                session.is_admin,
                session.is_editor,
            )
        },
        None => auth::JsAuthContext::anonymous(),
    }
}

fn create_user_context_from_session(session: Option<&auth::AuthSession>) -> security::UserContext {
    security::UserContext::for_session(session.map(auth::AuthSession::roles).unwrap_or_default())
}
