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
		// Check basic JavaScript functionality
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

// Log server start with timestamp if writeLog is available
try {
	if (typeof writeLog === 'function') {
		writeLog(`server started ${new Date().toISOString()}`);
	}
} catch (e) {
	// ignore if host function isn't present yet
}
