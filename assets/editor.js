// aiwebengine Editor - Main JavaScript
class AIWebEngineEditor {
  constructor() {
    this.currentScript = null;
    this.monacoEditor = null;
    this.templates = {};
    this.init();
  }

  async init() {
    console.log("[Editor] Starting initialization...");
    this.compileTemplates();
    console.log("[Editor] Templates compiled");
    this.setupEventListeners();
    console.log("[Editor] Event listeners set up");
    await this.setupMonacoEditor();
    console.log("[Editor] Monaco editor ready");
    this.loadInitialData();
    console.log("[Editor] Loading initial data...");

    // Auto-refresh logs every 5 seconds
    setInterval(() => this.loadLogs(), 5000);
  }

  compileTemplates() {
    // Using plain JavaScript template functions instead of Handlebars
    this.templates = {
      "script-item": (data) => `
        <div class="script-item ${data.active ? "active" : ""}" data-script="${data.name}">
          <div class="script-icon">📄</div>
          <div class="script-info">
            <div class="script-name">${data.name}</div>
            <div class="script-meta">${data.size} bytes</div>
          </div>
        </div>
      `,
      "asset-item": (data) => `
        <div class="asset-item" data-path="${data.path}">
          <div class="asset-preview">
            ${
              data.isImage
                ? `<img src="/api/assets${data.path}" alt="${data.name}" loading="lazy">`
                : `<div class="asset-icon">${data.icon}</div>`
            }
          </div>
          <div class="asset-info">
            <div class="asset-name">${data.name}</div>
            <div class="asset-meta">${data.size} • ${data.type}</div>
          </div>
          <div class="asset-actions">
            <button class="btn btn-small btn-secondary download-btn" data-path="${data.path}">↓</button>
            <button class="btn btn-small btn-danger delete-btn" data-path="${data.path}">×</button>
          </div>
        </div>
      `,
      "log-entry": (data) => `
        <div class="log-entry log-${data.level}">
          <span class="log-time">${data.time}</span>
          <span class="log-level">${data.level}</span>
          <span class="log-message">${data.message}</span>
        </div>
      `,
      "route-item": (data) => `
        <div class="route-item">
          <div class="route-method ${data.method}">${data.method}</div>
          <div class="route-path">${data.path}</div>
          <div class="route-handler">${data.handler}</div>
          <div class="route-actions">
            <button class="btn btn-small btn-secondary test-btn" data-path="${data.path}" data-method="${data.method}">Test</button>
          </div>
        </div>
      `,
    };
    console.log("[Editor] Templates compiled (using plain JS)");
  }

