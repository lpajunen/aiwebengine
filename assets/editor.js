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
        contentType: 'text/plain'
    };
}

function init(context) {
    writeLog('Initializing ${fullName} at ' + new Date().toISOString());
    register('/', 'handler', 'GET');
    writeLog('${fullName} endpoints registered');
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
      logsContent.innerHTML = "";

      logs.forEach((log) => {
        const logElement = document.createElement("div");
        logElement.innerHTML = this.templates["log-entry"]({
          time: new Date(log.timestamp).toLocaleTimeString(),
          level: log.level || "info",
          message: log.message,
        });
        logsContent.appendChild(logElement.firstElementChild);
      });

      // Auto-scroll to bottom
      logsContent.scrollTop = logsContent.scrollHeight;
    } catch (error) {
      this.showStatus("Error loading logs: " + error.message, "error");
    }
  }

  async clearLogs() {
    // Note: This would require a backend endpoint to clear logs
    this.showStatus("Clear logs functionality not implemented yet", "warning");
  }

  // Routes Management
  async loadRoutes() {
    try {
      // This would need a backend endpoint to list routes
      // For now, show a placeholder
      const routesList = document.getElementById("routes-list");
      routesList.innerHTML =
        '<div class="loading">Routes listing not yet implemented</div>';
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
}

// Initialize the editor when the page loads
function initEditor() {
  console.log("[Editor] initEditor() called, DOM ready");
  console.log("[Editor] Creating AIWebEngineEditor instance...");
  window.editor = new AIWebEngineEditor();
}

console.log("[Editor] Script loaded, waiting for DOMContentLoaded...");
document.addEventListener("DOMContentLoaded", initEditor);
