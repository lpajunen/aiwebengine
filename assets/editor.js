// aiwebengine Editor - Main JavaScript
class AIWebEngineEditor {
  constructor() {
    this.currentScript = null;
    this.currentAsset = null;
    this.monacoEditor = null;
    this.monacoAssetEditor = null;
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
        <div class="asset-item ${data.active ? "active" : ""}" data-path="${data.path}">
          <div class="asset-icon">${data.icon}</div>
          <div class="asset-info">
            <div class="asset-name">${data.name}</div>
            <div class="asset-meta">${data.isText ? "text" : "binary"} • ${data.size}</div>
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
      .getElementById("new-asset-btn")
      .addEventListener("click", () => this.createNewAsset());
    document
      .getElementById("upload-asset-btn")
      .addEventListener("click", () => this.triggerAssetUpload());
    document
      .getElementById("asset-upload")
      .addEventListener("change", (e) => this.uploadAssets(e.target.files));
    document
      .getElementById("save-asset-btn")
      .addEventListener("click", () => this.saveCurrentAsset());
    document
      .getElementById("delete-asset-btn")
      .addEventListener("click", () => this.deleteCurrentAsset());

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
        // Script editor
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

        // Asset editor
        this.monacoAssetEditor = monaco.editor.create(
          document.getElementById("monaco-asset-editor"),
          {
            value: "// Select an asset to edit",
            language: "plaintext",
            theme: "vs-dark",
            fontSize: 14,
            minimap: { enabled: true },
            scrollBeyondLastLine: false,
            automaticLayout: true,
            wordWrap: "on",
          },
        );

        this.monacoAssetEditor.onDidChangeModelContent(() => {
          this.updateAssetSaveButton();
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

      const assetsList = document.getElementById("assets-list");
      assetsList.innerHTML = "";

      data.assets.forEach((asset) => {
        const assetElement = document.createElement("div");
        const isText = this.isTextAsset(asset.path);

        assetElement.innerHTML = this.templates["asset-item"]({
          path: asset.path,
          name: asset.name,
          size: this.formatBytes(asset.size),
          type: asset.type,
          isText: isText,
          icon: this.getFileIcon(asset.type, isText),
          active: this.currentAsset === asset.path,
        });

        // Add click listener to select asset
        const item = assetElement.firstElementChild;
        item.addEventListener("click", () => this.selectAsset(asset.path));

        assetsList.appendChild(item);
      });
    } catch (error) {
      this.showStatus("Error loading assets: " + error.message, "error");
    }
  }

  isTextAsset(path) {
    const textExtensions = [
      ".css",
      ".svg",
      ".json",
      ".html",
      ".md",
      ".txt",
      ".js",
      ".xml",
      ".csv",
      ".yaml",
      ".yml",
      ".toml",
      ".log",
      ".ini",
      ".conf",
      ".config",
    ];
    const ext = path.substring(path.lastIndexOf(".")).toLowerCase();
    return textExtensions.includes(ext);
  }

  getLanguageMode(path) {
    const ext = path.substring(path.lastIndexOf(".")).toLowerCase();
    const languageMap = {
      ".css": "css",
      ".svg": "xml",
      ".json": "json",
      ".html": "html",
      ".md": "markdown",
      ".txt": "plaintext",
      ".js": "javascript",
      ".xml": "xml",
      ".yaml": "yaml",
      ".yml": "yaml",
      ".toml": "ini",
      ".log": "plaintext",
      ".ini": "ini",
      ".conf": "plaintext",
      ".config": "plaintext",
    };
    return languageMap[ext] || "plaintext";
  }

  async selectAsset(path) {
    this.currentAsset = path;

    // Update active state in list
    document.querySelectorAll(".asset-item").forEach((item) => {
      item.classList.remove("active");
    });
    const activeItem = document.querySelector(`[data-path="${path}"]`);
    if (activeItem) {
      activeItem.classList.add("active");
    }

    // Update toolbar
    document.getElementById("current-asset-name").textContent = path;
    document.getElementById("save-asset-btn").disabled = false;
    document.getElementById("delete-asset-btn").disabled = false;

    const isText = this.isTextAsset(path);

    if (isText) {
      // Load text asset in Monaco editor
      try {
        const response = await fetch(`/api/assets${path}`);

        // Get the content as an ArrayBuffer first, then decode as UTF-8
        const buffer = await response.arrayBuffer();
        const decoder = new TextDecoder("utf-8");
        const content = decoder.decode(buffer);

        this.monacoAssetEditor.setValue(content);
        const language = this.getLanguageMode(path);
        monaco.editor.setModelLanguage(
          this.monacoAssetEditor.getModel(),
          language,
        );

        // Show editor, hide binary info
        document.getElementById("monaco-asset-editor").style.display = "block";
        document.getElementById("binary-asset-info").style.display = "none";
        document.getElementById("no-asset-selected").style.display = "none";
        document.getElementById("save-asset-btn").disabled = false;
      } catch (error) {
        this.showStatus("Error loading asset: " + error.message, "error");
      }
    } else {
      // Binary asset - show info panel
      this.showBinaryAssetInfo(path);
      document.getElementById("save-asset-btn").disabled = true;
    }
  }

  showBinaryAssetInfo(path) {
    const filename = path.split("/").pop();
    const ext = path.substring(path.lastIndexOf(".")).toLowerCase();

    // Hide editor, show binary info
    document.getElementById("monaco-asset-editor").style.display = "none";
    document.getElementById("no-asset-selected").style.display = "none";
    document.getElementById("binary-asset-info").style.display = "block";

    const detailsDiv = document.getElementById("binary-asset-details");
    detailsDiv.innerHTML = `
      <p><strong>File:</strong> ${filename}</p>
      <p><strong>Path:</strong> ${path}</p>
      <p><strong>Type:</strong> Binary file</p>
      <div class="binary-actions">
        <button class="btn btn-secondary" onclick="window.editor.downloadAsset('${path}')">Download</button>
      </div>
    `;

    const previewDiv = document.getElementById("binary-asset-preview");
    previewDiv.innerHTML = "";

    // Show preview for images
    const imageExtensions = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg"];
    if (imageExtensions.includes(ext)) {
      previewDiv.innerHTML = `
        <div class="image-preview">
          <img src="/api/assets${path}" alt="${filename}" style="max-width: 100%; max-height: 400px;">
        </div>
      `;
    }
  }

  createNewAsset() {
    const filename = prompt("Enter asset filename (e.g., styles/custom.css):");
    if (!filename) return;

    // Ensure it starts with /
    const path = filename.startsWith("/") ? filename : "/" + filename;

    if (!this.isTextAsset(path)) {
      alert(
        "Only text-based assets can be created. Use Upload for binary files.",
      );
      return;
    }

    // Create empty asset
    this.currentAsset = path;
    this.monacoAssetEditor.setValue("");
    const language = this.getLanguageMode(path);
    monaco.editor.setModelLanguage(this.monacoAssetEditor.getModel(), language);

    document.getElementById("current-asset-name").textContent = path + " (new)";
    document.getElementById("monaco-asset-editor").style.display = "block";
    document.getElementById("binary-asset-info").style.display = "none";
    document.getElementById("no-asset-selected").style.display = "none";
    document.getElementById("save-asset-btn").disabled = false;
    document.getElementById("delete-asset-btn").disabled = true;

    this.showStatus("Create your asset and click Save", "info");
  }

  async saveCurrentAsset() {
    if (!this.currentAsset) return;

    const content = this.monacoAssetEditor.getValue();

    try {
      // Convert content to base64 using UTF-8 safe encoding
      const base64 = this.textToBase64(content);

      // Determine MIME type from extension
      const ext = this.currentAsset
        .substring(this.currentAsset.lastIndexOf("."))
        .toLowerCase();
      const mimeTypes = {
        ".css": "text/css",
        ".svg": "image/svg+xml",
        ".json": "application/json",
        ".html": "text/html",
        ".md": "text/markdown",
        ".txt": "text/plain",
        ".js": "application/javascript",
        ".xml": "application/xml",
        ".yaml": "text/yaml",
        ".yml": "text/yaml",
      };
      const mimetype = mimeTypes[ext] || "text/plain";

      const response = await fetch("/api/assets", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          publicPath: this.currentAsset,
          mimetype: mimetype,
          content: base64,
        }),
      });

