// core script: registers root handler
function core_root(req) {
	writeLog('core-root-called');
	writeLog('req: ' + JSON.stringify(req));
	if (auth.isAuthenticated) {
    	writeLog("User is logged in");
	} else {
    	writeLog("Anonymous user");
	}
	return { status: 200, body: 'Core handler: OK' };
}

register('/', 'core_root', 'GET');
register('/', 'core_root', 'POST');

// Health check endpoint
function health_check(req) {
	return { 
		status: 200, 
		body: JSON.stringify({
			status: "healthy",
			timestamp: new Date().toISOString(),
			checks: {
				javascript: "ok",
				logging: "ok",
				json: "ok"
			}
		}),
		contentType: 'application/json'
	};
}

register('/health', 'health_check', 'GET');

// Register script updates stream endpoint
registerWebStream('/script_updates');

// Register GraphQL subscription for script updates
registerGraphQLSubscription(
	"scriptUpdates", 
	"type ScriptUpdate { type: String!, uri: String!, action: String!, timestamp: String!, contentLength: Int, previousExists: Boolean, via: String } type Subscription { scriptUpdates: ScriptUpdate! }", 
	"scriptUpdatesResolver"
);

// GraphQL subscription resolver for script updates
function scriptUpdatesResolver() {
	writeLog("Client subscribed to scriptUpdates GraphQL subscription");
	return {
		type: "subscription_init",
		uri: "system",
		action: "initialized",
		timestamp: new Date().toISOString()
	};
}

// Helper function to broadcast script update messages
function broadcastScriptUpdate(uri, action, details = {}) {
	try {
		var message = {
			type: 'script_update',
			uri: uri,
			action: action, // 'inserted', 'updated', 'removed'
			timestamp: new Date().toISOString()
		};
		
		// Add details to the message
		for (var key in details) {
			if (details.hasOwnProperty(key)) {
				message[key] = details[key];
			}
		}
		
		sendStreamMessageToPath('/script_updates', JSON.stringify(message));
		
		// Send to GraphQL subscription using modern approach
		sendSubscriptionMessage('scriptUpdates', JSON.stringify(message));
		
		writeLog('Broadcasted script update: ' + action + ' ' + uri);
	} catch (error) {
		writeLog('Failed to broadcast script update: ' + error.message);
	}
}

// Script management endpoint
function upsert_script_handler(req) {
	try {
		// Extract parameters from form data (for POST requests)
		let uri = null;
		let content = null;
		
		if (req.form) {
			uri = req.form.uri;
			content = req.form.content;
		}
		
		// Fallback to query parameters if form data is not available
		if (!uri && req.query) {
			uri = req.query.uri;
		}
		if (!content && req.query) {
			content = req.query.content;
		}
		
		// Validate required parameters
		if (!uri) {
			return {
				status: 400,
				body: JSON.stringify({
					error: 'Missing required parameter: uri',
					timestamp: new Date().toISOString()
				}),
				contentType: 'application/json'
			};
		}
		
		if (!content) {
			return {
				status: 400,
				body: JSON.stringify({
					error: 'Missing required parameter: content',
					timestamp: new Date().toISOString()
				}),
				contentType: 'application/json'
			};
		}
		
		// Check if script already exists to determine action
		const existingScript = getScript(uri);
		const action = existingScript ? 'updated' : 'inserted';
		
		// Call the upsertScript function
		upsertScript(uri, content);
		
		// Broadcast the script update
		broadcastScriptUpdate(uri, action, {
			contentLength: content.length,
			previousExists: !!existingScript
		});
		
		writeLog(`Script upserted: ${uri} (${content.length} characters)`);
		
		return {
			status: 200,
			body: JSON.stringify({
				success: true,
				message: 'Script upserted successfully',
				uri: uri,
				contentLength: content.length,
				timestamp: new Date().toISOString()
			}),
			contentType: 'application/json'
		};
	} catch (error) {
		writeLog(`Script upsert failed: ${error.message}`);
		return {
			status: 500,
			body: JSON.stringify({
				error: 'Failed to upsert script',
				details: error.message,
				timestamp: new Date().toISOString()
			}),
			contentType: 'application/json'
		};
	}
}

register('/upsert_script', 'upsert_script_handler', 'POST');