  setupEventListeners() {
    // Tab navigation
    document.querySelectorAll(".nav-tab").forEach((tab) => {
      tab.addEventListener("click", (e) =>
        this.switchTab(e.target.dataset.tab),
      );
    });

    // Script management
    document
      .getElementById("new-script-btn")
      .addEventListener("click", () => this.createNewScript());
    document
      .getElementById("save-script-btn")
      .addEventListener("click", () => this.saveCurrentScript());
    document
      .getElementById("delete-script-btn")
      .addEventListener("click", () => this.deleteCurrentScript());

    // Asset management
    document
      .getElementById("upload-asset-btn")
      .addEventListener("click", () => this.triggerAssetUpload());
    document
      .getElementById("asset-upload")
      .addEventListener("change", (e) => this.uploadAssets(e.target.files));

    // Logs
    document
      .getElementById("refresh-logs-btn")
      .addEventListener("click", () => this.loadLogs());
    document
      .getElementById("clear-logs-btn")
      .addEventListener("click", () => this.clearLogs());

    // Routes
    document
      .getElementById("refresh-routes-btn")
      .addEventListener("click", () => this.loadRoutes());

    // Test endpoint
    document
      .getElementById("test-endpoint-btn")
      .addEventListener("click", () => this.testEndpoint());

    // AI Assistant
    document
      .getElementById("toggle-ai-assistant")
      .addEventListener("click", () => this.toggleAIAssistant());
    document
      .getElementById("submit-prompt-btn")
      .addEventListener("click", () => this.submitAIPrompt());
    document
      .getElementById("clear-prompt-btn")
      .addEventListener("click", () => this.clearAIPrompt());

    // Allow Enter key to submit (Shift+Enter for new line)
    document.getElementById("ai-prompt").addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        this.submitAIPrompt();
      }
    });

    // Diff modal controls
    document
      .getElementById("close-diff-modal")
      .addEventListener("click", () => this.closeDiffModal());
    document
      .getElementById("reject-changes-btn")
      .addEventListener("click", () => this.closeDiffModal());
    document
      .getElementById("apply-changes-btn")
      .addEventListener("click", () => this.applyPendingChange());
  }

  async setupMonacoEditor() {
    // Load Monaco Editor
    return new Promise((resolve) => {
      require.config({
        paths: { vs: "https://unpkg.com/monaco-editor@0.45.0/min/vs" },
      });

      require(["vs/editor/editor.main"], () => {
        this.monacoEditor = monaco.editor.create(
          document.getElementById("monaco-editor"),
          {
            value: "// Select a script to edit",
            language: "javascript",
            theme: "vs-dark",
            fontSize: 14,
            minimap: { enabled: true },
            scrollBeyondLastLine: false,
            automaticLayout: true,
            wordWrap: "on",
          },
        );

        this.monacoEditor.onDidChangeModelContent(() => {
          this.updateSaveButton();
        });

        resolve();
      });
    });
  }

  switchTab(tabName) {
    // Update navigation
    document
      .querySelectorAll(".nav-tab")
      .forEach((tab) => tab.classList.remove("active"));
    document.querySelector(`[data-tab="${tabName}"]`).classList.add("active");

    // Update content
    document
      .querySelectorAll(".tab-content")
      .forEach((content) => content.classList.remove("active"));
    document.getElementById(`${tabName}-tab`).classList.add("active");

    // Load tab-specific data
    switch (tabName) {
      case "scripts":
        this.loadScripts();
        break;
      case "assets":
        this.loadAssets();
        break;
      case "logs":
        this.loadLogs();
        break;
      case "routes":
        this.loadRoutes();
        break;
    }
  }

  // Script Management
  async loadScripts() {
    console.log("[Editor] loadScripts() called");
    try {
      const response = await fetch("/api/scripts");
      console.log("[Editor] API response status:", response.status);
      const scripts = await response.json();
      console.log("[Editor] Loaded scripts:", scripts);

      const scriptsList = document.getElementById("scripts-list");
      scriptsList.innerHTML = "";

      scripts.forEach((script) => {
        const scriptElement = document.createElement("div");
        scriptElement.innerHTML = this.templates["script-item"]({
          name: script.name,
          size: script.size || 0,
          active: script.name === this.currentScript,
        });

        scriptElement
          .querySelector(".script-item")
          .addEventListener("click", () => {
            this.loadScript(script.name);
          });

        scriptsList.appendChild(scriptElement.firstElementChild);
      });
    } catch (error) {
      this.showStatus("Error loading scripts: " + error.message, "error");
    }
  }

  async loadScript(scriptName) {
    console.log("[Editor] loadScript() called for:", scriptName);
    try {
      const encodedScriptName = encodeURIComponent(scriptName);
      const response = await fetch(`/api/scripts/${encodedScriptName}`);
      console.log("[Editor] loadScript response status:", response.status);
      const content = await response.text();
      console.log("[Editor] Script content length:", content.length);

      this.currentScript = scriptName;
      document.getElementById("current-script-name").textContent = scriptName;

      if (this.monacoEditor) {
        console.log("[Editor] Setting Monaco editor value...");
        this.monacoEditor.setValue(content);
        this.updateSaveButton();
      } else {
        console.error("[Editor] Monaco editor not available!");
      }

      // Update active state in list
      document.querySelectorAll(".script-item").forEach((item) => {
        item.classList.toggle("active", item.dataset.script === scriptName);
      });

      document.getElementById("delete-script-btn").disabled = false;
    } catch (error) {
      this.showStatus("Error loading script: " + error.message, "error");
    }
  }

  createNewScript() {
    const scriptName = prompt("Enter script name (without .js extension):");
    if (!scriptName) return;

    const fullName = scriptName.endsWith(".js")
      ? scriptName
      : scriptName + ".js";

    // Create empty script with proper init() pattern
    const encodedScriptName = encodeURIComponent(fullName);
    fetch(`/api/scripts/${encodedScriptName}`, {
      method: "POST",
      body: `// ${fullName}
// New script created at ${new Date().toISOString()}

function handler(req) {
    return {
        status: 200,
        body: 'Hello from ${fullName}!',
        contentType: 'text/plain; charset=UTF-8'
    };
}

function init(context) {
    console.log('Initializing ${fullName} at ' + new Date().toISOString());
    register('/', 'handler', 'GET');
    console.log('${fullName} endpoints registered');
    return { success: true };
}`,
    })
      .then(() => {
        this.loadScripts();
        this.loadScript(fullName);
        this.showStatus("Script created successfully", "success");
      })
      .catch((error) => {
        this.showStatus("Error creating script: " + error.message, "error");
      });
  }

  saveCurrentScript() {
    if (!this.currentScript || !this.monacoEditor) return;

    const content = this.monacoEditor.getValue();
    const encodedScriptName = encodeURIComponent(this.currentScript);

    fetch(`/api/scripts/${encodedScriptName}`, {
      method: "POST",
      body: content,
    })
      .then(() => {
        this.showStatus("Script saved successfully", "success");
        this.updateSaveButton();
      })
      .catch((error) => {
        this.showStatus("Error saving script: " + error.message, "error");
      });
  }

  deleteCurrentScript() {
    if (!this.currentScript) return;

    if (!confirm(`Are you sure you want to delete ${this.currentScript}?`))
      return;

    const encodedScriptName = encodeURIComponent(this.currentScript);
    fetch(`/api/scripts/${encodedScriptName}`, {
      method: "DELETE",
    })
      .then(() => {
        this.currentScript = null;
        document.getElementById("current-script-name").textContent =
          "No script selected";
        document.getElementById("delete-script-btn").disabled = true;

        if (this.monacoEditor) {
          this.monacoEditor.setValue("// Select a script to edit");
        }

        this.loadScripts();
        this.showStatus("Script deleted successfully", "success");
      })
      .catch((error) => {
        this.showStatus("Error deleting script: " + error.message, "error");
      });
  }

  updateSaveButton() {
    const saveBtn = document.getElementById("save-script-btn");
    if (this.currentScript && this.monacoEditor) {
      saveBtn.disabled = false;
    } else {
      saveBtn.disabled = true;
    }
  }

  // Asset Management
  async loadAssets() {
    try {
      const response = await fetch("/api/assets");
      const data = await response.json();

      const assetsGrid = document.getElementById("assets-grid");
      assetsGrid.innerHTML = "";

      data.assets.forEach((asset) => {
        const assetElement = document.createElement("div");
        assetElement.innerHTML = this.templates["asset-item"]({
          path: asset.path,
          name: asset.name,
          size: this.formatBytes(asset.size),
          type: asset.type,
          isImage: asset.type.startsWith("image/"),
          icon: this.getFileIcon(asset.type),
        });

        // Add event listeners
        const item = assetElement.querySelector(".asset-item");
        item.querySelector(".download-btn").addEventListener("click", (e) => {
          e.stopPropagation();
          this.downloadAsset(asset.path);
        });
        item.querySelector(".delete-btn").addEventListener("click", (e) => {
          e.stopPropagation();
          this.deleteAsset(asset.path);
        });

        assetsGrid.appendChild(assetElement.firstElementChild);
      });
    } catch (error) {
      this.showStatus("Error loading assets: " + error.message, "error");
    }
  }

  triggerAssetUpload() {
    document.getElementById("asset-upload").click();
  }

  async uploadAssets(files) {
    for (const file of files) {
      try {
        const base64 = await this.fileToBase64(file);
        const publicPath = `/${file.name}`;

        await fetch("/api/assets", {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
          },
          body: JSON.stringify({
            publicPath: publicPath,
            mimetype: file.type,
            content: base64,
          }),
        });

        this.showStatus(`Uploaded ${file.name}`, "success");
      } catch (error) {
        this.showStatus(
          `Error uploading ${file.name}: ${error.message}`,
          "error",
        );
      }
    }

    this.loadAssets();
  }

  async deleteAsset(path) {
    if (!confirm(`Are you sure you want to delete ${path}?`)) return;

    try {
      await fetch(`/api/assets${path}`, {
        method: "DELETE",
      });
      this.loadAssets();
      this.showStatus("Asset deleted successfully", "success");
    } catch (error) {
      this.showStatus("Error deleting asset: " + error.message, "error");
    }
  }

  downloadAsset(path) {
    window.open(`/api/assets${path}`, "_blank");
  }

  // Logs Management
  async loadLogs() {
    try {
      const response = await fetch("/api/logs");
      const logs = await response.json();

      const logsContent = document.getElementById("logs-content");

      // Remember if user was at the bottom before refresh
      const wasAtBottom = this.isScrolledToBottom(logsContent);

      logsContent.innerHTML = "";

      // Reverse logs so newest appear at bottom
      logs.reverse().forEach((log) => {
        const logElement = document.createElement("div");
        logElement.innerHTML = this.templates["log-entry"]({
          time: new Date(log.timestamp).toLocaleTimeString(),
          level: log.level || "info",
          message: log.message,
        });
        logsContent.appendChild(logElement.firstElementChild);
      });

      // Only auto-scroll if user was already at the bottom
      if (wasAtBottom) {
        logsContent.scrollTop = logsContent.scrollHeight;
      }
    } catch (error) {
      this.showStatus("Error loading logs: " + error.message, "error");
    }
  }

  // Helper method to check if element is scrolled to bottom
  isScrolledToBottom(element) {
    // Consider "at bottom" if within 50px of the bottom
    // This accounts for rounding errors and makes it easier to stay "at bottom"
    const threshold = 50;
    return (
      element.scrollHeight - element.scrollTop - element.clientHeight <
      threshold
    );
  }

  async clearLogs() {
    // Note: This would require a backend endpoint to clear logs
    this.showStatus("Clear logs functionality not implemented yet", "warning");
  }

  // Routes Management
  async loadRoutes() {
    try {
      const response = await fetch("/api/routes");
      const routes = await response.json();

      const routesList = document.getElementById("routes-list");
      routesList.innerHTML = "";

      if (routes.length === 0) {
        routesList.innerHTML =
          '<div class="no-routes">No routes registered yet</div>';
        return;
      }

      routes.forEach((route) => {
        const routeElement = document.createElement("div");
        routeElement.innerHTML = this.templates["route-item"]({
          method: route.method,
          path: route.path,
          handler: route.handler,
        });

        // Add event listener for test button
        const testBtn = routeElement.querySelector(".test-btn");
        testBtn.addEventListener("click", () => {
          this.testRoute(route.path, route.method);
        });

        routesList.appendChild(routeElement.firstElementChild);
      });
    } catch (error) {
      this.showStatus("Error loading routes: " + error.message, "error");
    }
  }

  // Utility Methods
  testEndpoint() {
    const testUrl = prompt("Enter endpoint URL to test:", "/");
    if (!testUrl) return;

    fetch(testUrl)
      .then((response) => response.text())
      .then((data) => {
        alert(`Response: ${data}`);
      })
      .catch((error) => {
        alert(`Error: ${error.message}`);
      });
  }

  testRoute(path, method) {
    const testUrl = prompt(`Test ${method} ${path}:`, path);
    if (!testUrl) return;

    // For simplicity, we'll do a GET request regardless of the method
    // In a real implementation, you'd want to handle different HTTP methods
    fetch(testUrl)
      .then((response) => response.text())
      .then((data) => {
        alert(`Response from ${method} ${path}:\n${data}`);
      })
      .catch((error) => {
        alert(`Error testing ${method} ${path}: ${error.message}`);
      });
  }

  loadInitialData() {
    this.loadScripts();
  }

  showStatus(message, type = "info") {
    const statusElement = document.getElementById("status-message");
    statusElement.textContent = message;
    statusElement.className = `status-${type}`;

    // Clear status after 5 seconds
    setTimeout(() => {
      statusElement.textContent = "Ready";
      statusElement.className = "";
    }, 5000);
  }

  fileToBase64(file) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.readAsDataURL(file);
      reader.onload = () => resolve(reader.result.split(",")[1]);
      reader.onerror = (error) => reject(error);
    });
  }

  formatBytes(bytes) {
    if (bytes === 0) return "0 Bytes";
    const k = 1024;
    const sizes = ["Bytes", "KB", "MB", "GB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + " " + sizes[i];
  }

  getFileIcon(type) {
    if (type.startsWith("image/")) return "🖼️";
    if (type.startsWith("text/")) return "📄";
    if (type.includes("javascript")) return "📜";
    if (type.includes("json")) return "📋";
    return "📁";
  }

  // AI Assistant Methods
  toggleAIAssistant() {
    const aiAssistant = document.querySelector(".ai-assistant");
    const toggleBtn = document.getElementById("toggle-ai-assistant");

    aiAssistant.classList.toggle("collapsed");

    if (aiAssistant.classList.contains("collapsed")) {
      toggleBtn.textContent = "▲";
    } else {
      toggleBtn.textContent = "▼";
    }
  }

  async submitAIPrompt() {
    const promptInput = document.getElementById("ai-prompt");
    const responseDiv = document.getElementById("ai-response");
    const submitBtn = document.getElementById("submit-prompt-btn");

    const prompt = promptInput.value.trim();

    if (!prompt) {
      this.showStatus("Please enter a prompt", "error");
      return;
    }

    // Disable submit button and show loading state
    submitBtn.disabled = true;
    submitBtn.textContent = "Submitting...";
    responseDiv.innerHTML =
      '<p class="ai-placeholder">Processing your request...</p>';
    responseDiv.classList.add("loading");

    try {
      // Include current script context
      const requestBody = {
        prompt: prompt,
        currentScript: this.currentScript,
        currentScriptContent: this.monacoEditor
          ? this.monacoEditor.getValue()
          : null,
      };

      const response = await fetch("/api/ai-assistant", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify(requestBody),
      });

      if (!response.ok) {
        throw new Error(`HTTP error! status: ${response.status}`);
      }

      const data = await response.json();

      // Check if we got a structured response
      if (data.parsed && data.parsed.type) {
        this.handleStructuredAIResponse(data.parsed, prompt);
      } else {
        // Display plain text response
        this.displayPlainAIResponse(data.response, prompt);
      }

      this.showStatus("AI response received", "success");
    } catch (error) {
      responseDiv.classList.remove("loading");
      responseDiv.innerHTML = `
        <p style="color: var(--danger-color);">
          <strong>Error:</strong> ${this.escapeHtml(error.message)}
        </p>
      `;
      this.showStatus("Failed to get AI response", "error");
    } finally {
      // Re-enable submit button
      submitBtn.disabled = false;
      submitBtn.textContent = "Submit";
    }
  }

  displayPlainAIResponse(responseText, prompt) {
    const responseDiv = document.getElementById("ai-response");
    responseDiv.classList.remove("loading");
    responseDiv.innerHTML = `
      <div class="ai-response-content">
        <p><strong>You asked:</strong> ${this.escapeHtml(prompt)}</p>
        <hr style="border-color: var(--border-color); margin: 10px 0;">
        <div class="ai-response-text">${this.escapeHtml(responseText)}</div>
        <p style="color: var(--text-muted); font-size: 11px; margin-top: 10px;">
          ${new Date().toLocaleString()}
        </p>
      </div>
    `;
  }

  handleStructuredAIResponse(parsed, prompt) {
    const responseDiv = document.getElementById("ai-response");
    responseDiv.classList.remove("loading");

    const actionType = parsed.type;
    const message = parsed.message || "AI suggestion";
    const scriptName = parsed.script_name || "untitled.js";
    const code = parsed.code || "";
    const originalCode = parsed.original_code || "";

    // Store the data for button clicks
    this.pendingAIAction = {
      type: actionType,
      scriptName: scriptName,
      code: code,
      originalCode: originalCode,
      message: message,
    };

    let html = `
      <div class="ai-response-content">
        <p><strong>You asked:</strong> ${this.escapeHtml(prompt)}</p>
        <hr style="border-color: var(--border-color); margin: 10px 0;">
        <div class="ai-action-header">
          <span class="ai-action-type ai-action-${actionType}">${actionType.replace(/_/g, " ").toUpperCase()}</span>
        </div>
        <p><strong>AI Suggestion:</strong> ${this.escapeHtml(message)}</p>
    `;

    if (actionType === "explanation") {
      // Just show the explanation, no actions needed
      html += `</div>`;
    } else if (actionType === "create_script") {
      html += `
        <p><strong>Script Name:</strong> <code>${this.escapeHtml(scriptName)}</code></p>
        <div class="ai-code-preview">
          <pre><code>${this.escapeHtml(code.substring(0, 300))}${code.length > 300 ? "..." : ""}</code></pre>
        </div>
        <div class="ai-action-buttons">
          <button class="btn btn-success" onclick="window.editor.applyPendingAIAction()">Preview & Create</button>
        </div>
        </div>
      `;
    } else if (actionType === "edit_script") {
      html += `
        <p><strong>Script Name:</strong> <code>${this.escapeHtml(scriptName)}</code></p>
        <div class="ai-action-buttons">
          <button class="btn btn-primary" onclick="window.editor.applyPendingAIAction()">Preview Changes</button>
        </div>
        </div>
      `;
    } else if (actionType === "delete_script") {
      html += `
        <p><strong>Script Name:</strong> <code>${this.escapeHtml(scriptName)}</code></p>
        <div class="ai-action-buttons">
          <button class="btn btn-danger" onclick="window.editor.applyPendingAIAction()">Confirm Delete</button>
        </div>
        </div>
      `;
    }

    html += `
      <p style="color: var(--text-muted); font-size: 11px; margin-top: 10px;">
        ${new Date().toLocaleString()}
      </p>
    `;

    responseDiv.innerHTML = html;
  }

  async applyPendingAIAction() {
    if (!this.pendingAIAction) {
      this.showStatus("No pending action", "error");
      return;
    }

    const { type, scriptName, code, originalCode, message } =
      this.pendingAIAction;

    if (type === "create_script") {
      await this.showDiffModal(scriptName, "", code, message, "create");
    } else if (type === "edit_script") {
      await this.showDiffModal(
        scriptName,
        originalCode || "",
        code,
        message,
        "edit",
      );
    } else if (type === "delete_script") {
      this.confirmDeleteScript(scriptName, message);
    }
  }

  async showDiffModal(scriptName, originalCode, newCode, explanation, action) {
    const modal = document.getElementById("diff-modal");
    const title = document.getElementById("diff-modal-title");
    const explanationDiv = document.getElementById("diff-explanation");

    // Set title based on action
    if (action === "create") {
      title.textContent = `Create Script: ${scriptName}`;
    } else if (action === "edit") {
      title.textContent = `Edit Script: ${scriptName}`;
    }

    explanationDiv.innerHTML = `<p>${this.escapeHtml(explanation)}</p>`;

    // Show modal
    modal.style.display = "flex";

    // Create diff editor
    await this.createDiffEditor(originalCode || "", newCode);

    // Store data for apply action
    this.pendingChange = {
      scriptName: scriptName,
      newCode: newCode,
      action: action,
    };
  }

  async createDiffEditor(originalCode, newCode) {
    const container = document.getElementById("monaco-diff-editor");

    // Clear any existing content
    container.innerHTML = "";

    return new Promise((resolve) => {
      if (this.monacoDiffEditor) {
        this.monacoDiffEditor.dispose();
      }

      this.monacoDiffEditor = monaco.editor.createDiffEditor(container, {
        theme: "vs-dark",
        readOnly: true,
        automaticLayout: true,
        renderSideBySide: true,
        fontSize: 13,
      });

      const original = monaco.editor.createModel(
        originalCode || "// New file",
        "javascript",
      );
      const modified = monaco.editor.createModel(newCode, "javascript");

      this.monacoDiffEditor.setModel({
        original: original,
        modified: modified,
      });

      resolve();
    });
  }

  closeDiffModal() {
    const modal = document.getElementById("diff-modal");
    modal.style.display = "none";

    if (this.monacoDiffEditor) {
      this.monacoDiffEditor.dispose();
      this.monacoDiffEditor = null;
    }

    this.pendingChange = null;
  }

  async applyPendingChange() {
    if (!this.pendingChange) return;

    const { scriptName, newCode, action } = this.pendingChange;

    try {
      if (action === "create" || action === "edit") {
        const encodedScriptName = encodeURIComponent(scriptName);
        const response = await fetch(`/api/scripts/${encodedScriptName}`, {
          method: "POST",
          body: newCode,
        });

        if (!response.ok) {
          throw new Error(`Failed to save script: ${response.status}`);
        }

        this.showStatus(
          `Script ${action === "create" ? "created" : "updated"} successfully`,
          "success",
        );
        this.loadScripts();

        // Load the script in editor
        this.loadScript(scriptName);
      }

      this.closeDiffModal();
    } catch (error) {
      this.showStatus(`Error applying changes: ${error.message}`, "error");
    }
  }

  confirmDeleteScript(scriptName, explanation) {
    if (
      confirm(
        `${explanation}\n\nAre you sure you want to delete ${scriptName}?`,
      )
    ) {
      const encodedScriptName = encodeURIComponent(scriptName);
      fetch(`/api/scripts/${encodedScriptName}`, {
        method: "DELETE",
      })
        .then(() => {
          this.showStatus("Script deleted successfully", "success");
          this.loadScripts();

          if (this.currentScript === scriptName) {
            this.currentScript = null;
            document.getElementById("current-script-name").textContent =
              "No script selected";
            if (this.monacoEditor) {
              this.monacoEditor.setValue("// Select a script to edit");
            }
          }
        })
        .catch((error) => {
          this.showStatus("Error deleting script: " + error.message, "error");
        });
    }
  }

  clearAIPrompt() {
    const promptInput = document.getElementById("ai-prompt");
    promptInput.value = "";
    promptInput.focus();
  }

  escapeHtml(text) {
    const div = document.createElement("div");
    div.textContent = text;
    return div.innerHTML;
  }
}

// Initialize the editor when the page loads
function initEditor() {
  console.log("[Editor] initEditor() called, DOM ready");
  console.log("[Editor] Creating AIWebEngineEditor instance...");
  window.editor = new AIWebEngineEditor();
}

console.log("[Editor] Script loaded, waiting for DOMContentLoaded...");
document.addEventListener("DOMContentLoaded", initEditor);
