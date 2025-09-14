// core script: registers root handler
function core_root(req) {
	writeLog('core-root-called');
	writeLog('req: ' + JSON.stringify(req));
	return { status: 200, body: 'Core handler: OK' };
}

register('/', 'core_root', 'GET');

// Log server start with timestamp if writeLog is available
try {
	if (typeof writeLog === 'function') {
		writeLog(`server started ${new Date().toISOString()}`);
	}
} catch (e) {
	// ignore if host function isn't present yet
}