// Script deletion endpoint
function delete_script_handler(req) {
	try {
		// Extract uri parameter from form data (for POST requests)
		let uri = null;
		
		if (req.form) {
			uri = req.form.uri;
		}
		
		// Fallback to query parameters if form data is not available
		if (!uri && req.query) {
			uri = req.query.uri;
		}
		
		// Validate required parameter
		if (!uri) {
			return {
				status: 400,
				body: JSON.stringify({
					error: 'Missing required parameter: uri',
					timestamp: new Date().toISOString()
				}),
				contentType: 'application/json'
			};
		}
		
		// Call the deleteScript function
		const deleted = deleteScript(uri);
		
		if (deleted) {
			// Broadcast the script removal
			broadcastScriptUpdate(uri, 'removed');
			
			writeLog(`Script deleted: ${uri}`);
			return {
				status: 200,
				body: JSON.stringify({
					success: true,
					message: 'Script deleted successfully',
					uri: uri,
					timestamp: new Date().toISOString()
				}),
				contentType: 'application/json'
			};
		} else {
			writeLog(`Script not found for deletion: ${uri}`);
			return {
				status: 404,
				body: JSON.stringify({
					error: 'Script not found',
					message: 'No script with the specified URI was found',
					uri: uri,
					timestamp: new Date().toISOString()
				}),
				contentType: 'application/json'
			};
		}
	} catch (error) {
		writeLog(`Script deletion failed: ${error.message}`);
		return {
			status: 500,
			body: JSON.stringify({
				error: 'Failed to delete script',
				details: error.message,
				timestamp: new Date().toISOString()
			}),
			contentType: 'application/json'
		};
	}
}

register('/delete_script', 'delete_script_handler', 'POST');

// Script reading endpoint
function read_script_handler(req) {
	try {
		// Extract uri parameter from query string
		let uri = null;
		
		if (req.query) {
			uri = req.query.uri;
		}
		
		// Validate required parameter
		if (!uri) {
			return {
				status: 400,
				body: JSON.stringify({
					error: 'Missing required parameter: uri',
					timestamp: new Date().toISOString()
				}),
				contentType: 'application/json'
			};
		}
		
		// Call the getScript function
		const content = getScript(uri);
		
		// getScript returns null if script not found or access denied
		if (content !== null && content !== undefined) {
			writeLog(`Script retrieved: ${uri} (${content.length} characters)`);
			return {
				status: 200,
				body: content,
				contentType: 'application/javascript'
			};
		} else {
			writeLog(`Script not found: ${uri}`);
			return {
				status: 404,
				body: JSON.stringify({
					error: 'Script not found',
					message: 'No script with the specified URI was found',
					uri: uri,
					timestamp: new Date().toISOString()
				}),
				contentType: 'application/json'
			};
		}
	} catch (error) {
		writeLog(`Script read failed: ${error.message}`);
		return {
			status: 500,
			body: JSON.stringify({
				error: 'Failed to read script',
				details: error.message,
				timestamp: new Date().toISOString()
			}),
			contentType: 'application/json'
		};
	}
}

register('/read_script', 'read_script_handler', 'GET');

// Script logs endpoint
function script_logs_handler(req) {
	try {
		// Extract uri parameter from query string
		let uri = null;
		
		if (req.query) {
			uri = req.query.uri;
		}
		
		// Validate required parameter
		if (!uri) {
			return {
				status: 400,
				body: JSON.stringify({
					error: 'Missing required parameter: uri',
					timestamp: new Date().toISOString()
				}),
				contentType: 'application/json'
			};
		}
		
		// Call the listLogsForUri function
		const logs = listLogsForUri(uri);
		
		writeLog(`Retrieved ${logs.length} log entries for script: ${uri}`);
		
		return {
			status: 200,
			body: JSON.stringify({
				uri: uri,
				logs: logs,
				count: logs.length,
				timestamp: new Date().toISOString()
			}),
			contentType: 'application/json'
		};
	} catch (error) {
		writeLog(`Script logs retrieval failed: ${error.message}`);
		return {
			status: 500,
			body: JSON.stringify({
				error: 'Failed to retrieve script logs',
				details: error.message,
				timestamp: new Date().toISOString()
			}),
			contentType: 'application/json'
		};
	}
}

