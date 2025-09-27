// Simple aiwebengine Editor script
// This script provides basic editor functionality

// Serve the editor HTML page
function serveEditor(req) {
    const html = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>AIWebEngine Editor</title>
    <style>
        * {
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }

        body {
            font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
            background-color: #1e1e1e;
            color: #ffffff;
            height: 100vh;
            overflow: hidden;
        }

        .container {
            display: flex;
            height: 100vh;
        }

        .sidebar {
            width: 300px;
            background-color: #252526;
            border-right: 1px solid #3e3e42;
            display: flex;
            flex-direction: column;
        }

        .sidebar-tabs {
            display: flex;
            border-bottom: 1px solid #3e3e42;
        }

        .sidebar-tab {
            flex: 1;
            padding: 10px;
            text-align: center;
            cursor: pointer;
            background-color: #2d2d30;
            border-bottom: 2px solid transparent;
            transition: all 0.2s;
        }

        .sidebar-tab.active {
            background-color: #37373d;
            border-bottom-color: #007acc;
        }

        .sidebar-tab:hover {
            background-color: #37373d;
        }

        .sidebar-content {
            flex: 1;
            display: flex;
            flex-direction: column;
        }

        .sidebar-header {
            padding: 15px;
            border-bottom: 1px solid #3e3e42;
            font-weight: bold;
            font-size: 14px;
        }

        .script-list {
            flex: 1;
            overflow-y: auto;
        }

        .script-item {
            padding: 10px 15px;
            border-bottom: 1px solid #3e3e42;
            cursor: pointer;
            transition: background-color 0.2s;
        }

        .script-item:hover {
            background-color: #37373d;
        }

        .script-item.active {
            background-color: #007acc;
        }

        .script-item.new-script {
            color: #4ec9b0;
            font-style: italic;
        }

        .tab-panel {
            display: none;
            height: 100%;
        }

        .tab-panel.active {
            display: flex;
            flex-direction: column;
        }

        .log-item, .asset-item {
            padding: 8px 15px;
            border-bottom: 1px solid #3e3e42;
            font-family: monospace;
            font-size: 12px;
            word-wrap: break-word;
        }

        .log-item {
            color: #cccccc;
        }

        .asset-item {
            color: #9cdcfe;
        }

        .main-content {
            flex: 1;
            display: flex;
            flex-direction: column;
        }

        .toolbar {
            padding: 10px 15px;
            background-color: #323233;
            border-bottom: 1px solid #3e3e42;
            display: flex;
            gap: 10px;
            align-items: center;
        }

        .toolbar button {
            padding: 8px 16px;
            background-color: #007acc;
            color: white;
            border: none;
            border-radius: 3px;
            cursor: pointer;
            font-size: 12px;
            transition: background-color 0.2s;
        }

        .toolbar button:hover {
            background-color: #005a9e;
        }

        .toolbar button:disabled {
            background-color: #555;
            cursor: not-allowed;
        }

        .toolbar input {
            flex: 1;
            padding: 8px;
            background-color: #3c3c3c;
            border: 1px solid #555;
            border-radius: 3px;
            color: white;
            font-size: 12px;
        }

        .editor-container {
            flex: 1;
            position: relative;
        }

        .editor {
            width: 100%;
            height: 100%;
            background-color: #1e1e1e;
            color: #ffffff;
            border: none;
            padding: 15px;
            font-family: 'Consolas', 'Monaco', 'Courier New', monospace;
            font-size: 14px;
            line-height: 1.5;
            resize: none;
            outline: none;
        }

        .status-bar {
            padding: 5px 15px;
            background-color: #007acc;
            color: white;
            font-size: 12px;
            display: flex;
            justify-content: space-between;
        }

        .loading {
            display: flex;
            justify-content: center;
            align-items: center;
            height: 100vh;
            font-size: 18px;
            color: #cccccc;
        }

        .error {
            color: #f48771;
            padding: 10px;
            background-color: #1e1e1e;
            border: 1px solid #f48771;
            margin: 10px;
            border-radius: 3px;
        }
    </style>
</head>
<body>
    <div id="loading" class="loading">Loading AIWebEngine Editor...</div>

    <div id="app" style="display: none;">
        <div class="container">
            <div class="sidebar">
                <div class="sidebar-tabs">
                    <div class="sidebar-tab active" onclick="switchTab('scripts')">Scripts</div>
                    <div class="sidebar-tab" onclick="switchTab('logs')">Logs</div>
                    <div class="sidebar-tab" onclick="switchTab('assets')">Assets</div>
                </div>
                <div class="sidebar-content">
                    <div id="scripts-panel" class="tab-panel active">
                        <div class="sidebar-header">Scripts</div>
                        <div id="script-list" class="script-list">
                            <div class="script-item new-script" onclick="createNewScript()">
                                + New Script
                            </div>
                        </div>
                    </div>
                    <div id="logs-panel" class="tab-panel">
                        <div class="sidebar-header">Logs</div>
                        <div id="logs-list" class="script-list">
                            <div class="log-item">Loading logs...</div>
                        </div>
                    </div>
                    <div id="assets-panel" class="tab-panel">
                        <div class="sidebar-header">Assets</div>
                        <div id="assets-list" class="script-list">
                            <div class="asset-item">Loading assets...</div>
                        </div>
                    </div>
                </div>
            </div>

            <div class="main-content">
                <div class="toolbar">
                    <button id="save-btn" onclick="saveScript()" disabled>Save</button>
                    <button onclick="refreshScripts()">Refresh</button>
                    <input id="script-name" type="text" placeholder="Script name..." readonly>
                </div>

                <div class="editor-container">
                    <textarea id="editor" class="editor" placeholder="Select a script to edit..."></textarea>
                </div>

                <div class="status-bar">
                    <span id="status">Ready</span>
                    <span id="info"></span>
                </div>
            </div>
        </div>
    </div>

    <script>
        let currentScript = null;
        let scripts = [];

        // Initialize the editor
        async function init() {
            try {
                document.getElementById('loading').style.display = 'none';
                document.getElementById('app').style.display = 'flex';
                await loadScripts();
                switchTab('scripts'); // Start with scripts tab
            } catch (error) {
                showError('Failed to initialize editor: ' + error.message);
            }
        }

        // Load all scripts
        async function loadScripts() {
            try {
                setStatus('Loading scripts...');
                const response = await fetch('/api/scripts');
                if (!response.ok) throw new Error('Failed to load scripts');

                scripts = await response.json();
                renderScriptList();
                setStatus('Scripts loaded');
            } catch (error) {
                showError('Failed to load scripts: ' + error.message);
            }
        }

        // Render the script list
        function renderScriptList() {
            const list = document.getElementById('script-list');
            const existingItems = list.querySelectorAll('.script-item:not(.new-script)');

            // Remove existing script items
            existingItems.forEach(item => item.remove());

            // Add script items
            scripts.forEach(script => {
                const item = document.createElement('div');
                item.className = 'script-item';
                item.textContent = script.name;
                item.onclick = () => loadScript(script.name);
                if (currentScript === script.name) {
                    item.classList.add('active');
                }
                list.appendChild(item);
            });
        }

        // Load a specific script
        async function loadScript(scriptName) {
            try {
                setStatus('Loading script...');
                const response = await fetch('/api/scripts/' + encodeURIComponent(scriptName));
                if (!response.ok) throw new Error('Failed to load script');

                const content = await response.text();
                document.getElementById('editor').value = content;
                document.getElementById('script-name').value = scriptName;
                document.getElementById('save-btn').disabled = false;

                currentScript = scriptName;
                renderScriptList();
                setStatus('Script loaded');
            } catch (error) {
                showError('Failed to load script: ' + error.message);
            }
        }

        // Save the current script
        async function saveScript() {
            if (!currentScript) return;

            try {
                setStatus('Saving script...');
                const content = document.getElementById('editor').value;

                const response = await fetch('/api/scripts/' + encodeURIComponent(currentScript), {
                    method: 'POST',
                    headers: {
                        'Content-Type': 'text/plain',
                    },
                    body: content
                });

                if (!response.ok) throw new Error('Failed to save script');

                const result = await response.json();
                setStatus('Script saved successfully');
            } catch (error) {
                showError('Failed to save script: ' + error.message);
            }
        }

        // Create a new script
        function createNewScript() {
            const scriptName = prompt('Enter script name:');
            if (scriptName) {
                currentScript = scriptName;
                document.getElementById('editor').value = '// New script\\n// Add your code here\\n';
                document.getElementById('script-name').value = scriptName;
                document.getElementById('save-btn').disabled = false;
                renderScriptList();
                setStatus('New script created');
            }
        }

        // Refresh scripts list
        async function refreshScripts() {
            await loadScripts();
        }

        // Switch between tabs
        function switchTab(tabName) {
            // Update tab buttons
            document.querySelectorAll('.sidebar-tab').forEach(tab => {
                tab.classList.remove('active');
            });
            event.target.classList.add('active');

            // Update tab panels
            document.querySelectorAll('.tab-panel').forEach(panel => {
                panel.classList.remove('active');
            });
            document.getElementById(tabName + '-panel').classList.add('active');

            // Load content for the selected tab
            if (tabName === 'logs') {
                loadLogs();
            } else if (tabName === 'assets') {
                loadAssets();
            } else if (tabName === 'scripts') {
                loadScripts();
            }
        }

        // Load logs
        async function loadLogs() {
            try {
                setStatus('Loading logs...');
                const response = await fetch('/api/logs');
                if (!response.ok) throw new Error('Failed to load logs');

                const logs = await response.json();
                renderLogsList(logs);
                setStatus('Logs loaded');
            } catch (error) {
                showError('Failed to load logs: ' + error.message);
                document.getElementById('logs-list').innerHTML = '<div class="log-item">Failed to load logs</div>';
            }
        }

        // Load assets
        async function loadAssets() {
            try {
                setStatus('Loading assets...');
                const response = await fetch('/api/assets');
                if (!response.ok) throw new Error('Failed to load assets');

                const data = await response.json();
                renderAssetsList(data.assets || []);
                setStatus('Assets loaded');
            } catch (error) {
                showError('Failed to load assets: ' + error.message);
                document.getElementById('assets-list').innerHTML = '<div class="asset-item">Failed to load assets</div>';
            }
        }

        // Render logs list
        function renderLogsList(logs) {
            const list = document.getElementById('logs-list');
            list.innerHTML = '<div class="log-item">Loading logs...</div>';

            if (logs.length === 0) {
                list.innerHTML = '<div class="log-item">No logs available</div>';
                return;
            }

            list.innerHTML = '';
            logs.forEach(function(log) {
                const item = document.createElement('div');
                item.className = 'log-item';
                item.textContent = '[' + log.timestamp + '] ' + log.level + ': ' + log.message;
                list.appendChild(item);
            });
        }

        // Render assets list
        function renderAssetsList(assets) {
            const list = document.getElementById('assets-list');
            list.innerHTML = '<div class="asset-item">Loading assets...</div>';

            if (assets.length === 0) {
                list.innerHTML = '<div class="asset-item">No assets available</div>';
                return;
            }

            list.innerHTML = '';
            assets.forEach(function(asset) {
                const item = document.createElement('div');
                item.className = 'asset-item';
                item.textContent = asset.name + ' (' + asset.size + ' bytes)';
                list.appendChild(item);
            });
        }

        // Set status message
        function setStatus(message) {
            document.getElementById('status').textContent = message;
        }

        // Show error message
        function showError(message) {
            const errorDiv = document.createElement('div');
            errorDiv.className = 'error';
            errorDiv.textContent = message;
            document.body.appendChild(errorDiv);
            setTimeout(() => errorDiv.remove(), 5000);
        }

        // Initialize when page loads
        window.onload = init;
    </script>
</body>
</html>`;

    return {
        status: 200,
        body: html,
        contentType: 'text/html'
    };
}

// API: List all scripts
function apiListScripts(req) {
    try {
        const scripts = (typeof listScripts === 'function') ? listScripts() : [];
        const scriptDetails = scripts.map(name => ({
            name: name,
            size: 0,
            lastModified: new Date().toISOString()
        }));

        return {
            status: 200,
            body: JSON.stringify(scriptDetails),
            contentType: 'application/json'
        };
    } catch (error) {
        return {
            status: 500,
            body: JSON.stringify({ error: error.message }),
            contentType: 'application/json'
        };
    }
}

// API: Get script content
function apiGetScript(req) {
    try {
        // Extract the script name from the path
        // The path will be something like /api/scripts/https://example.com/core
        let scriptName = req.path.replace('/api/scripts/', '');

        // URL decode the script name in case it contains encoded characters
        scriptName = decodeURIComponent(scriptName);

        // If it's already a full URI, use it as-is
        // If it's just a short name, convert it to full URI
        let fullUri;
        if (scriptName.startsWith('https://')) {
            fullUri = scriptName;
        } else {
            fullUri = 'https://example.com/' + scriptName;
        }

        let content = '';

        if (typeof getScript === 'function') {
            content = getScript(fullUri) || '';
        } else {
            return {
                status: 500,
                body: 'getScript function not available',
                contentType: 'text/plain'
            };
        }

        if (!content) {
            return {
                status: 404,
                body: 'Script not found',
                contentType: 'text/plain'
            };
        }

        return {
            status: 200,
            body: content,
            contentType: 'text/plain'
        };
    } catch (error) {
        return {
            status: 500,
            body: 'Error: ' + error.message,
            contentType: 'text/plain'
        };
    }
}

// API: Save/update script
function apiSaveScript(req) {
    try {
        // Extract the script name from the path
        let scriptName = req.path.replace('/api/scripts/', '');

        // If it's already a full URI, use it as-is
        // If it's just a short name, convert it to full URI
        let fullUri;
        if (scriptName.startsWith('https://')) {
            fullUri = scriptName;
        } else {
            fullUri = 'https://example.com/' + scriptName;
        }

        if (typeof upsertScript === 'function') {
            // Check if script already exists to determine action
            const existingScript = getScript ? getScript(fullUri) : null;
            const action = existingScript ? 'updated' : 'inserted';
            
            upsertScript(fullUri, req.body);
            
            // Broadcast the script update notification
            if (typeof sendStreamMessageToPath === 'function') {
                try {
                    const message = {
                        type: 'script_update',
                        uri: fullUri,
                        action: action,
                        timestamp: new Date().toISOString(),
                        contentLength: req.body.length,
                        previousExists: !!existingScript,
                        via: 'editor'
                    };
                    sendStreamMessageToPath('/script_updates', JSON.stringify(message));
                    writeLog('Broadcasted script update from editor: ' + action + ' ' + fullUri);
                } catch (broadcastError) {
                    writeLog('Failed to broadcast script update from editor: ' + broadcastError.message);
                }
            }
        }

        return {
            status: 200,
            body: JSON.stringify({ message: 'Script saved' }),
            contentType: 'application/json'
        };
    } catch (error) {
        return {
            status: 500,
            body: JSON.stringify({ error: error.message }),
            contentType: 'application/json'
        };
    }
}

// API: Get logs
function apiGetLogs(req) {
    try {
        const logs = (typeof listLogs === 'function') ? listLogs() : [];
        const formattedLogs = logs.map(log => ({
            timestamp: new Date().toISOString(),
            level: 'info',
            message: log
        }));

        return {
            status: 200,
            body: JSON.stringify(formattedLogs),
            contentType: 'application/json'
        };
    } catch (error) {
        return {
            status: 500,
            body: JSON.stringify({ error: error.message }),
            contentType: 'application/json'
        };
    }
}

// API: Get assets
function apiGetAssets(req) {
    try {
        const assets = (typeof listAssets === 'function') ? listAssets() : [];
        const assetDetails = assets.map(path => ({
            path: path,
            name: path.split('/').pop(),
            size: 0,
            type: 'application/octet-stream'
        }));

        return {
            status: 200,
            body: JSON.stringify({ assets: assetDetails }),
            contentType: 'application/json'
        };
    } catch (error) {
        return {
            status: 500,
            body: JSON.stringify({ error: error.message }),
            contentType: 'application/json'
        };
    }
}

// Register routes
register('/editor', 'serveEditor', 'GET');
register('/api/scripts', 'apiListScripts', 'GET');
register('/api/scripts/*', 'apiGetScript', 'GET');
register('/api/scripts/*', 'apiSaveScript', 'POST');
register('/api/logs', 'apiGetLogs', 'GET');
register('/api/assets', 'apiGetAssets', 'GET');