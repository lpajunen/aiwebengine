// core script: registers root handler
function core_root(req) {
	writeLog('core-root-called');
	writeLog('req: ' + JSON.stringify(req));
	return { status: 200, body: 'Core handler: OK' };
}

function query_handler(req) {
    let queryInfo = 'none';
    if (req.query && Object.keys(req.query).length > 0) {
        // req.query is now an object with parsed parameters
        let params = [];
        for (let key in req.query) {
            params.push(`${key}=${req.query[key]}`);
        }
        queryInfo = params.join(', ');
    }

    return {
        status: 200,
        body: `Path: ${req.path}, Query: ${queryInfo}`
    };
}

register('/', 'core_root', 'GET');
register('/api/query', 'query_handler', 'GET');

// Log server start with timestamp if writeLog is available
try {
	if (typeof writeLog === 'function') {
		writeLog(`server started ${new Date().toISOString()}`);
	}
} catch (e) {
	// ignore if host function isn't present yet
}
