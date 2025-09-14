// debug script: registers /debug and returns current logs via listLogs()
function debug_handler(req) {
	try {
		const logs = listLogs();
		return { status: 200, body: JSON.stringify(logs) };
	} catch (e) {
		return { status: 500, body: String(e) };
	}
}
register('/debug', 'debug_handler', 'GET');
