// core script: registers root handler
function core_root(req) { return { status: 200, body: 'Core handler: OK' }; }
register('/', 'core_root', 'GET');

// Log server start with timestamp if writeLog is available
try {
	if (typeof writeLog === 'function') {
		writeLog(`server started ${new Date().toISOString()}`);
	}
} catch (e) {
	// ignore if host function isn't present yet
}
