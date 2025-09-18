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

// Log server start with timestamp if writeLog is available
try {
	if (typeof writeLog === 'function') {
		writeLog(`server started ${new Date().toISOString()}`);
	}
} catch (e) {
	// ignore if host function isn't present yet
}
