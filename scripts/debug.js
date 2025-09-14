// debug script: registers /debug and returns current logs via listLogs()
function debug_handler(req) {
	try {
		const logs = listLogs();
		return { 
			status: 200, 
			body: JSON.stringify(logs),
			contentType: "application/json"
		};
	} catch (e) {
		return { 
			status: 500, 
			body: String(e),
			contentType: "text/plain"
		};
	}
}
register('/debug', 'debug_handler', 'GET');
