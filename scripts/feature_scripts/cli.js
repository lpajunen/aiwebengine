/// <reference path="../../assets/aiwebengine-priv.d.ts" />

function getArgs(context) {
  return (context && context.args) || {};
}

// asset management script: manages assets for other scripts
function asset_handler(context) {
  const req = context.request || {};
  try {
    const path = req.path;
    const method = req.method;
    const queryParams = req.query || {};

    // Validate required script parameter
    const scriptUri = queryParams.script;
    if (!scriptUri) {
      return ResponseBuilder.error(400, "Missing required parameter: script");
    }

    if (method === "GET") {
      if (path === "/assets") {
        const assetUri = queryParams.asset;

        if (assetUri) {
          // Fetch specific asset
          const assetContent = assetStorage.fetchAssetForUri(
            scriptUri,
            assetUri,
          );
          if (
            assetContent &&
            !assetContent.startsWith("Error:") &&
            assetContent !== "Asset '" + assetUri + "' not found"
          ) {
            return ResponseBuilder.json({
              script: scriptUri,
              asset: assetUri,
              content: assetContent,
            });
          } else {
            return ResponseBuilder.error(
              404,
              assetContent || "Asset not found",
            );
          }
        } else {
          // List all assets for the script
          const assetsJson = assetStorage.listAssetsForUri(scriptUri);
          const assetMetadata = JSON.parse(assetsJson);
          return ResponseBuilder.json({
            script: scriptUri,
            assets: assetMetadata,
          });
        }
      }
    } else if (method === "POST") {
      if (path === "/assets") {
        // Create/update asset
        const body = JSON.parse(req.body || "{}");
        const assetUri = body.asset || queryParams.asset;

        if (!assetUri) {
          return ResponseBuilder.error(
            400,
            "Missing required parameter: asset",
          );
        }

        if (body.mimetype && body.content) {
          const result = assetStorage.upsertAssetForUri(
            scriptUri,
            assetUri,
            body.mimetype,
            body.content,
          );

          if (
            (result && result.startsWith("Error")) ||
            result.startsWith("Access denied")
          ) {
            return ResponseBuilder.error(403, result);
          }

          return ResponseBuilder.json(
            {
              message: result || "Asset created/updated",
              script: scriptUri,
              asset: assetUri,
            },
            201,
          );
        } else {
          return ResponseBuilder.error(
            400,
            "Missing required fields: mimetype, content",
          );
        }
      }
    } else if (method === "DELETE") {
      if (path === "/assets") {
        const assetUri = queryParams.asset;

        if (!assetUri) {
          return ResponseBuilder.error(
            400,
            "Missing required parameter: asset",
          );
        }

        const result = assetStorage.deleteAssetForUri(scriptUri, assetUri);

        if (result && result.startsWith("Error:")) {
          return ResponseBuilder.error(403, result);
        } else if (result && result.includes("not found")) {
          return ResponseBuilder.error(404, result);
        }

        return ResponseBuilder.json({
          message: result || "Asset deleted",
          script: scriptUri,
          asset: assetUri,
        });
      }
    }

    return ResponseBuilder.error(400, "Invalid request");
  } catch (e) {
    console.log("Asset handler error: " + String(e));
    return ResponseBuilder.error(500, String(e));
  }
}

// ============================================================================
// MCP Tool Handlers for Asset Operations
// ============================================================================

// List assets handler - lists all assets owned by a script
function listAssetsHandler(context) {
  const args = getArgs(context);
  const scriptUri = args.script;

  if (!scriptUri) {
    return JSON.stringify({
      error: "Missing required parameter: script",
    });
  }

  try {
    const assetsJson = assetStorage.listAssetsForUri(scriptUri);
    const assets = JSON.parse(assetsJson);

    console.log(
      `MCP list_assets found ${assets.length} assets for: ${scriptUri}`,
    );

    return JSON.stringify({
      script: scriptUri,
      assets: assets,
      count: assets.length,
      timestamp: new Date().toISOString(),
    });
  } catch (error) {
    console.error(`MCP list_assets error: ${error.message}`);
    return JSON.stringify({
      error: `Failed to list assets: ${error.message}`,
    });
  }
}

