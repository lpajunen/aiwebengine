// GraphQL Script Updates Demo Page
// This example demonstrates real-time script updates using GraphQL subscriptions

function scriptUpdatesDemoPage(req) {
  return {
    status: 200,
    body: `
<!DOCTYPE html>
<html>
<head>
	<title>Script Updates - GraphQL Subscription Demo</title>
	<style>
		body { font-family: Arial, sans-serif; margin: 40px; }
		.container { max-width: 1000px; }
		.updates { border: 1px solid #ddd; height: 300px; overflow-y: auto; padding: 10px; margin: 20px 0; background: #f9f9f9; }
		.update { margin: 5px 0; padding: 8px; background: #fff; border-left: 4px solid #007cba; border-radius: 3px; }
		.update.inserted { border-left-color: #28a745; }
		.update.updated { border-left-color: #ffc107; }
		.update.removed { border-left-color: #dc3545; }
		input, textarea, button { padding: 10px; margin: 5px; }
		input, textarea { width: 300px; }
		textarea { height: 100px; }
		.status { padding: 10px; margin: 10px 0; border-radius: 3px; }
		.status.connected { background: #d4edda; color: #155724; }
		.status.error { background: #f8d7da; color: #721c24; }
		.grid { display: grid; grid-template-columns: 1fr 1fr; gap: 20px; }
	</style>
</head>
<body>
	<div class="container">
		<h1>Script Updates - GraphQL Subscription Demo</h1>
		<p>This page demonstrates real-time script updates using GraphQL subscriptions.</p>
		
		<div class="status" id="status">Connecting to subscription...</div>
		
		<div class="grid">
			<div>
				<h3>Script Management via GraphQL</h3>
				<div>
					<input type="text" id="scriptUri" placeholder="Script URI (e.g., test-script.js)" />
				</div>
				<div>
					<textarea id="scriptContent" placeholder="Script content...">function testScript() {
	return "Hello from " + new Date().toISOString();
}</textarea>
				</div>
				<div>
					<button onclick="upsertScriptGraphQL()">Upsert Script (GraphQL)</button>
					<button onclick="deleteScriptGraphQL()">Delete Script (GraphQL)</button>
					<button onclick="getScriptGraphQL()">Get Script (GraphQL)</button>
				</div>
				
				<h3>Script Management via HTTP</h3>
				<div>
					<button onclick="upsertScriptHTTP()">Upsert Script (HTTP)</button>
					<button onclick="deleteScriptHTTP()">Delete Script (HTTP)</button>
				</div>
			</div>
			
			<div>
				<h3>Live Script Updates</h3>
				<div class="updates" id="updates">
					<p>Waiting for script updates...</p>
				</div>
				<button onclick="clearUpdates()">Clear Updates</button>
			</div>
		</div>
		
		<h3>Instructions</h3>
		<ol>
			<li>The page automatically subscribes to the GraphQL scriptUpdates subscription</li>
			<li>Try creating, updating, or deleting scripts using either GraphQL mutations or HTTP endpoints</li>
			<li>Watch the real-time updates appear on the right side</li>
			<li>Updates include the action (inserted/updated/removed), URI, and timestamp</li>
		</ol>
	</div>
	
	<script>
		let updateCount = 0;
		
		// Subscribe to GraphQL scriptUpdates subscription
		function subscribeToScriptUpdates() {
			const subscriptionQuery = {
				query: \`subscription { scriptUpdates }\`
			};
			
			fetch('/graphql/sse', {
				method: 'POST',
				headers: {
					'Content-Type': 'application/json',
				},
				body: JSON.stringify(subscriptionQuery)
			})
			.then(response => {
				if (!response.ok) {
					throw new Error('Failed to start subscription');
				}
				
				const reader = response.body.getReader();
				const decoder = new TextDecoder();
				
				document.getElementById('status').className = 'status connected';
				document.getElementById('status').textContent = 'Connected to scriptUpdates subscription ✓';
				
				function readStream() {
					reader.read().then(({ done, value }) => {
						if (done) {
							document.getElementById('status').className = 'status error';
							document.getElementById('status').textContent = 'Subscription ended';
							return;
						}
						
						const chunk = decoder.decode(value);
						const lines = chunk.split('\\n');
						
						lines.forEach(line => {
							if (line.startsWith('data: ')) {
								try {
									const data = JSON.parse(line.slice(6));
									if (data.data && data.data.scriptUpdates) {
										displayUpdate(data.data.scriptUpdates);
									}
								} catch (e) {
									console.log('Non-JSON data:', line);
								}
							}
						});
						
						readStream();
					});
				}
				
				readStream();
			})
			.catch(error => {
				document.getElementById('status').className = 'status error';
				document.getElementById('status').textContent = 'Connection failed: ' + error.message;
				console.error('Subscription error:', error);
			});
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
				updateEl.className = 'update ' + update.action;
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
    writeLog(
      `Initializing script_updates_demo.js script at ${new Date().toISOString()}`,
    );
    writeLog(`Init context: ${JSON.stringify(context)}`);

    // Register the demo page endpoint
    register("/script-updates-demo", "scriptUpdatesDemoPage", "GET");

    writeLog("Script updates demo script initialized successfully");

    return {
      success: true,
      message: "Script updates demo script initialized successfully",
      registeredEndpoints: 1,
    };
  } catch (error) {
    writeLog(
      `Script updates demo script initialization failed: ${error.message}`,
    );
    throw error;
  }
}
