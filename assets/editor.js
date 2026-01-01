// aiwebengine Editor - Main JavaScript

/**
 * @typedef {Object} ScriptData
 * @property {string} uri
 * @property {string} content
 * @property {boolean} privileged
 * @property {boolean} [defaultPrivileged]
 * @property {string[]} owners
 * @property {boolean} [isOwner]
 * @property {number} [ownerCount]
 * @property {string} [displayName]
 * @property {number} [size]
 */

/**
 * @typedef {Object} SecurityProfile
 * @property {boolean} privileged
 * @property {boolean} [defaultPrivileged]
 * @property {string[]} [owners]
 * @property {boolean} [isOwner]
 * @property {number} [ownerCount]
 */

class AIWebEngineEditor {
  constructor() {
    /** @type {string | null} */
    this.currentScript = null;
    /** @type {string | null} */
    this.currentAsset = null;
    /** @type {ScriptData[]} */
    this.scriptsData = [];
    /** @type {Object.<string, SecurityProfile>} */
    this.scriptSecurityProfiles = {};
    /** @type {any} */
    this.monacoEditor = null;
    /** @type {any} */
    this.monacoAssetEditor = null;
    /** @type {Object.<string, Function>} */
    this.templates = {};
    this.monacoEditor = null;
    this.monacoAssetEditor = null;
    this.templates = {};
    this.scriptSecurityProfiles = {};
    this.permissions = { canTogglePrivileged: false };
    this.currentFilter = "all";
    this.scriptsData = [];
    this.currentUserId = null;
    this.init();
  }

  // ============================================================================
  // Helper Utilities for Type-Safe DOM Access
  // ============================================================================

  /**
   * Get element by ID with null check
   * @param {string} id
   * @returns {HTMLElement | null}
   */
  getElement(id) {
    return document.getElementById(id);
  }

  /**
   * Get button element by ID
   * @param {string} id
   * @returns {HTMLButtonElement | null}
   */
  getButton(id) {
    return /** @type {HTMLButtonElement | null} */ (
      document.getElementById(id)
    );
  }

  /**
   * Get input element by ID
   * @param {string} id
   * @returns {HTMLInputElement | null}
   */
  getInput(id) {
    return /** @type {HTMLInputElement | null} */ (document.getElementById(id));
  }

  /**
   * Get select element by ID
   * @param {string} id
   * @returns {HTMLSelectElement | null}
   */
  getSelect(id) {
    return /** @type {HTMLSelectElement | null} */ (
      document.getElementById(id)
    );
  }

  /**
   * Set text content safely
   * @param {string} id
   * @param {string} text
   */
  setText(id, text) {
    const elem = this.getElement(id);
    if (elem) {
      elem.textContent = text;
    }
  }

  /**
   * Set disabled state on button
   * @param {string} id
   * @param {boolean} disabled
   */
  setDisabled(id, disabled) {
    const btn = this.getButton(id);
    if (btn) {
      btn.disabled = disabled;
    }
  }

  /**
   * Add event listener with null check
   * @param {string} id
   * @param {string} event
   * @param {EventListener} handler
   */
  addListener(id, event, handler) {
    const elem = this.getElement(id);
    if (elem) {
      elem.addEventListener(event, handler);
    }
  }

  /**
   * Set element display style
   * @param {string} id
   * @param {string} display
   */
  setDisplay(id, display) {
    const elem = this.getElement(id);
    if (elem) {
      elem.style.display = display;
    }
  }

  // ============================================================================
  // Initialization
  // ============================================================================

