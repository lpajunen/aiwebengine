// core script: registers root handler
function core_root(req) {
	writeLog('core-root-called');
	writeLog('req: ' + JSON.stringify(req));
	return { status: 200, body: 'Core handler: OK' };
}

register('/', 'core_root', 'GET');
register('/', 'core_root', 'POST');

// Health check endpoint
function health_check(req) {
	try {
		// GraphQL operations for script management

// Query to list all scripts
registerGraphQLQuery("scripts", "type Query { scripts: String }", "scriptsQuery");

// Query to get a specific script
registerGraphQLQuery("script", "type Query { script(uri: String!): String }", "scriptQuery");

// Mutation to upsert a script
registerGraphQLMutation("upsertScript", "type Mutation { upsertScript(uri: String!, content: String!): String }", "upsertScriptMutation");

// Mutation to delete a script
registerGraphQLMutation("deleteScript", "type Mutation { deleteScript(uri: String!): String }", "deleteScriptMutation");
		const timestamp = new Date().toISOString();
		
		// Check logging functionality
		let logWorking = true;
		try {
			writeLog('Health check performed at ' + timestamp);
		} catch (e) {
			logWorking = false;
		}
		
		// Check JSON parsing/serialization
		const testObj = { status: 'healthy', timestamp: timestamp };
		const jsonTest = JSON.stringify(testObj);
		JSON.parse(jsonTest);
		
		// Prepare health status
		const health = {
			status: 'healthy',
			timestamp: timestamp,
			checks: {
				javascript: 'ok',
				logging: logWorking ? 'ok' : 'failed',
				json: 'ok'
			},
			uptime: 'unknown' // Could be enhanced with process uptime if available
		};
		
		// If any critical check fails, mark as unhealthy
		if (!logWorking) {
			health.status = 'degraded';
		}
		
		return { 
			status: 200, 
			body: JSON.stringify(health, null, 2),
			contentType: 'application/json'
		};
	} catch (error) {
		writeLog('Health check failed: ' + error.message);
		return { 
			status: 503, 
			body: JSON.stringify({
				status: 'unhealthy',
				timestamp: new Date().toISOString(),
				error: error.message
			}),
			contentType: 'application/json'
		};
	}
}

register('/health', 'health_check', 'GET');

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
		
		// Call the upsertScript function
		upsertScript(uri, content);
		
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
		
		if (content) {
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

// Query to list all scripts
registerGraphQLQuery("scripts", "type Query { scripts: String }", "scriptsQuery");

// Query to get a specific script
registerGraphQLQuery("script", "type Query { script(uri: String!): String }", "scriptQuery");

// Mutation to upsert a script
registerGraphQLMutation("upsertScript", "type ScriptMutationResult { success: Boolean!, message: String!, uri: String!, contentLength: Int, timestamp: String! } type Mutation { upsertScript(uri: String!, content: String!): ScriptMutationResult! }", "upsertScriptMutation");

// Mutation to delete a script
registerGraphQLMutation("deleteScript", "type Mutation { deleteScript(uri: String!): ScriptMutationResult! }", "deleteScriptMutation");

// GraphQL resolvers
function scriptsQuery() {
	try {
		const scripts = listScripts();
		const scriptList = Object.keys(scripts).map(uri => `${uri}: ${scripts[uri].length} chars`).join('\n');
		return scriptList || 'No scripts found';
	} catch (error) {
		writeLog(`Scripts query failed: ${error.message}`);
		return `Error: Failed to list scripts: ${error.message}`;
	}
}

function scriptQuery(args) {
	try {
		const content = getScript(args.uri);
		if (content) {
			return JSON.stringify({
				uri: args.uri,
				content: content,
				contentLength: content.length
			});
		}
		return null;
	} catch (error) {
		writeLog(`Script query failed: ${error.message}`);
		return null;
	}
}

function upsertScriptMutation(args) {
	try {
		upsertScript(args.uri, args.content);
		writeLog(`Script upserted via GraphQL: ${args.uri} (${args.content.length} characters)`);
		return `Script upserted successfully: ${args.uri} (${args.content.length} characters)`;
	} catch (error) {
		writeLog(`Script upsert mutation failed: ${error.message}`);
		return `Error: Failed to upsert script: ${error.message}`;
	}
}

function deleteScriptMutation(args) {
	try {
		const deleted = deleteScript(args.uri);
		if (deleted) {
			writeLog(`Script deleted via GraphQL: ${args.uri}`);
			return `Script deleted successfully: ${args.uri}`;
		} else {
			return `Script not found: ${args.uri}`;
		}
	} catch (error) {
		writeLog(`Script delete mutation failed: ${error.message}`);
		return `Error: Failed to delete script: ${error.message}`;
	}
}

try {
	if (typeof writeLog === 'function') {
		writeLog(`server started ${new Date().toISOString()}`);
	}
} catch (e) {
	// ignore if host function isn't present yet
}
