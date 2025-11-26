// GraphQL Script Updates Demo Page
// This example demonstrates real-time script updates using GraphQL subscriptions

function scriptUpdatesDemoPage(context) {
  return {
    status: 200,
    body: `
<!DOCTYPE html>
<html lang="en">
<head>
	<meta charset="UTF-8">
	<meta name="viewport" content="width=device-width, initial-scale=1.0">
	<title>Script Updates - GraphQL Subscription Demo</title>
	<link rel="stylesheet" href="/engine.css">
	<link rel="icon" type="image/x-icon" href="/favicon.ico">
	<style>
		/* Demo-specific overrides */
		body {
			background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
			padding: 2rem 0;
		}

		.demo-container {
			max-width: 1200px;
			margin: 0 auto;
			background: rgba(255, 255, 255, 0.95);
			backdrop-filter: blur(10px);
			border-radius: var(--border-radius-lg);
			box-shadow: var(--shadow-lg);
			overflow: hidden;
		}

		.demo-header {
			background: var(--bg-secondary);
			padding: 2rem;
			text-align: center;
			border-bottom: 1px solid var(--border-color);
		}

		.demo-content {
			padding: 2rem;
		}

		.updates-container {
			background: var(--bg-secondary);
			border: 1px solid var(--border-color);
			border-radius: var(--border-radius);
			height: 300px;
			overflow-y: auto;
			padding: 1rem;
			margin: 1.5rem 0;
		}

		.update-item {
			margin: 0.5rem 0;
			padding: 0.75rem;
			background: var(--bg-color);
			border-left: 4px solid var(--primary-color);
			border-radius: var(--border-radius);
			box-shadow: var(--shadow-sm);
		}

		.update-item.inserted { border-left-color: var(--success-color); }
		.update-item.updated { border-left-color: var(--warning-color); }
		.update-item.removed { border-left-color: var(--danger-color); }

		.form-group {
			margin-bottom: 1rem;
		}

		.form-group input,
		.form-group textarea {
			width: 100%;
			max-width: 400px;
		}

		.form-group textarea {
			min-height: 100px;
			resize: vertical;
		}

		.button-group {
			display: flex;
			gap: 0.5rem;
			flex-wrap: wrap;
			margin-top: 1rem;
		}

		.status-indicator {
			padding: 0.75rem 1rem;
			border-radius: var(--border-radius);
			margin: 1rem 0;
			font-weight: 500;
		}

		.status-connected {
			background: rgba(40, 167, 69, 0.1);
			border: 1px solid rgba(40, 167, 69, 0.2);
			color: #155724;
		}

		.status-error {
			background: rgba(220, 53, 69, 0.1);
			border: 1px solid rgba(220, 53, 69, 0.2);
			color: #721c24;
		}

		.demo-grid {
			display: grid;
			grid-template-columns: 1fr 1fr;
			gap: 2rem;
			margin-top: 2rem;
		}

		.demo-section {
			background: var(--bg-secondary);
			padding: 1.5rem;
			border-radius: var(--border-radius);
			border: 1px solid var(--border-color);
		}

		.demo-section h3 {
			margin-top: 0;
			margin-bottom: 1rem;
			color: var(--text-color);
		}

		.nav-links {
			text-align: center;
			margin-top: 2rem;
			padding-top: 1rem;
			border-top: 1px solid var(--border-color);
		}

		.nav-links a {
			color: var(--primary-color);
			text-decoration: none;
			margin: 0 1rem;
			font-weight: 500;
		}

		.nav-links a:hover {
			text-decoration: underline;
		}

		@media (max-width: 768px) {
			.demo-grid {
				grid-template-columns: 1fr;
				gap: 1rem;
			}

			.demo-content {
				padding: 1rem;
			}

			.demo-header {
				padding: 1rem;
			}

			.button-group {
				flex-direction: column;
			}

			.button-group .btn {
				width: 100%;
			}
		}
	</style>
</head>
<body>
	<div class="demo-container">
		<header class="demo-header">
			<h1>Script Updates - GraphQL Subscription Demo</h1>
			<p class="text-muted">This page demonstrates real-time script updates using GraphQL subscriptions.</p>
		</header>

		<main class="demo-content">
			<div class="status-indicator status-connected" id="status">Connecting to subscription...</div>

			<div class="demo-grid">
				<div class="demo-section">
					<h3>Script Management via GraphQL</h3>
					<div class="form-group">
						<label for="scriptUri" class="form-label">Script URI</label>
						<input type="text" id="scriptUri" class="form-control" placeholder="Script URI (e.g., test-script.js)" />
					</div>
					<div class="form-group">
						<label for="scriptContent" class="form-label">Script Content</label>
						<textarea id="scriptContent" class="form-control" placeholder="Script content...">function testScript() {
	return "Hello from " + new Date().toISOString();
}</textarea>
					</div>
					<div class="button-group">
						<button class="btn btn-primary" onclick="upsertScriptGraphQL()">Upsert Script (GraphQL)</button>
						<button class="btn btn-danger" onclick="deleteScriptGraphQL()">Delete Script (GraphQL)</button>
						<button class="btn btn-secondary" onclick="getScriptGraphQL()">Get Script (GraphQL)</button>
					</div>

					<h3>Script Management via HTTP</h3>
					<div class="button-group">
						<button class="btn btn-success" onclick="upsertScriptHTTP()">Upsert Script (HTTP)</button>
						<button class="btn btn-warning" onclick="deleteScriptHTTP()">Delete Script (HTTP)</button>
					</div>
				</div>

				<div class="demo-section">
					<h3>Live Script Updates</h3>
					<div class="updates-container" id="updates">
						<p class="text-muted">Waiting for script updates...</p>
					</div>
					<button class="btn btn-secondary" onclick="clearUpdates()">Clear Updates</button>
				</div>
			</div>

			<div class="demo-section">
				<h3>Instructions</h3>
				<ol>
					<li>The page automatically subscribes to the GraphQL scriptUpdates subscription</li>
					<li>Try creating, updating, or deleting scripts using either GraphQL mutations or HTTP endpoints</li>
					<li>Watch the real-time updates appear on the right side</li>
					<li>Updates include the action (inserted/updated/removed), URI, and timestamp</li>
				</ol>
			</div>

			<div class="nav-links">
				<a href="/">🏠 Home</a>
				<a href="/editor">✏️ Editor</a>
				<a href="/engine/admin">👥 User Manager</a>
				<a href="/engine/docs">📚 Documentation</a>
			</div>
		</main>
	</div>
	
	<script>
		let updateCount = 0;
		
		// Subscribe to GraphQL scriptUpdates subscription using EventSource
		function subscribeToScriptUpdates() {
			const subscriptionQuery = {
				query: \`subscription { scriptUpdates }\`
			};

			const eventSource = new EventSource('/graphql/sse?query=' + encodeURIComponent(subscriptionQuery.query));

			eventSource.onopen = function(event) {
				document.getElementById('status').className = 'status-indicator status-connected';
				document.getElementById('status').textContent = 'Connected to scriptUpdates subscription ✓';
				console.log('SSE connection opened');
			};

			eventSource.onmessage = function(event) {
				try {
					const data = JSON.parse(event.data);
					if (data.data && data.data.scriptUpdates) {
						displayUpdate(data.data.scriptUpdates);
					}
				} catch (e) {
					console.log('Non-JSON data:', event.data);
				}
			};

			eventSource.onerror = function(event) {
				document.getElementById('status').className = 'status-indicator status-error';
				document.getElementById('status').textContent = 'Connection failed or lost';
				console.error('SSE connection error:', event);
			};

			// Store the EventSource instance for potential cleanup
			window.scriptUpdatesEventSource = eventSource;
		}
		
		function displayUpdate(updateStr) {
			try {
				const update = JSON.parse(updateStr);
				const updatesDiv = document.getElementById('updates');
				
				// Remove "waiting" message if it's the first update
				if (updateCount === 0) {
					updatesDiv.innerHTML = '';
				}
				
				const updateEl = document.createElement('div');
				updateEl.className = 'update-item ' + update.action;
				updateEl.innerHTML = \`
					<strong>\${update.action.toUpperCase()}</strong>: \${update.uri}<br>
					<small>Time: \${update.timestamp}</small>
					\${update.contentLength ? '<br><small>Size: ' + update.contentLength + ' characters</small>' : ''}
					\${update.source ? '<br><small>Source: ' + update.source + '</small>' : ''}
				\`;
				
				updatesDiv.insertBefore(updateEl, updatesDiv.firstChild);
				updateCount++;
				
				// Keep only the last 50 updates
				while (updatesDiv.children.length > 50) {
					updatesDiv.removeChild(updatesDiv.lastChild);
				}
			} catch (e) {
				console.error('Failed to parse update:', e);
			}
		}
		
		function getScriptValues() {
			const uri = document.getElementById('scriptUri').value.trim();
			const content = document.getElementById('scriptContent').value.trim();
			return { uri, content };
		}
		
		function upsertScriptGraphQL() {
			const { uri, content } = getScriptValues();
			if (!uri || !content) {
				alert('Please provide both URI and content');
				return;
			}
			
			const mutation = {
				query: \`mutation { upsertScript(uri: "\${uri}", content: \${JSON.stringify(content)}) }\`
			};
			
			fetch('/graphql', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify(mutation)
			})
			.then(response => response.json())
			.then(data => {
				console.log('GraphQL upsert result:', data);
				if (data.errors) {
					alert('GraphQL Error: ' + data.errors[0].message);
				}
			})
			.catch(error => {
				console.error('GraphQL upsert error:', error);
				alert('Failed to upsert script: ' + error.message);
			});
		}
		
		function deleteScriptGraphQL() {
			const { uri } = getScriptValues();
			if (!uri) {
				alert('Please provide a URI');
				return;
			}
			
			const mutation = {
				query: \`mutation { deleteScript(uri: "\${uri}") }\`
			};
			
			fetch('/graphql', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify(mutation)
			})
			.then(response => response.json())
			.then(data => {
				console.log('GraphQL delete result:', data);
				if (data.errors) {
					alert('GraphQL Error: ' + data.errors[0].message);
				}
			})
			.catch(error => {
				console.error('GraphQL delete error:', error);
				alert('Failed to delete script: ' + error.message);
			});
		}
		
		function getScriptGraphQL() {
			const { uri } = getScriptValues();
			if (!uri) {
				alert('Please provide a URI');
				return;
			}
			
			const query = {
				query: \`query { script(uri: "\${uri}") }\`
			};
			
			fetch('/graphql', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify(query)
			})
			.then(response => response.json())
			.then(data => {
				console.log('GraphQL query result:', data);
				if (data.errors) {
					alert('GraphQL Error: ' + data.errors[0].message);
				} else if (data.data && data.data.script) {
					document.getElementById('scriptContent').value = data.data.script;
					alert('Script loaded successfully');
				} else {
					alert('Script not found');
				}
			})
			.catch(error => {
				console.error('GraphQL query error:', error);
				alert('Failed to get script: ' + error.message);
			});
		}
		
		function upsertScriptHTTP() {
			const { uri, content } = getScriptValues();
			if (!uri || !content) {
				alert('Please provide both URI and content');
				return;
			}
			
			const formData = new FormData();
			formData.append('uri', uri);
			formData.append('content', content);
			
			fetch('/upsert_script', {
				method: 'POST',
				body: formData
			})
			.then(response => response.json())
			.then(data => {
				console.log('HTTP upsert result:', data);
				if (!data.success) {
					alert('HTTP Error: ' + data.error);
				}
			})
			.catch(error => {
				console.error('HTTP upsert error:', error);
				alert('Failed to upsert script: ' + error.message);
			});
		}
		
		function deleteScriptHTTP() {
			const { uri } = getScriptValues();
			if (!uri) {
				alert('Please provide a URI');
				return;
			}
			
			const formData = new FormData();
			formData.append('uri', uri);
			
			fetch('/delete_script', {
				method: 'POST',
				body: formData
			})
			.then(response => response.json())
			.then(data => {
				console.log('HTTP delete result:', data);
				if (!data.success) {
					alert('HTTP Error: ' + data.error);
				}
			})
			.catch(error => {
				console.error('HTTP delete error:', error);
				alert('Failed to delete script: ' + error.message);
			});
		}
		
		function clearUpdates() {
			document.getElementById('updates').innerHTML = '<p>Waiting for script updates...</p>';
			updateCount = 0;
		}
		
		// Start the subscription when page loads
		subscribeToScriptUpdates();
	</script>
</body>
</html>`,
    contentType: "text/html",
  };
}

// Initialization function - called when script is loaded or updated
function init(context) {
  try {
    console.log(
      `Initializing script_updates_demo.js script at ${new Date().toISOString()}`,
    );
    console.log(`Init context: ${JSON.stringify(context)}`);

    // Register the demo page endpoint
    routeRegistry.registerRoute(
      "/script-updates-demo",
      "scriptUpdatesDemoPage",
      "GET",
    );

    console.log("Script updates demo script initialized successfully");

    return {
      success: true,
      message: "Script updates demo script initialized successfully",
      registeredEndpoints: 1,
    };
  } catch (error) {
    console.log(
      `Script updates demo script initialization failed: ${error.message}`,
    );
    throw error;
  }
}
