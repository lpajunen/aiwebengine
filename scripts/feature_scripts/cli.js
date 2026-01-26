/// <reference path="../../assets/aiwebengine-priv.d.ts" />

// asset management script: manages assets for other scripts
function asset_handler(context) {
  const req = context.request || {};
  try {
    const path = req.path;
    const method = req.method;
    const queryParams = req.queryParams || {};

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
        "Requires user to own the script, have ReadAssets capability, or be an administrator. " +
        "Query parameters: " +
        "- script (required): URI of the script whose assets to manage (e.g., 'https://example.com/myscript'). " +
        "- asset (optional): URI/path of the specific asset to fetch (e.g., '/images/logo.png'). If omitted, returns list of all assets.",
      tags: ["Assets"],
    });

    routeRegistry.registerRoute("/assets", "asset_handler", "POST", {
      summary: "Create or update an asset",
      description:
        "Creates or updates an asset for a script. " +
        "Requires user to own the script, have WriteAssets capability, or be an administrator. " +
        "Query parameters: " +
        "- script (required): URI of the script that will own this asset. " +
        "Request body (JSON): " +
        "- asset (required, string): URI/path of the asset (e.g., '/images/logo.png'). " +
        "- mimetype (required, string): MIME type of the asset (e.g., 'image/png', 'text/css'). " +
        "- content (required, string): Base64-encoded content of the asset (max 10MB).",
      tags: ["Assets"],
    });

    routeRegistry.registerRoute("/assets", "asset_handler", "DELETE", {
      summary: "Delete an asset",
      description:
        "Deletes an asset from a script. " +
        "Requires user to own the script, have DeleteAssets capability, or be an administrator. " +
        "Query parameters: " +
        "- script (required): URI of the script that owns the asset. " +
        "- asset (required): URI/path of the asset to delete (e.g., '/images/logo.png').",
      tags: ["Assets"],
    });

    console.log("Asset management script initialized successfully");

    return {
      success: true,
      message: "Asset management script initialized successfully",
      registeredEndpoints: 3,
    };
  } catch (error) {
    console.log(
      `Asset management script initialization failed: ${error.message}`,
    );
    throw error;
  }
}
