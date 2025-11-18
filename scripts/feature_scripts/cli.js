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
        return {
          status: 200,
          body: JSON.stringify({ assets: assetNames, metadata: assetMetadata }),
          contentType: "application/json",
        };
      } else if (path.startsWith("/assets/")) {
        // Fetch specific asset
        const publicPath = path.substring("/assets/".length);
        const assetJson = assetStorage.fetchAsset("/" + publicPath);
        if (assetJson !== "null") {
          return {
            status: 200,
            body: assetJson,
            contentType: "application/json",
          };
        } else {
          return {
            status: 404,
            body: JSON.stringify({ error: "Asset not found" }),
            contentType: "application/json",
          };
        }
      }
    } else if (method === "POST") {
      if (path === "/assets") {
        // Create/update asset
        const body = JSON.parse(req.body || "{}");
        if (body.publicPath && body.mimetype && body.content) {
          assetStorage.upsertAsset(
            body.publicPath,
            body.mimetype,
            body.content,
          );
          return {
            status: 201,
            body: JSON.stringify({ message: "Asset created/updated" }),
            contentType: "application/json",
          };
        } else {
          return {
            status: 400,
            body: JSON.stringify({
              error: "Missing required fields: publicPath, mimetype, content",
            }),
            contentType: "application/json",
          };
        }
      }
    } else if (method === "DELETE") {
      if (path.startsWith("/assets/")) {
        // Delete asset
        const publicPath = path.substring("/assets/".length);
        const deleted = assetStorage.deleteAsset("/" + publicPath);
        if (deleted) {
          return {
            status: 200,
            body: JSON.stringify({ message: "Asset deleted" }),
            contentType: "application/json",
          };
        } else {
          return {
            status: 404,
            body: JSON.stringify({ error: "Asset not found" }),
            contentType: "application/json",
          };
        }
      }
    }

    return {
      status: 400,
      body: JSON.stringify({ error: "Invalid request" }),
      contentType: "application/json",
    };
  } catch (e) {
    console.log("Asset handler error: " + String(e));
    return {
      status: 500,
      body: JSON.stringify({ error: String(e) }),
      contentType: "application/json",
    };
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