  async init() {
    console.log("[Editor] Starting initialization...");
    this.compileTemplates();
    console.log("[Editor] Templates compiled");
    this.setupEventListeners();
    this.renderScriptSecurity(null);
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
      /** @param {any} data */
      "script-item": (data) => {
        const ownerClass =
          data.ownerCount === 0
            ? "system-script"
            : data.isOwner
              ? "owned"
              : "unowned";
        const ownerBadge =
          data.ownerCount === 0
            ? '<span class="owner-badge system">SYSTEM</span>'
            : data.isOwner
              ? '<span class="owner-badge">MINE</span>'
              : "";
        return `
          <div class="script-item ${data.active ? "active" : ""} ${ownerClass}" data-script="${data.uri}" data-owner-count="${data.ownerCount}" data-is-owner="${data.isOwner}" title="${data.uri}">
            <div class="script-icon">📄</div>
            <div class="script-info">
              <div class="script-item-header">
                <div class="script-item-name">${data.displayName}</div>
                <div class="script-item-badges">
                  ${ownerBadge}
                  <span class="privileged-pill ${data.privileged ? "privileged" : "restricted"}">
                    ${data.privileged ? "P" : "R"}
                  </span>
                </div>
              </div>
              <div class="script-meta">
                <span class="script-size">${data.size} bytes</span>
                ${data.ownerCount > 0 ? `<span class="script-owners">👥 ${data.ownerCount}</span>` : ""}
              </div>
            </div>
          </div>
        `;
      },
      /** @param {any} data */
      "asset-item": (data) => `
        <div class="asset-item ${data.active ? "active" : ""}" data-path="${data.uri}" title="${data.uri}">
          <div class="asset-icon">${data.icon}</div>
          <div class="asset-info">
            <div class="asset-name">${data.displayName}</div>
            <div class="asset-meta">${data.isText ? "text" : "binary"} • ${data.size}</div>
          </div>
        </div>
      `,
      /** @param {any} data */
      "log-entry": (data) => `
        <div class="log-entry log-${data.level}">
          <span class="log-time">${data.time}</span>
          <span class="log-level">${data.level}</span>
          <span class="log-message">${data.message}</span>
        </div>
      `,
      /** @param {any} data */
      "route-item": (data) => `
        <div class="route-item">
          <div class="route-method ${data.method}">${data.method}</div>
          <div class="route-path">${data.path}</div>
          <div class="route-handler">
            <div class="handler-function">${data.handler}</div>
            <div class="handler-script">${data.script_uri || ""}</div>
          </div>
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
      tab.addEventListener("click", (e) => {
        const target = /** @type {HTMLElement} */ (e.target);
        if (target && target.dataset.tab) {
          this.switchTab(target.dataset.tab);
        }
      });
    });

    // Script management
    this.addListener("new-script-btn", "click", () => this.createNewScript());
    this.addListener("save-script-btn", "click", () =>
      this.saveCurrentScript(),
    );
    this.addListener("delete-script-btn", "click", () =>
      this.deleteCurrentScript(),
    );
    this.addListener("toggle-privileged-btn", "click", () =>
      this.togglePrivilegedFlag(),
    );
    this.addListener("manage-owners-btn", "click", () =>
      this.showManageOwnersModal(),
    );

    const scriptsFilter = this.getSelect("scripts-filter-select");
    if (scriptsFilter) {
      scriptsFilter.addEventListener("change", (e) => {
        const target = /** @type {HTMLSelectElement} */ (e.target);
        if (target) {
          this.filterScripts(target.value);
        }
      });
    }

    // Asset management - script selector
    const assetsScriptSelect = this.getSelect("assets-script-select");
    if (assetsScriptSelect) {
      assetsScriptSelect.addEventListener("change", (e) => {
        const target = /** @type {HTMLSelectElement} */ (e.target);
        if (target) {
          this.selectedAssetScript = target.value;
          this.currentAsset = null; // Clear current asset when switching scripts
          // Reset the asset editor view
          this.clearAssetEditor();
          this.loadAssets();
        }
      });
    }

    // Asset management
    this.addListener("new-asset-btn", "click", () => this.createNewAsset());
    this.addListener("upload-asset-btn", "click", () =>
      this.triggerAssetUpload(),
    );

    const assetUpload = this.getInput("asset-upload");
    if (assetUpload) {
      assetUpload.addEventListener("change", (e) => {
        const target = /** @type {HTMLInputElement} */ (e.target);
        if (target && target.files) {
          this.uploadAssets(target.files);
        }
      });
    }

    this.addListener("save-asset-btn", "click", () => this.saveCurrentAsset());
    this.addListener("delete-asset-btn", "click", () =>
      this.deleteCurrentAsset(),
    );

    // Logs - Jump to Latest (guarded event listeners)
    this.addListener("refresh-logs-btn", "click", () =>
      this.jumpToLatestLogs(),
    );
    this.addListener("clear-logs-btn", "click", () => this.clearLogs());

    // Routes
    this.addListener("refresh-routes-btn", "click", () => this.loadRoutes());

    // Test endpoint
    this.addListener("test-endpoint-btn", "click", () => this.testEndpoint());

    // AI Assistant
    this.addListener("toggle-ai-assistant", "click", () =>
      this.toggleAIAssistant(),
    );
    this.addListener("submit-prompt-btn", "click", () => this.submitAIPrompt());
    this.addListener("clear-prompt-btn", "click", () => this.clearAIPrompt());

    // Allow Enter key to submit (Shift+Enter for new line)
    const aiPrompt = this.getElement("ai-prompt");
    if (aiPrompt) {
      aiPrompt.addEventListener("keydown", (e) => {
        if (e.key === "Enter" && !e.shiftKey) {
          e.preventDefault();
          this.submitAIPrompt();
        }
      });
    }

    // Diff modal controls
    this.addListener("close-diff-modal", "click", () => this.closeDiffModal());
    this.addListener("reject-changes-btn", "click", () =>
      this.closeDiffModal(),
    );
    this.addListener("apply-changes-btn", "click", () =>
      this.applyPendingChange(),
    );
  }

  /** @returns {Promise<void>} */
  async setupMonacoEditor() {
    // Load Monaco Editor
    return new Promise((resolve) => {
      // @ts-ignore - require is loaded via AMD script tag
      require.config({
        paths: { vs: "https://unpkg.com/monaco-editor@0.45.0/min/vs" },
      });

      // @ts-ignore - AMD require is loaded via script tag
      require(["vs/editor/editor.main"], () => {
        // @ts-ignore - monaco is global from AMD module
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
        // @ts-ignore - monaco is global from AMD module
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

        resolve(undefined);
      });
    });
  }

  /**
   * @param {string} tabName
   */
  switchTab(tabName) {
    // Update navigation
    document
      .querySelectorAll(".nav-tab")
      .forEach((tab) => tab.classList.remove("active"));
    const navTab = document.querySelector(`[data-tab="${tabName}"]`);
    if (navTab) {
      navTab.classList.add("active");
    }

    // Update content
    document
      .querySelectorAll(".tab-content")
      .forEach((content) => content.classList.remove("active"));
    const tabContent = document.getElementById(`${tabName}-tab`);
    if (tabContent) {
      tabContent.classList.add("active");
    }

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
      const payload = await response.json();
      const scripts = Array.isArray(payload) ? payload : payload.scripts || [];
      const permissions = Array.isArray(payload)
        ? this.permissions
        : payload.permissions || {};

      this.permissions = {
        canTogglePrivileged: !!permissions.canTogglePrivileged,
      };
      this.scriptSecurityProfiles = {};
      this.scriptsData = scripts;
      console.log("[Editor] Loaded scripts:", scripts);

      scripts.forEach((/** @type {any} */ script) => {
        const scriptUri = script.uri || script.name;
        this.scriptSecurityProfiles[scriptUri] = {
          privileged: !!script.privileged,
          defaultPrivileged: !!script.defaultPrivileged,
          owners: script.owners || [],
          isOwner: !!script.isOwner,
          ownerCount: script.ownerCount || 0,
        };
      });

      this.renderScripts();
      this.renderScriptSecurity(this.currentScript);
    } catch (error) {
      const err = /** @type {Error} */ (error);
      this.showStatus("Error loading scripts: " + err.message, "error");
    }
  }

  renderScripts() {
    const scriptsList = document.getElementById("scripts-list");
    if (!scriptsList) return;

    scriptsList.innerHTML = "";

    const filteredScripts = this.getFilteredScripts();

    filteredScripts.forEach((/** @type {any} */ script) => {
      const scriptUri = script.uri || script.name;
      const scriptElement = document.createElement("div");
      scriptElement.innerHTML = this.templates["script-item"]({
        uri: scriptUri,
        displayName: script.displayName || scriptUri,
        size: script.size || 0,
        active: scriptUri === this.currentScript,
        privileged: !!script.privileged,
        owners: script.owners || [],
        isOwner: !!script.isOwner,
        ownerCount: script.ownerCount || 0,
      });

      const scriptItem = scriptElement.querySelector(".script-item");
      if (scriptItem) {
        scriptItem.addEventListener("click", () => {
          this.loadScript(scriptUri);
        });
      }

      const firstChild = scriptElement.firstElementChild;
      if (firstChild) {
        scriptsList.appendChild(firstChild);
      }
    });
  }

  getFilteredScripts() {
    switch (this.currentFilter) {
      case "mine":
        return this.scriptsData.filter((s) => s.isOwner);
      case "system":
        return this.scriptsData.filter((s) => s.ownerCount === 0);
      case "unowned":
        return this.scriptsData.filter((s) => s.ownerCount === 0);
      case "all":
      default:
        return this.scriptsData;
    }
  }

  /**
   * @param {string} filter
   */
  filterScripts(filter) {
    this.currentFilter = filter;
    this.renderScripts();
  }

  /**
   * @param {string} scriptName
   */
  async loadScript(scriptName) {
    console.log("[Editor] loadScript() called for:", scriptName);
    try {
      const encodedScriptName = encodeURIComponent(scriptName);
      const response = await fetch(`/api/scripts/${encodedScriptName}`);
      console.log("[Editor] loadScript response status:", response.status);
      const content = await response.text();
      console.log("[Editor] Script content length:", content.length);

      this.currentScript = scriptName;
      this.setText("current-script-name", scriptName);

      if (this.monacoEditor) {
        console.log("[Editor] Setting Monaco editor value...");
        this.monacoEditor.setValue(content);
        this.updateSaveButton();
      } else {
        console.error("[Editor] Monaco editor not available!");
      }

      // Update active state in list
      document.querySelectorAll(".script-item").forEach((item) => {
        const htmlItem = /** @type {HTMLElement} */ (item);
        htmlItem.classList.toggle(
          "active",
          htmlItem.dataset.script === scriptName,
        );
      });

      this.setDisabled("delete-script-btn", false);

      this.renderScriptSecurity(scriptName);
      this.renderScriptOwnership(scriptName);
    } catch (error) {
      const err = /** @type {Error} */ (error);
      this.showStatus("Error loading script: " + err.message, "error");
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
    routeRegistry.registerRoute('/', 'handler', 'GET');
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
        const err = /** @type {Error} */ (error);
        this.showStatus("Error creating script: " + err.message, "error");
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
        const err = /** @type {Error} */ (error);
        this.showStatus("Error saving script: " + err.message, "error");
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
        this.setText("current-script-name", "No script selected");
        this.setDisabled("delete-script-btn", true);

        if (this.monacoEditor) {
          this.monacoEditor.setValue("// Select a script to edit");
        }

        this.loadScripts();
        this.renderScriptSecurity(null);
        this.showStatus("Script deleted successfully", "success");
      })
      .catch((error) => {
        const err = /** @type {Error} */ (error);
        this.showStatus("Error deleting script: " + err.message, "error");
      });
  }

  updateSaveButton() {
    const hasScript = this.currentScript && this.monacoEditor;
    this.setDisabled("save-script-btn", !hasScript);
  }

  /**
   * @param {string | null} scriptName
   */
  renderScriptSecurity(scriptName) {
    const badge = this.getElement("script-privileged-badge");
    const toggleBtn = this.getButton("toggle-privileged-btn");

    if (!badge || !toggleBtn) {
      return;
    }

    if (!scriptName || !this.scriptSecurityProfiles[scriptName]) {
      badge.textContent = "No script selected";
      badge.className = "privileged-badge neutral";
      toggleBtn.textContent = "Toggle Privileged";
      toggleBtn.disabled = true;
      return;
    }

    const profile = this.scriptSecurityProfiles[scriptName];
    const privileged = !!profile.privileged;

    badge.textContent = privileged ? "Privileged script" : "Restricted script";
    badge.className = `privileged-badge ${privileged ? "privileged" : "restricted"}`;

    if (this.permissions.canTogglePrivileged) {
      toggleBtn.disabled = false;
      toggleBtn.textContent = privileged
        ? "Revoke Privileged Access"
        : "Grant Privileged Access";
    } else {
      toggleBtn.disabled = true;
      toggleBtn.textContent = "Admin only";
    }
  }

  async togglePrivilegedFlag() {
    if (!this.currentScript || !this.permissions.canTogglePrivileged) {
      return;
    }

    const profile = this.scriptSecurityProfiles[this.currentScript];
    const nextValue = !(profile && profile.privileged);
    const encoded = encodeURIComponent(this.currentScript);

    try {
      const response = await fetch(`/api/script-security/${encoded}`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ privileged: nextValue }),
      });

      if (!response.ok) {
        let message = `Failed to update privilege (status ${response.status})`;
        try {
          const errorBody = await response.json();
          if (errorBody && errorBody.error) {
            message = errorBody.error;
          }
        } catch (err) {
          console.warn("[Editor] Failed to parse privilege error:", err);
        }
        throw new Error(message);
      }

      if (!this.scriptSecurityProfiles[this.currentScript]) {
        this.scriptSecurityProfiles[this.currentScript] = {
          privileged: nextValue,
          defaultPrivileged: false,
        };
      } else {
        this.scriptSecurityProfiles[this.currentScript].privileged = nextValue;
      }

      this.showStatus(
        `Script ${this.currentScript} is now ${nextValue ? "privileged" : "restricted"}`,
        "success",
      );
      this.renderScriptSecurity(this.currentScript);
      this.loadScripts();
    } catch (error) {
      const err = /** @type {Error} */ (error);
      this.showStatus("Error updating privilege: " + err.message, "error");
    }
  }

  /**
   * @param {string | null} scriptName
   */
  renderScriptOwnership(scriptName) {
    const ownersList = document.getElementById("script-owners-list");
    const manageBtn = /** @type {HTMLButtonElement | null} */ (
      document.getElementById("manage-owners-btn")
    );

    if (!ownersList || !manageBtn) {
      return;
    }

    if (!scriptName || !this.scriptSecurityProfiles[scriptName]) {
      ownersList.textContent = "-";
      manageBtn.disabled = true;
      return;
    }

    const profile = this.scriptSecurityProfiles[scriptName];
    const owners = profile.owners || [];

    if (owners.length === 0) {
      ownersList.textContent = "System (no owners)";
      manageBtn.disabled = true;
    } else {
      ownersList.textContent = owners.join(", ");
      // Enable manage button if user is owner or admin
      manageBtn.disabled =
        !profile.isOwner && !this.permissions.canTogglePrivileged;
    }
  }

  async showManageOwnersModal() {
    if (!this.currentScript) return;

    const profile = this.scriptSecurityProfiles[this.currentScript];
    const owners = profile.owners || [];

    // Create modal
    const modal = document.createElement("div");
    modal.className = "modal-overlay";
    modal.innerHTML = `
      <div class="modal-content">
        <div class="modal-header">
          <h3 class="modal-title">Manage Script Owners</h3>
          <button class="modal-close">&times;</button>
        </div>
        <div class="modal-body">
          <p><strong>Script:</strong> ${this.currentScript}</p>
          <h4>Current Owners</h4>
          <ul class="owner-list" id="owner-list">
            ${
              owners.length === 0
                ? "<li>No owners (system script)</li>"
                : owners
                    .map(
                      (owner) => `
              <li class="owner-item">
                <span class="owner-name">${owner}</span>
                <button class="owner-remove-btn" data-owner="${owner}" ${owners.length === 1 ? 'disabled title="Cannot remove last owner"' : ""}>Remove</button>
              </li>
            `,
                    )
                    .join("")
            }
          </ul>
          <div class="add-owner-section">
            <h4>Add Owner</h4>
            <div class="add-owner-form">
              <input type="text" class="add-owner-input" id="new-owner-input" placeholder="Enter user ID">
              <button class="add-owner-btn" id="add-owner-btn">Add</button>
            </div>
          </div>
        </div>
        <div class="modal-footer">
          <button class="btn btn-secondary modal-close-btn">Close</button>
        </div>
      </div>
    `;

    document.body.appendChild(modal);

    // Event listeners
    modal.querySelectorAll(".modal-close, .modal-close-btn").forEach((btn) => {
      btn.addEventListener("click", () => modal.remove());
    });

    modal.addEventListener("click", (e) => {
      if (e.target === modal) modal.remove();
    });

    // Remove owner buttons
    modal.querySelectorAll(".owner-remove-btn").forEach((btn) => {
      btn.addEventListener("click", async (e) => {
        const target = /** @type {HTMLElement} */ (e.target);
        const owner = target.dataset.owner;
        if (owner) {
          await this.removeScriptOwner(owner);
          modal.remove();
          await this.loadScripts();
          this.renderScriptOwnership(this.currentScript);
        }
      });
    });

    // Add owner button
    const addOwnerBtn = modal.querySelector("#add-owner-btn");
    const newOwnerInput = /** @type {HTMLInputElement | null} */ (
      modal.querySelector("#new-owner-input")
    );

    if (addOwnerBtn && newOwnerInput) {
      addOwnerBtn.addEventListener("click", async () => {
        const newOwner = newOwnerInput.value.trim();
        if (newOwner) {
          await this.addScriptOwner(newOwner);
          modal.remove();
          await this.loadScripts();
          this.renderScriptOwnership(this.currentScript);
        }
      });
    }
  }

  /**
   * @param {string} ownerId
   */
  async addScriptOwner(ownerId) {
    if (!this.currentScript) return;

    try {
      const encodedScript = encodeURIComponent(this.currentScript);
      const response = await fetch(`/api/script-owners/${encodedScript}`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ ownerId }),
      });

      if (response.ok) {
        this.showStatus(`Added owner: ${ownerId}`, "success");
      } else {
        const error = await response.json();
        this.showStatus(
          `Error: ${error.error || "Failed to add owner"}`,
          "error",
        );
      }
    } catch (error) {
      const err = /** @type {Error} */ (error);
      this.showStatus("Error adding owner: " + err.message, "error");
    }
  }

  /**
   * @param {string} ownerId
   */
  async removeScriptOwner(ownerId) {
    if (!this.currentScript) return;

    try {
      const encodedScript = encodeURIComponent(this.currentScript);
      const response = await fetch(`/api/script-owners/${encodedScript}`, {
        method: "DELETE",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ ownerId }),
      });

      if (response.ok) {
        this.showStatus(`Removed owner: ${ownerId}`, "success");
      } else {
        const error = await response.json();
        this.showStatus(
          `Error: ${error.error || "Failed to remove owner"}`,
          "error",
        );
      }
    } catch (error) {
      const err = /** @type {Error} */ (error);
      this.showStatus("Error removing owner: " + err.message, "error");
    }
  }

  // Asset Management
  clearAssetEditor() {
    this.currentAsset = null;
    this.setText("current-asset-name", "No asset selected");
    this.setDisplay("monaco-asset-editor", "none");
    this.setDisplay("binary-asset-info", "none");
    this.setDisplay("no-asset-selected", "block");
    this.setDisabled("save-asset-btn", true);
    this.setDisabled("delete-asset-btn", true);
  }

  async loadAssets() {
    try {
      const assetsList = document.getElementById("assets-list");
      if (!assetsList) return;

      // Check if a script is selected
      if (!this.selectedAssetScript) {
        assetsList.innerHTML =
          '<div class="no-selection"><p>Select a script to view its assets</p></div>';
        return;
      }

      // Build URL with script URI parameter
      const url = `/api/assets?uri=${encodeURIComponent(this.selectedAssetScript)}`;
      const response = await fetch(url);
      const data = await response.json();

      assetsList.innerHTML = "";

      if (data.assets.length === 0) {
        assetsList.innerHTML =
          '<div class="no-selection"><p>No assets found for this script</p></div>';
        return;
      }

      data.assets.forEach(
        /** @param {any} asset */
        (asset) => {
          const assetElement = document.createElement("div");
          const assetUri = asset.uri || asset.path;
          const isText = this.isTextAsset(assetUri);

          assetElement.innerHTML = this.templates["asset-item"]({
            uri: assetUri,
            displayName: asset.displayName || assetUri,
            size: this.formatBytes(asset.size),
            type: asset.type,
            isText: isText,
            icon: this.getFileIcon(asset.type, isText),
            active: this.currentAsset === assetUri,
          });

          // Add click listener to select asset
          const item = assetElement.firstElementChild;
          if (item) {
            item.addEventListener("click", () => this.selectAsset(assetUri));
            assetsList.appendChild(item);
          }
        },
      );
    } catch (error) {
      const err = /** @type {Error} */ (error);
      this.showStatus("Error loading assets: " + err.message, "error");
    }
  }

  /**
   * @param {string} path
   */
  isTextAsset(path) {
    const textExtensions = [
      ".css",
      ".svg",
      ".json",
      ".html",
      ".md",
      ".txt",
      ".js",
      ".ts",
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

  /**
   * @param {string} path
   */
  getLanguageMode(path) {
    const ext = path.substring(path.lastIndexOf(".")).toLowerCase();
    /** @type {Record<string, string>} */
    const languageMap = {
      ".css": "css",
      ".svg": "xml",
      ".json": "json",
      ".html": "html",
      ".md": "markdown",
      ".txt": "plaintext",
      ".js": "javascript",
      ".ts": "typescript",
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

  /**
   * @param {string} path
   */
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
    this.setText("current-asset-name", path);
    this.setDisabled("save-asset-btn", false);
    this.setDisabled("delete-asset-btn", false);

    const isText = this.isTextAsset(path);

    if (isText) {
      // Load text asset in Monaco editor
      try {
        // Build URL with script URI parameter if selected
        let url = `/api/assets/${path}`;
        if (this.selectedAssetScript) {
          url += `?uri=${encodeURIComponent(this.selectedAssetScript)}`;
        }

        const response = await fetch(url);
        const content = await response.text();

        this.monacoAssetEditor.setValue(content);
        const language = this.getLanguageMode(path);
        // @ts-ignore - monaco is loaded via AMD
        // @ts-ignore - monaco is loaded via AMD
        monaco.editor.setModelLanguage(
          this.monacoAssetEditor.getModel(),
          language,
        );

        // Show editor, hide binary info
        this.setDisplay("monaco-asset-editor", "block");
        this.setDisplay("binary-asset-info", "none");
        this.setDisplay("no-asset-selected", "none");
        this.setDisabled("save-asset-btn", false);
      } catch (error) {
        const err = /** @type {Error} */ (error);
        this.showStatus("Error loading asset: " + err.message, "error");
      }
    } else {
      // Binary asset - show info panel
      this.showBinaryAssetInfo(path);
      this.setDisabled("save-asset-btn", true);
    }
  }

  /**
   * @param {string} path
   */
  showBinaryAssetInfo(path) {
    const filename = path.split("/").pop();
    const ext = path.substring(path.lastIndexOf(".")).toLowerCase();

    // Hide editor, show binary info
    this.setDisplay("monaco-asset-editor", "none");
    this.setDisplay("no-asset-selected", "none");
    this.setDisplay("binary-asset-info", "block");

    const detailsDiv = document.getElementById("binary-asset-details");
    if (detailsDiv) {
      detailsDiv.innerHTML = `
        <p><strong>File:</strong> ${filename}</p>
        <p><strong>Path:</strong> ${path}</p>
        <p><strong>Type:</strong> Binary file</p>
        <div class="binary-actions">
          <button class="btn btn-secondary" onclick="window.editor.downloadAsset('${path}')">Download</button>
        </div>
      `;
    }

    const previewDiv = document.getElementById("binary-asset-preview");
    if (previewDiv) {
      previewDiv.innerHTML = "";

      // Show preview for images
      const imageExtensions = [
        ".png",
        ".jpg",
        ".jpeg",
        ".gif",
        ".webp",
        ".svg",
      ];
      if (imageExtensions.includes(ext)) {
        previewDiv.innerHTML = `
          <div class="image-preview">
            <img src="/api/assets/${path}" alt="${filename}" style="max-width: 100%; max-height: 400px;">
          </div>
        `;
      }
    }
  }

  createNewAsset() {
    if (!this.selectedAssetScript) {
      this.showStatus("Please select a script first", "error");
      return;
    }

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
    // @ts-ignore - monaco is loaded via AMD
    // @ts-ignore - monaco is loaded via AMD
    monaco.editor.setModelLanguage(this.monacoAssetEditor.getModel(), language);

    this.setText("current-asset-name", path + " (new)");
    this.setDisplay("monaco-asset-editor", "block");
    this.setDisplay("binary-asset-info", "none");
    this.setDisplay("no-asset-selected", "none");
    this.setDisabled("save-asset-btn", false);
    this.setDisabled("delete-asset-btn", true);

    this.showStatus("Create your asset and click Save", "info");
  }

  async saveCurrentAsset() {
    if (!this.currentAsset) return;
    if (!this.selectedAssetScript) {
      this.showStatus("No script selected for asset", "error");
      return;
    }

    const content = this.monacoAssetEditor.getValue();

    try {
      // Convert content to base64 using UTF-8 safe encoding
      const base64 = this.textToBase64(content);

      // Determine MIME type from extension
      const ext = this.currentAsset
        .substring(this.currentAsset.lastIndexOf("."))
        .toLowerCase();
      /** @type {Record<string, string>} */
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

      // Build URL with script URI parameter
      const url = `/api/assets?uri=${encodeURIComponent(this.selectedAssetScript)}`;

      const response = await fetch(url, {
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
      this.setText("current-asset-name", this.currentAsset);
      this.setDisabled("delete-asset-btn", false);

      // Reload assets list
      this.loadAssets();
    } catch (error) {
      const err = /** @type {Error} */ (error);
      this.showStatus("Error saving asset: " + err.message, "error");
    }
  }

  async deleteCurrentAsset() {
    if (!this.currentAsset) return;
    if (!this.selectedAssetScript) {
      this.showStatus("No script selected for asset", "error");
      return;
    }

    if (!confirm(`Are you sure you want to delete ${this.currentAsset}?`))
      return;

    try {
      // Build URL with script URI parameter
      const url = `/api/assets/${this.currentAsset}?uri=${encodeURIComponent(this.selectedAssetScript)}`;

      const response = await fetch(url, {
        method: "DELETE",
      });

      if (!response.ok) {
        const errorText = await response.text();
        throw new Error(
          `Delete failed with status ${response.status}: ${errorText}`,
        );
      }

      this.showStatus("Asset deleted successfully", "success");

      // Clear editor using the helper method
      this.clearAssetEditor();

      // Reload assets list
      this.loadAssets();
    } catch (error) {
      const err = /** @type {Error} */ (error);
      this.showStatus("Error deleting asset: " + err.message, "error");
    }
  }

  updateAssetSaveButton() {
    const saveBtn = /** @type {HTMLButtonElement | null} */ (
      document.getElementById("save-asset-btn")
    );
    if (saveBtn && this.currentAsset && this.isTextAsset(this.currentAsset)) {
      saveBtn.disabled = false;
      saveBtn.textContent = "Save *";
    }
  }

  triggerAssetUpload() {
    const uploadInput = /** @type {HTMLInputElement | null} */ (
      document.getElementById("asset-upload")
    );
    if (uploadInput) {
      uploadInput.click();
    }
  }

  /**
   * @param {FileList} files
   */
  async uploadAssets(files) {
    if (!this.selectedAssetScript) {
      this.showStatus("Please select a script first", "error");
      return;
    }

    for (const file of files) {
      try {
        const base64 = await this.fileToBase64(file);
        const publicPath = `/${file.name}`;

        // Build URL with script URI parameter
        const url = `/api/assets?uri=${encodeURIComponent(this.selectedAssetScript)}`;

        await fetch(url, {
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
        const err = /** @type {Error} */ (error);
        this.showStatus(
          `Error uploading ${file.name}: ${err.message}`,
          "error",
        );
      }
    }

    this.loadAssets();
  }

  /**
   * @param {string} path
   */
  downloadAsset(path) {
    const filename = path.split("/").pop();
    if (!filename) return;

    const isIco = filename.toLowerCase().endsWith(".ico");

    console.log(`Downloading asset: ${path} (isIco: ${isIco})`);

    if (isIco) {
      // For ICO files, use fetch + blob to ensure proper binary handling
      fetch(`/api/assets/${path}`)
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
          const err = /** @type {Error} */ (error);
          console.error("ICO download failed:", error);
          this.showStatus(`Download failed: ${err.message}`, "error");
          // Fallback to window.open
          window.open(`/api/assets/${path}`, "_blank");
        });
    } else {
      // For other files, use the simple window.open approach
      window.open(`/api/assets/${path}`, "_blank");
    }
  }

  // Logs Management
  async loadLogs() {
    try {
      const response = await fetch("/api/logs");
      const logs = await response.json();

      const logsContent = document.getElementById("logs-content");
      if (!logsContent) return;

      // Remember if user was at the bottom before refresh
      const wasAtBottom = this.isScrolledToBottom(logsContent);

      logsContent.innerHTML = "";

      // Reverse logs so newest appear at bottom
      logs.reverse().forEach(
        /** @param {any} log */
        (log) => {
          const logElement = document.createElement("div");
          logElement.innerHTML = this.templates["log-entry"]({
            time: new Date(log.timestamp).toLocaleTimeString(),
            level: log.level || "info",
            message: this.escapeHtml(log.message),
          });
          const firstChild = logElement.firstElementChild;
          if (firstChild) {
            logsContent.appendChild(firstChild);
          }
        },
      );

      // Only auto-scroll if user was already at the bottom
      if (wasAtBottom) {
        logsContent.scrollTop = logsContent.scrollHeight;
      }
    } catch (error) {
      const err = /** @type {Error} */ (error);
      this.showStatus("Error loading logs: " + err.message, "error");
    }
  }

  // Jump to the latest log entry without reloading
  jumpToLatestLogs() {
    const logsContent = document.getElementById("logs-content");
    if (!logsContent) return;

    // Scroll to the bottom of logs
    logsContent.scrollTop = logsContent.scrollHeight;

    // Briefly flash the logs area to indicate action (CSS handles animation)
    logsContent.classList.add("logs-flash");
    setTimeout(() => logsContent.classList.remove("logs-flash"), 500);
  }

  // Helper method to check if element is scrolled to bottom
  /**
   * @param {HTMLElement} element
   * @returns {boolean}
   */
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
    try {
      this.showStatus("Clearing logs...", "info");
      const response = await fetch("/api/logs", { method: "DELETE" });
      if (response.ok) {
        this.showStatus("Logs pruned successfully", "success");
        // Refresh logs after successful prune
        await this.loadLogs();
      } else {
        /** @type {any} */
        let body = {};
        try {
          body = await response.json();
        } catch (e) {
          /* ignore */
        }
        this.showStatus(
          "Failed to prune logs: " + (body.error || response.statusText),
          "error",
        );
      }
    } catch (error) {
      const err = /** @type {Error} */ (error);
      this.showStatus("Error pruning logs: " + err.message, "error");
    }
  }

  // Routes Management
  async loadRoutes() {
    try {
      const response = await fetch("/api/routes");
      const routes = await response.json();

      const routesList = document.getElementById("routes-list");
      if (!routesList) return;

      routesList.innerHTML = "";

      if (routes.length === 0) {
        routesList.innerHTML =
          '<div class="no-routes">No routes registered yet</div>';
        return;
      }

      routes.forEach(
        /** @param {any} route */
        (route) => {
          const routeElement = document.createElement("div");
          routeElement.innerHTML = this.templates["route-item"]({
            method: route.method,
            path: route.path,
            handler: route.handler,
            script_uri: route.script_uri,
          });

          // Add event listener for test button
          const testBtn = routeElement.querySelector(".test-btn");
          if (testBtn) {
            testBtn.addEventListener("click", () => {
              this.testRoute(route.path, route.method);
            });
          }

          const firstChild = routeElement.firstElementChild;
          if (firstChild) {
            routesList.appendChild(firstChild);
          }
        },
      );
    } catch (error) {
      const err = /** @type {Error} */ (error);
      this.showStatus("Error loading routes: " + err.message, "error");
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
        const err = /** @type {Error} */ (error);
        alert(`Error: ${err.message}`);
      });
  }

  /**
   * @param {string} path
   * @param {string} method
   */
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
        const err = /** @type {Error} */ (error);
        alert(`Error testing ${method} ${path}: ${err.message}`);
      });
  }

  loadInitialData() {
    this.loadScripts();
    this.populateAssetsScriptSelector();
  }

  async populateAssetsScriptSelector() {
    try {
      const response = await fetch("/api/scripts");
      const data = await response.json();

      const selector = this.getSelect("assets-script-select");
      if (!selector) return;

      // Clear existing options except the first placeholder
      selector.innerHTML = '<option value="">Select a script...</option>';

      // Check if user has admin privileges (can see all scripts)
      const isAdmin = data.permissions && data.permissions.canTogglePrivileged;

      // Filter scripts based on permissions
      let filteredScripts = data.scripts;
      if (!isAdmin) {
        // Non-admin users only see scripts they own
        filteredScripts = data.scripts.filter(
          /** @param {any} script */ (script) => script.isOwner,
        );
      }

      // Add scripts to selector
      filteredScripts.forEach(
        /** @param {any} script */ (script) => {
          const option = document.createElement("option");
          option.value = script.uri;
          const badge = script.privileged ? " [P]" : " [R]";
          option.textContent = script.displayName + badge;
          selector.appendChild(option);
        },
      );

      // If current script is set and available, select it
      if (
        this.currentScript &&
        filteredScripts.some(
          /** @param {any} s */ (s) => s.uri === this.currentScript,
        )
      ) {
        selector.value = this.currentScript;
        this.selectedAssetScript = this.currentScript;
        this.loadAssets();
      }
    } catch (error) {
      const err = /** @type {Error} */ (error);
      console.error("Error populating assets script selector:", err.message);
    }
  }

  /**
   * @param {string} message
   * @param {string} [type]
   */
  showStatus(message, type = "info") {
    const statusElement = document.getElementById("status-message");
    if (!statusElement) return;

    statusElement.textContent = message;
    statusElement.className = `status-${type}`;

    // Clear status after 5 seconds
    setTimeout(() => {
      statusElement.textContent = "Ready";
      statusElement.className = "";
    }, 5000);
  }

  /**
   * @param {File} file
   * @returns {Promise<string>}
   */
  fileToBase64(file) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.readAsDataURL(file);
      reader.onload = () => {
        const result = reader.result;
        if (typeof result === "string") {
          resolve(result.split(",")[1]);
        } else {
          reject(new Error("FileReader result is not a string"));
        }
      };
      reader.onerror = (error) => reject(error);
    });
  }

  // UTF-8 safe base64 encoding for text content
  /**
   * @param {string} text
   * @returns {string}
   */
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

  /**
   * @param {number} bytes
   * @returns {string}
   */
  formatBytes(bytes) {
    if (bytes === 0) return "0 Bytes";
    const k = 1024;
    const sizes = ["Bytes", "KB", "MB", "GB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + " " + sizes[i];
  }

  /**
   * @param {string} type
   * @param {boolean} isText
   * @returns {string}
   */
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

    if (!aiAssistant || !toggleBtn) return;

    aiAssistant.classList.toggle("collapsed");

    if (aiAssistant.classList.contains("collapsed")) {
      toggleBtn.textContent = "▲";
    } else {
      toggleBtn.textContent = "▼";
    }
  }

  async submitAIPrompt() {
    const promptInput = /** @type {HTMLInputElement | null} */ (
      document.getElementById("ai-prompt")
    );
    const responseDiv = document.getElementById("ai-response");
    const submitBtn = /** @type {HTMLButtonElement | null} */ (
      document.getElementById("submit-prompt-btn")
    );

    if (!promptInput || !responseDiv || !submitBtn) return;

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

      // Check if response was truncated
      if (data.truncated) {
        console.warn("AI response was truncated due to length");

        // If we have a parse error, show a helpful message
        if (data.parse_error && !data.parsed) {
          responseDiv.classList.remove("loading");
          responseDiv.innerHTML = `
            <div class="ai-response-content">
              <p><strong>⚠️ Response Incomplete</strong></p>
              <p style="color: var(--warning-color);">
                The AI's response was too long and got cut off. The generated code may be incomplete.
              </p>
              <p style="font-size: 12px; color: var(--text-muted);">
                Try asking for a simpler version or breaking your request into smaller parts.
              </p>
              <hr style="border-color: var(--border-color); margin: 10px 0;">
              <div class="ai-response-text">${this.escapeHtml(data.response)}</div>
            </div>
          `;
          this.showStatus("AI response was truncated", "warning");
          return;
        }
      }

      // Check if we got a structured response
      if (data.parsed && data.parsed.type) {
        this.handleStructuredAIResponse(data.parsed, prompt, data.truncated);
      } else {
        // Display plain text response
        this.displayPlainAIResponse(data.response, prompt, data.truncated);
      }

      this.showStatus("AI response received", "success");
    } catch (error) {
      const err = /** @type {Error} */ (error);
      responseDiv.classList.remove("loading");
      responseDiv.innerHTML = `
        <p style="color: var(--danger-color);">
          <strong>Error:</strong> ${this.escapeHtml(err.message)}
        </p>
      `;
      this.showStatus("Failed to get AI response", "error");
    } finally {
      // Re-enable submit button
      submitBtn.disabled = false;
      submitBtn.textContent = "Submit";
    }
  }

  /**
   * @param {string} responseText
   * @param {string} prompt
   * @param {boolean} [truncated]
   */
  displayPlainAIResponse(responseText, prompt, truncated = false) {
    const responseDiv = document.getElementById("ai-response");
    if (!responseDiv) return;

    responseDiv.classList.remove("loading");

    const truncationWarning = truncated
      ? `
      <div style="background: var(--warning-bg, #fff3cd); color: var(--warning-color, #856404); padding: 10px; border-radius: 4px; margin-bottom: 10px;">
        ⚠️ <strong>Note:</strong> The response may have been truncated due to length. Consider asking for a shorter version.
      </div>
    `
      : "";

    responseDiv.innerHTML = `
      <div class="ai-response-content">
        <p><strong>You asked:</strong> ${this.escapeHtml(prompt)}</p>
        <hr style="border-color: var(--border-color); margin: 10px 0;">
        ${truncationWarning}
        <div class="ai-response-text">${this.escapeHtml(responseText)}</div>
        <p style="color: var(--text-muted); font-size: 11px; margin-top: 10px;">
          ${new Date().toLocaleString()}
        </p>
      </div>
    `;
  }

  /**
   * @param {any} parsed
   * @param {string} prompt
   * @param {boolean} [truncated]
   */
  handleStructuredAIResponse(parsed, prompt, truncated = false) {
    const responseDiv = document.getElementById("ai-response");
    if (!responseDiv) return;

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

    const truncationWarning = truncated
      ? `
      <div style="background: var(--warning-bg, #fff3cd); color: var(--warning-color, #856404); padding: 10px; border-radius: 4px; margin-bottom: 10px;">
        ⚠️ <strong>Warning:</strong> The AI response was truncated. The generated code may be incomplete. Please review carefully before applying.
      </div>
    `
      : "";

    let html = `
      <div class="ai-response-content">
        <p><strong>You asked:</strong> ${this.escapeHtml(prompt)}</p>
        <hr style="border-color: var(--border-color); margin: 10px 0;">
        ${truncationWarning}
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

  /**
   * @param {string} name
   * @param {string} originalCode
   * @param {string} newCode
   * @param {string} explanation
   * @param {string} action
   * @param {string} contentType
   */
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

    if (!modal || !title || !explanationDiv) return;

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

  /**
   * @param {string} originalCode
   * @param {string} newCode
   * @param {string} [language]
   * @returns {Promise<void>}
   */
  async createDiffEditor(originalCode, newCode, language = "javascript") {
    const container = document.getElementById("monaco-diff-editor");
    if (!container) return Promise.resolve();

    // Clear any existing content
    container.innerHTML = "";

    return new Promise((resolve) => {
      if (this.monacoDiffEditor) {
        this.monacoDiffEditor.dispose();
      }

      // @ts-ignore - monaco is loaded via AMD
      this.monacoDiffEditor = monaco.editor.createDiffEditor(container, {
        theme: "vs-dark",
        readOnly: true,
        automaticLayout: true,
        renderSideBySide: true,
        fontSize: 13,
      });

      // @ts-ignore - monaco is loaded via AMD
      const original = monaco.editor.createModel(
        originalCode || "// New file",
        language,
      );
      // @ts-ignore - monaco is loaded via AMD
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
    if (!modal) return;

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
          /** @type {Record<string, string>} */
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
          this.setText("current-asset-name", name);
          this.setDisabled("save-asset-btn", false);
          this.setDisabled("delete-asset-btn", false);

          // Set content directly from newCode
          this.monacoAssetEditor.setValue(newCode);
          const language = this.getLanguageMode(name);
          // @ts-ignore - monaco is loaded via AMD
          monaco.editor.setModelLanguage(
            this.monacoAssetEditor.getModel(),
            language,
          );

          // Show editor
          this.setDisplay("monaco-asset-editor", "block");
          this.setDisplay("binary-asset-info", "none");
          this.setDisplay("no-asset-selected", "none");

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
      const err = /** @type {Error} */ (error);
      this.showStatus(`Error applying changes: ${err.message}`, "error");
    }
  }

  /**
   * @param {string} assetPath
   * @param {string} explanation
   */
  confirmDeleteAsset(assetPath, explanation) {
    if (
      confirm(`${explanation}\n\nAre you sure you want to delete ${assetPath}?`)
    ) {
      fetch(`/api/assets/${assetPath}`, {
        method: "DELETE",
      })
        .then(() => {
          this.showStatus("Asset deleted successfully", "success");
          this.loadAssets();

          if (this.currentAsset === assetPath) {
            this.currentAsset = null;
            this.setText("current-asset-name", "No asset selected");
            this.setDisplay("monaco-asset-editor", "none");
            this.setDisplay("binary-asset-info", "none");
            this.setDisplay("no-asset-selected", "block");
          }
        })
        .catch((error) => {
          const err = /** @type {Error} */ (error);
          this.showStatus("Error deleting asset: " + err.message, "error");
        });
    }
  }

  /**
   * @param {string} scriptName
   * @param {string} explanation
   */
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
            this.setText("current-script-name", "No script selected");
            if (this.monacoEditor) {
              this.monacoEditor.setValue("// Select a script to edit");
            }
          }
        })
        .catch((error) => {
          const err = /** @type {Error} */ (error);
          this.showStatus("Error deleting script: " + err.message, "error");
        });
    }
  }

  clearAIPrompt() {
    const promptInput = /** @type {HTMLInputElement | null} */ (
      document.getElementById("ai-prompt")
    );
    if (promptInput) {
      promptInput.value = "";
      promptInput.focus();
    }
  }

  /**
   * @param {string} text
   * @returns {string}
   */
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
  // @ts-ignore - window.editor is expected by onclick handlers in HTML
  window.editor = new AIWebEngineEditor();
}

console.log("[Editor] Script loaded, waiting for DOMContentLoaded...");
document.addEventListener("DOMContentLoaded", initEditor);