// Read asset handler - fetches the content of a specific asset
function readAssetHandler(context) {
  const args = getArgs(context);
  const scriptUri = args.script;
  const assetUri = args.asset;

  if (!scriptUri) {
    return JSON.stringify({
      error: "Missing required parameter: script",
    });
  }
  if (!assetUri) {
    return JSON.stringify({
      error: "Missing required parameter: asset",
    });
  }

  try {
    const content = assetStorage.fetchAssetForUri(scriptUri, assetUri);

    if (
      content &&
      !content.startsWith("Error:") &&
      content !== "Asset '" + assetUri + "' not found"
    ) {
      return JSON.stringify({
        script: scriptUri,
        asset: assetUri,
        content: content,
        timestamp: new Date().toISOString(),
      });
    }

    return JSON.stringify({
      error: content || `Asset not found: ${assetUri}`,
    });
  } catch (error) {
    console.error(`MCP read_asset error: ${error.message}`);
    return JSON.stringify({
      error: `Failed to read asset: ${error.message}`,
    });
  }
}

// Write asset handler - creates or updates an asset for a script
function writeAssetHandler(context) {
  const args = getArgs(context);
  const scriptUri = args.script;
  const assetUri = args.asset;
  const mimetype = args.mimetype;
  const content = args.content;

  if (!scriptUri) {
    return JSON.stringify({
      error: "Missing required parameter: script",
    });
  }
  if (!assetUri) {
    return JSON.stringify({
      error: "Missing required parameter: asset",
    });
  }
  if (!mimetype) {
    return JSON.stringify({
      error: "Missing required parameter: mimetype",
    });
  }
  if (content === undefined || content === null) {
    return JSON.stringify({
      error: "Missing required parameter: content",
    });
  }

  try {
    const result = assetStorage.upsertAssetForUri(
      scriptUri,
      assetUri,
      mimetype,
      content,
    );

    if (
      !result ||
      result.startsWith("Error") ||
      result.startsWith("Access denied")
    ) {
      console.error(`MCP write_asset failed: ${result}`);
      return JSON.stringify({
        error: `Failed to write asset: ${result || "Unknown error"}`,
      });
    }

    console.log(`MCP write_asset: ${assetUri} for ${scriptUri}`);

    return JSON.stringify({
      success: true,
      message: result,
      script: scriptUri,
      asset: assetUri,
      timestamp: new Date().toISOString(),
    });
  } catch (error) {
    console.error(`MCP write_asset error: ${error.message}`);
    return JSON.stringify({
      error: `Failed to write asset: ${error.message}`,
    });
  }
}

// Delete asset handler - removes an asset from a script
function deleteAssetHandler(context) {
  const args = getArgs(context);
  const scriptUri = args.script;
  const assetUri = args.asset;

  if (!scriptUri) {
    return JSON.stringify({
      error: "Missing required parameter: script",
    });
  }
  if (!assetUri) {
    return JSON.stringify({
      error: "Missing required parameter: asset",
    });
  }

  try {
    const result = assetStorage.deleteAssetForUri(scriptUri, assetUri);

    if (result && result.startsWith("Error:")) {
      console.error(`MCP delete_asset failed: ${result}`);
      return JSON.stringify({
        error: `Failed to delete asset: ${result}`,
      });
    }
    if (result && result.includes("not found")) {
      return JSON.stringify({
        error: result,
      });
    }

    console.log(`MCP delete_asset: ${assetUri} for ${scriptUri}`);

    return JSON.stringify({
      success: true,
      message: result || "Asset deleted",
      script: scriptUri,
      asset: assetUri,
      timestamp: new Date().toISOString(),
    });
  } catch (error) {
    console.error(`MCP delete_asset error: ${error.message}`);
    return JSON.stringify({
      error: `Failed to delete asset: ${error.message}`,
    });
  }
}

