/// <reference path="../../assets/aiwebengine.d.ts" />

// asset management script: demonstrates asset CRUD operations
function asset_handler(context) {
  const req = context.request || {};
  try {
    const path = req.path;
    const method = req.method;

    if (method === "GET") {
      if (path === "/assets") {
        // List all assets
        const assetsJson = assetStorage.listAssets();
        const assetMetadata = JSON.parse(assetsJson);
        // Extract just the names for backwards compatibility
        const assetNames = assetMetadata.map((a) => a.name);
        return Response.json({ assets: assetNames, metadata: assetMetadata });
      } else if (path.startsWith("/assets/")) {
        // Fetch specific asset
        const publicPath = path.substring("/assets/".length);
        const assetJson = assetStorage.fetchAsset("/" + publicPath);
        if (assetJson !== "null") {
          return Response.json(JSON.parse(assetJson));
        } else {
          return Response.error(404, "Asset not found");
        }
      }
    } else if (method === "POST") {
      if (path === "/assets") {
        // Create/update asset
        const body = JSON.parse(req.body || "{}");
        if (body.publicPath && body.mimetype && body.content) {
          assetStorage.upsertAsset(
            body.publicPath,
            body.content,
            body.mimetype,
            null,
          );
          return Response.json({ message: "Asset created/updated" }, 201);
        } else {
          return Response.error(
            400,
            "Missing required fields: publicPath, mimetype, content",
          );
        }
      }
    } else if (method === "DELETE") {
      if (path.startsWith("/assets/")) {
        // Delete asset
        const publicPath = path.substring("/assets/".length);
        const deleted = assetStorage.deleteAsset("/" + publicPath);
        if (deleted) {
          return Response.json({ message: "Asset deleted" });
        } else {
          return Response.error(404, "Asset not found");
        }
      }
    }

    return Response.error(400, "Invalid request");
  } catch (e) {
    console.log("Asset handler error: " + String(e));
    return Response.error(500, String(e));
  }
}

// Initialization function - called when script is loaded or updated
function init(context) {
  try {
    console.log(`Initializing cli.js script at ${new Date().toISOString()}`);
    console.log(`Init context: ${JSON.stringify(context)}`);

    // Register the routes
    routeRegistry.registerRoute("/assets", "asset_handler", "GET");
    routeRegistry.registerRoute("/assets", "asset_handler", "POST");
    routeRegistry.registerRoute("/assets/*", "asset_handler", "GET");
    routeRegistry.registerRoute("/assets/*", "asset_handler", "DELETE");

    console.log("Asset management script initialized successfully");

    return {
      success: true,
      message: "Asset management script initialized successfully",
      registeredEndpoints: 4,
    };
  } catch (error) {
    console.log(
      `Asset management script initialization failed: ${error.message}`,
    );
    throw error;
  }
}