register('/script_logs', 'script_logs_handler', 'GET');

// GraphQL operations for script management  
registerGraphQLQuery("scripts", "type ScriptInfo { uri: String!, chars: Int! } type Query { scripts: [ScriptInfo!]! }", "scriptsQuery");
registerGraphQLQuery("script", "type ScriptDetail { uri: String!, content: String!, contentLength: Int!, logs: [String!]! } type Query { script(uri: String!): ScriptDetail }", "scriptQuery");
registerGraphQLMutation("upsertScript", "type UpsertScriptResponse { message: String!, uri: String!, chars: Int!, success: Boolean! } type Mutation { upsertScript(uri: String!, content: String!): UpsertScriptResponse! }", "upsertScriptMutation");
registerGraphQLMutation("deleteScript", "type DeleteScriptResponse { message: String!, uri: String!, success: Boolean! } type Mutation { deleteScript(uri: String!): DeleteScriptResponse! }", "deleteScriptMutation");

// GraphQL resolvers
function scriptsQuery() {
	try {
		const scripts = listScripts();
		const scriptArray = Object.keys(scripts).map(uri => ({
			uri: uri,
			chars: scripts[uri].length
		}));
		return JSON.stringify(scriptArray);
	} catch (error) {
		writeLog(`Scripts query failed: ${error.message}`);
		return JSON.stringify([]);
	}
}

function scriptQuery(args) {
	try {
		const content = getScript(args.uri);
		const logs = listLogsForUri(args.uri);
		
		// getScript returns null if script not found
		if (content !== null && content !== undefined) {
			return JSON.stringify({
				uri: args.uri,
				content: content,
				contentLength: content.length,
				logs: logs
			});
		} else {
			// Return null if script doesn't exist
			return JSON.stringify({
				uri: args.uri,
				content: null,
				contentLength: 0,
				logs: logs
			});
		}
	} catch (error) {
		writeLog(`Script query failed: ${error.message}`);
		return JSON.stringify({
			uri: args.uri,
			content: null,
			contentLength: 0,
			logs: []
		});
	}
}

function upsertScriptMutation(args) {
	try {
		// Check if script already exists to determine action
		const existingScript = getScript(args.uri);
		const action = existingScript ? 'updated' : 'inserted';
		
		upsertScript(args.uri, args.content);
		
		// Broadcast the script update
		broadcastScriptUpdate(args.uri, action, {
			contentLength: args.content.length,
			previousExists: !!existingScript,
			via: 'graphql'
		});
		
		writeLog(`Script upserted via GraphQL: ${args.uri} (${args.content.length} characters)`);
		return JSON.stringify({
			message: `Script upserted successfully: ${args.uri} (${args.content.length} characters)`,
			uri: args.uri,
			chars: args.content.length,
			success: true
		});
	} catch (error) {
		writeLog(`Script upsert mutation failed: ${error.message}`);
		return JSON.stringify({
			message: `Error: Failed to upsert script: ${error.message}`,
			uri: args.uri,
			chars: args.content ? args.content.length : 0,
			success: false
		});
	}
}

function deleteScriptMutation(args) {
	try {
		const result = deleteScript(args.uri);
		// deleteScript now returns boolean: true if deleted, false if not found
		
		if (result) {
			// Broadcast the script removal
			broadcastScriptUpdate(args.uri, 'removed', {
				via: 'graphql'
			});
			
			writeLog(`Script deleted via GraphQL: ${args.uri}`);
			return JSON.stringify({
				message: `Script deleted successfully: ${args.uri}`,
				uri: args.uri,
				success: true
			});
		} else {
			return JSON.stringify({
				message: `Script not found: ${args.uri}`,
				uri: args.uri,
				success: false
			});
		}
	} catch (error) {
		writeLog(`Script delete mutation failed: ${error.message}`);
		return JSON.stringify({
			message: `Error: Failed to delete script: ${error.message}`,
			uri: args.uri,
			success: false
		});
	}
}

// GraphQL resolvers are now handled in a separate script

try {
	if (typeof writeLog === 'function') {
		writeLog(`server started ${new Date().toISOString()}`);
	}
} catch (e) {
	// ignore if host function isn't present yet
}