      if (!response.ok) {
        throw new Error(`Save failed with status ${response.status}`);
      }

      this.showStatus("Asset saved successfully", "success");

      // Update the display name to remove (new) if it was there
      document.getElementById("current-asset-name").textContent =
        this.currentAsset;
      document.getElementById("delete-asset-btn").disabled = false;

      // Reload assets list
      this.loadAssets();
    } catch (error) {
      this.showStatus("Error saving asset: " + error.message, "error");
    }
  }

  async deleteCurrentAsset() {
    if (!this.currentAsset) return;

    if (!confirm(`Are you sure you want to delete ${this.currentAsset}?`))
      return;

    try {
      const response = await fetch(`/api/assets${this.currentAsset}`, {
        method: "DELETE",
      });

      if (!response.ok) {
        const errorText = await response.text();
        throw new Error(
          `Delete failed with status ${response.status}: ${errorText}`,
        );
      }

      this.showStatus("Asset deleted successfully", "success");

      // Clear editor
      this.currentAsset = null;
      document.getElementById("current-asset-name").textContent =
        "No asset selected";
      document.getElementById("monaco-asset-editor").style.display = "none";
      document.getElementById("binary-asset-info").style.display = "none";
      document.getElementById("no-asset-selected").style.display = "block";
      document.getElementById("save-asset-btn").disabled = true;
      document.getElementById("delete-asset-btn").disabled = true;

      // Reload assets list
      this.loadAssets();
    } catch (error) {
      this.showStatus("Error deleting asset: " + error.message, "error");
    }
  }

  updateAssetSaveButton() {
    const saveBtn = document.getElementById("save-asset-btn");
    if (this.currentAsset && this.isTextAsset(this.currentAsset)) {
      saveBtn.disabled = false;
      saveBtn.textContent = "Save *";
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

  downloadAsset(path) {
    const filename = path.split("/").pop();
    const isIco = filename.toLowerCase().endsWith(".ico");

    console.log(`Downloading asset: ${path} (isIco: ${isIco})`);

    if (isIco) {
      // For ICO files, use fetch + blob to ensure proper binary handling
      fetch(`/api/assets${path}`)
        .then((response) => {
          console.log(`Download response status: ${response.status}`);
          if (!response.ok) {
            throw new Error(`Download failed with status ${response.status}`);
          }
          return response.blob();
        })
        .then((blob) => {
          console.log(`Blob size: ${blob.size}, type: ${blob.type}`);
          const url = URL.createObjectURL(blob);
          const a = document.createElement("a");
          a.href = url;
          a.download = filename;
          document.body.appendChild(a);
          a.click();
          document.body.removeChild(a);
          URL.revokeObjectURL(url);
          this.showStatus(`Downloaded ${filename}`, "success");
        })
        .catch((error) => {
          console.error("ICO download failed:", error);
          this.showStatus(`Download failed: ${error.message}`, "error");
          // Fallback to window.open
          window.open(`/api/assets${path}`, "_blank");
        });
    } else {
      // For other files, use the simple window.open approach
      window.open(`/api/assets${path}`, "_blank");
    }
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
          message: this.escapeHtml(log.message),
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

  // UTF-8 safe base64 encoding for text content
  textToBase64(text) {
    // Convert text to UTF-8 bytes using TextEncoder
    const encoder = new TextEncoder();
    const utf8Bytes = encoder.encode(text);

    // Convert bytes to binary string
    let binaryString = "";
    for (let i = 0; i < utf8Bytes.length; i++) {
      binaryString += String.fromCharCode(utf8Bytes[i]);
    }

    // Encode to base64
    return btoa(binaryString);
  }

  formatBytes(bytes) {
    if (bytes === 0) return "0 Bytes";
    const k = 1024;
    const sizes = ["Bytes", "KB", "MB", "GB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + " " + sizes[i];
  }

  getFileIcon(type, isText) {
    // If isText is provided, use that to determine icon
    if (isText !== undefined) {
      if (isText) {
        // Text-based files
        if (type.includes("css")) return "🎨";
        if (type.includes("svg") || type.includes("xml")) return "🖼️";
        if (type.includes("json")) return "📋";
        if (type.includes("html")) return "📄";
        if (type.includes("markdown")) return "📝";
        if (type.includes("javascript")) return "📜";
        return "📄";
      } else {
        // Binary files
        if (type === "image/x-icon") return "⭐";
        if (type.startsWith("image/")) return "🖼️";
        if (type.includes("font")) return "🔤";
        return "📦";
      }
    }

    // Fallback to original logic
    if (type === "image/x-icon") return "⭐";
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
      // Include current context (script or asset)
      const requestBody = {
        prompt: prompt,
        currentScript: this.currentScript,
        currentScriptContent: this.monacoEditor
          ? this.monacoEditor.getValue()
          : null,
        currentAsset: this.currentAsset,
        currentAssetContent:
          this.monacoAssetEditor &&
          this.currentAsset &&
          this.isTextAsset(this.currentAsset)
            ? this.monacoAssetEditor.getValue()
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
    const assetPath = parsed.asset_path || parsed.asset_name || null;
    const code = parsed.code || "";
    const originalCode = parsed.original_code || "";

    // Store the data for button clicks
    this.pendingAIAction = {
      type: actionType,
      scriptName: scriptName,
      assetPath: assetPath,
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
    } else if (actionType === "create_asset") {
      html += `
        <p><strong>Asset Path:</strong> <code>${this.escapeHtml(assetPath)}</code></p>
        <div class="ai-code-preview">
          <pre><code>${this.escapeHtml(code.substring(0, 300))}${code.length > 300 ? "..." : ""}</code></pre>
        </div>
        <div class="ai-action-buttons">
          <button class="btn btn-success" onclick="window.editor.applyPendingAIAction()">Preview & Create</button>
        </div>
        </div>
      `;
    } else if (actionType === "edit_asset") {
      html += `
        <p><strong>Asset Path:</strong> <code>${this.escapeHtml(assetPath)}</code></p>
        <div class="ai-action-buttons">
          <button class="btn btn-primary" onclick="window.editor.applyPendingAIAction()">Preview Changes</button>
        </div>
        </div>
      `;
    } else if (actionType === "delete_asset") {
      html += `
        <p><strong>Asset Path:</strong> <code>${this.escapeHtml(assetPath)}</code></p>
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

    const { type, scriptName, assetPath, code, originalCode, message } =
      this.pendingAIAction;

    if (type === "create_script") {
      await this.showDiffModal(
        scriptName,
        "",
        code,
        message,
        "create",
        "script",
      );
    } else if (type === "edit_script") {
      await this.showDiffModal(
        scriptName,
        originalCode || "",
        code,
        message,
        "edit",
        "script",
      );
    } else if (type === "delete_script") {
      this.confirmDeleteScript(scriptName, message);
    } else if (type === "create_asset") {
      await this.showDiffModal(assetPath, "", code, message, "create", "asset");
    } else if (type === "edit_asset") {
      await this.showDiffModal(
        assetPath,
        originalCode || "",
        code,
        message,
        "edit",
        "asset",
      );
    } else if (type === "delete_asset") {
      this.confirmDeleteAsset(assetPath, message);
    }
  }

  async showDiffModal(
    name,
    originalCode,
    newCode,
    explanation,
    action,
    contentType,
  ) {
    const modal = document.getElementById("diff-modal");
    const title = document.getElementById("diff-modal-title");
    const explanationDiv = document.getElementById("diff-explanation");

    // Set title based on action and type
    const typeLabel = contentType === "asset" ? "Asset" : "Script";
    if (action === "create") {
      title.textContent = `Create ${typeLabel}: ${name}`;
    } else if (action === "edit") {
      title.textContent = `Edit ${typeLabel}: ${name}`;
    }

    explanationDiv.innerHTML = `<p>${this.escapeHtml(explanation)}</p>`;

    // Show modal
    modal.style.display = "flex";

    // Determine language mode based on content type
    let language = "javascript";
    if (contentType === "asset") {
      language = this.getLanguageMode(name);
    }

    // Create diff editor
    await this.createDiffEditor(originalCode || "", newCode, language);

    // Store data for apply action
    this.pendingChange = {
      name: name,
      newCode: newCode,
      action: action,
      contentType: contentType,
    };
  }

  async createDiffEditor(originalCode, newCode, language = "javascript") {
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
        language,
      );
      const modified = monaco.editor.createModel(newCode, language);

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

    const { name, newCode, action, contentType } = this.pendingChange;

    try {
      if (contentType === "asset") {
        // Handle asset creation/editing
        if (action === "create" || action === "edit") {
          // Convert content to base64 using UTF-8 safe encoding
          const base64 = this.textToBase64(newCode);

          // Determine MIME type from extension
          const ext = name.substring(name.lastIndexOf(".")).toLowerCase();
          const mimeTypes = {
            ".css": "text/css",
            ".svg": "image/svg+xml",
            ".json": "application/json",
            ".html": "text/html",
            ".md": "text/markdown",
            ".txt": "text/plain",
            ".js": "application/javascript",
            ".xml": "application/xml",
            ".yaml": "text/yaml",
            ".yml": "text/yaml",
          };
          const mimetype = mimeTypes[ext] || "text/plain";

          const response = await fetch("/api/assets", {
            method: "POST",
            headers: {
              "Content-Type": "application/json",
            },
            body: JSON.stringify({
              publicPath: name,
              mimetype: mimetype,
              content: base64,
            }),
          });

          if (!response.ok) {
            throw new Error(`Failed to save asset: ${response.status}`);
          }

          this.showStatus(
            `Asset ${action === "create" ? "created" : "updated"} successfully`,
            "success",
          );
          this.loadAssets();

          // Switch to Assets tab and set the content directly
          this.switchTab("assets");

          // Set the asset directly from newCode instead of loading from server
          // This avoids any server-side encoding issues
          this.currentAsset = name;

          // Update active state in list
          document.querySelectorAll(".asset-item").forEach((item) => {
            item.classList.remove("active");
          });

          // Update toolbar
          document.getElementById("current-asset-name").textContent = name;
          document.getElementById("save-asset-btn").disabled = false;
          document.getElementById("delete-asset-btn").disabled = false;

          // Set content directly from newCode
          this.monacoAssetEditor.setValue(newCode);
          const language = this.getLanguageMode(name);
          monaco.editor.setModelLanguage(
            this.monacoAssetEditor.getModel(),
            language,
          );

          // Show editor
          document.getElementById("monaco-asset-editor").style.display =
            "block";
          document.getElementById("binary-asset-info").style.display = "none";
          document.getElementById("no-asset-selected").style.display = "none";

          // Wait a bit for the list to reload, then update the active item
          setTimeout(() => {
            const activeItem = document.querySelector(`[data-path="${name}"]`);
            if (activeItem) {
              activeItem.classList.add("active");
            }
          }, 100);
        }
      } else {
        // Handle script creation/editing (existing logic)
        if (action === "create" || action === "edit") {
          const encodedScriptName = encodeURIComponent(name);
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
          this.loadScript(name);
        }
      }

      this.closeDiffModal();
    } catch (error) {
      this.showStatus(`Error applying changes: ${error.message}`, "error");
    }
  }

  confirmDeleteAsset(assetPath, explanation) {
    if (
      confirm(`${explanation}\n\nAre you sure you want to delete ${assetPath}?`)
    ) {
      fetch(`/api/assets${assetPath}`, {
        method: "DELETE",
      })
        .then(() => {
          this.showStatus("Asset deleted successfully", "success");
          this.loadAssets();

          if (this.currentAsset === assetPath) {
            this.currentAsset = null;
            document.getElementById("current-asset-name").textContent =
              "No asset selected";
            document.getElementById("monaco-asset-editor").style.display =
              "none";
            document.getElementById("binary-asset-info").style.display = "none";
            document.getElementById("no-asset-selected").style.display =
              "block";
          }
        })
        .catch((error) => {
          this.showStatus("Error deleting asset: " + error.message, "error");
        });
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
