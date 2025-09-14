// JS test script: registers /js-log-test and uses writeLog

function js_log_test_handler(path, req) {
  writeLog('js-log-test-called');
  return { status: 200, body: 'logged' };
}
register('/js-log-test', 'js_log_test_handler', 'GET');

function js_list_handler(path, req) {
  try {
    const logs = listLogs();
    return { status: 200, body: JSON.stringify(logs) };
  } catch (e) {
    return { status: 500, body: String(e) };
  }
}
register('/js-list', 'js_list_handler', 'GET');