// Initialization function - called when script is loaded or updated
function init(context) {
  try {
    console.log(`Initializing cli.js script at ${new Date().toISOString()}`);
    console.log(`Init context: ${JSON.stringify(context)}`);

    // Register the routes - now using query parameters for script and asset
    routeRegistry.registerRoute("/assets", "asset_handler", "GET", {
      summary: "List or fetch assets for a script",
      description:
        "Lists all assets for a script or fetches a specific asset. " +
        "Requires user to own the script, have ReadAssets capability, or be an administrator.",
      tags: ["Assets"],
      parameters: JSON.stringify([
        {
          name: "script",
          in: "query",
          required: true,
          schema: { type: "string" },
          description:
            "URI of the script whose assets to manage (e.g., 'https://example.com/myscript')",
        },
        {
          name: "asset",
          in: "query",
          required: false,
          schema: { type: "string" },
          description:
            "URI/path of the specific asset to fetch (e.g., '/images/logo.png'). If omitted, returns list of all assets.",
        },
      ]),
    });

    routeRegistry.registerRoute("/assets", "asset_handler", "POST", {
      summary: "Create or update an asset",
      description:
        "Creates or updates an asset for a script. " +
        "Requires user to own the script, have WriteAssets capability, or be an administrator.",
      tags: ["Assets"],
      parameters: JSON.stringify([
        {
          name: "script",
          in: "query",
          required: true,
          schema: { type: "string" },
          description: "URI of the script that will own this asset",
        },
      ]),
      requestBody: JSON.stringify({
        required: true,
        content: {
          "application/json": {
            schema: {
              type: "object",
              required: ["asset", "mimetype", "content"],
              properties: {
                asset: {
                  type: "string",
                  description:
                    "URI/path of the asset (e.g., '/images/logo.png')",
                },
                mimetype: {
                  type: "string",
                  description:
                    "MIME type of the asset (e.g., 'image/png', 'text/css')",
                },
                content: {
                  type: "string",
                  format: "byte",
                  description: "Base64-encoded content of the asset (max 10MB)",
                },
              },
            },
          },
        },
      }),
    });

    routeRegistry.registerRoute("/assets", "asset_handler", "DELETE", {
      summary: "Delete an asset",
      description:
        "Deletes an asset from a script. " +
        "Requires user to own the script, have DeleteAssets capability, or be an administrator.",
      tags: ["Assets"],
      parameters: JSON.stringify([
        {
          name: "script",
          in: "query",
          required: true,
          schema: { type: "string" },
          description: "URI of the script that owns the asset",
        },
        {
          name: "asset",
          in: "query",
          required: true,
          schema: { type: "string" },
          description:
            "URI/path of the asset to delete (e.g., '/images/logo.png')",
        },
      ]),
    });

    // Register MCP tools for asset operations
    let registeredMcpTools = 0;
    if (typeof mcpRegistry !== "undefined") {
      console.log("Registering MCP asset operation tools...");

      mcpRegistry.registerTool(
        "list_assets",
        "List all assets owned by a script. Requires the user to own the script, have ReadAssets capability, or be an administrator.",
        JSON.stringify({
          type: "object",
          properties: {
            script: {
              type: "string",
              description:
                "URI of the script whose assets to list (e.g., 'https://example.com/myscript')",
            },
          },
          required: ["script"],
        }),
        "listAssetsHandler",
      );

      mcpRegistry.registerTool(
        "read_asset",
        "Fetch the base64-encoded content of a specific asset owned by a script. Requires the user to own the script, have ReadAssets capability, or be an administrator.",
        JSON.stringify({
          type: "object",
          properties: {
            script: {
              type: "string",
              description: "URI of the script that owns the asset",
            },
            asset: {
              type: "string",
              description:
                "URI/path of the asset to fetch (e.g., '/images/logo.png')",
            },
          },
          required: ["script", "asset"],
        }),
        "readAssetHandler",
      );

      mcpRegistry.registerTool(
        "write_asset",
        "Create or update an asset for a script. Requires the user to own the script, have WriteAssets capability, or be an administrator.",
        JSON.stringify({
          type: "object",
          properties: {
            script: {
              type: "string",
              description: "URI of the script that will own the asset",
            },
            asset: {
              type: "string",
              description: "URI/path of the asset (e.g., '/images/logo.png')",
            },
            mimetype: {
              type: "string",
              description:
                "MIME type of the asset (e.g., 'image/png', 'text/css')",
            },
            content: {
              type: "string",
              description: "Base64-encoded content of the asset (max 10MB)",
            },
          },
          required: ["script", "asset", "mimetype", "content"],
        }),
        "writeAssetHandler",
      );

      mcpRegistry.registerTool(
        "delete_asset",
        "Delete an asset from a script. Requires the user to own the script, have DeleteAssets capability, or be an administrator.",
        JSON.stringify({
          type: "object",
          properties: {
            script: {
              type: "string",
              description: "URI of the script that owns the asset",
            },
            asset: {
              type: "string",
              description:
                "URI/path of the asset to delete (e.g., '/images/logo.png')",
            },
          },
          required: ["script", "asset"],
        }),
        "deleteAssetHandler",
      );

      registeredMcpTools = 4;
      console.log("MCP asset operation tools registered successfully");
    } else {
      console.warn("mcpRegistry unavailable; skipping MCP tool registration");
    }

    console.log("Asset management script initialized successfully");

    return {
      success: true,
      message: "Asset management script initialized successfully",
      registeredEndpoints: 3,
      registeredMcpTools: registeredMcpTools,
    };
  } catch (error) {
    console.log(
      `Asset management script initialization failed: ${error.message}`,
    );
    throw error;
  }
}
