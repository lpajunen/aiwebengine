// debug script: registers /debug and returns current logs via listLogs()
register('/debug', (req) => {
	try {
		const logs = listLogs();
		return { status: 200, body: JSON.stringify(logs) };
	} catch (e) {
		return { status: 500, body: String(e) };
	}
});
