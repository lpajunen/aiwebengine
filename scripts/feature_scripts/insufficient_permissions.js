// Insufficient Permissions Page
// This page is displayed when a user is authenticated but doesn't have the required role
// to access a particular resource (e.g., Editor or Administrator privileges required).

function serveInsufficientPermissions(req) {
  // Get the attempted path from query parameters if available
  const attemptedPath = req.query?.attempted || "the requested page";

  // Check if user is authenticated to show personalized message
  let userName = "User";
  let userEmail = "";
  let isAuthenticated = false;

  try {
    if (typeof auth !== "undefined") {
      const user = auth.getUser();
      if (user) {
        isAuthenticated = true;
        userName = user.name || user.email || "User";
        userEmail = user.email || "";
      }
    }
  } catch (error) {
    writeLog("Could not get user info: " + error.message);
  }

  const html = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Insufficient Permissions</title>
    <link rel="icon" type="image/x-icon" href="/favicon.ico">
    <style>
        * {
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }

        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
            padding: 20px;
        }

        .container {
            background: white;
            border-radius: 16px;
            box-shadow: 0 20px 60px rgba(0, 0, 0, 0.3);
            max-width: 600px;
            width: 100%;
            padding: 48px;
            text-align: center;
        }

        .icon {
            width: 80px;
            height: 80px;
            margin: 0 auto 24px;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            border-radius: 50%;
            display: flex;
            align-items: center;
            justify-content: center;
            font-size: 40px;
            color: white;
        }

        h1 {
            font-size: 28px;
            color: #2d3748;
            margin-bottom: 16px;
            font-weight: 700;
        }

        .subtitle {
            font-size: 18px;
            color: #718096;
            margin-bottom: 32px;
            line-height: 1.6;
        }

        .info-box {
            background: #f7fafc;
            border-left: 4px solid #667eea;
            border-radius: 8px;
            padding: 20px;
            margin-bottom: 32px;
            text-align: left;
        }

        .info-box p {
            color: #4a5568;
            line-height: 1.6;
            margin-bottom: 12px;
        }

        .info-box p:last-child {
            margin-bottom: 0;
        }

        .info-box strong {
            color: #2d3748;
        }

        .user-info {
            background: #edf2f7;
            border-radius: 8px;
            padding: 16px;
            margin-bottom: 24px;
            font-size: 14px;
            color: #4a5568;
        }

        .user-info strong {
            color: #2d3748;
        }

        .attempted-path {
            background: #fff5f5;
            border-left: 4px solid #f56565;
            border-radius: 8px;
            padding: 16px;
            margin-bottom: 24px;
            text-align: left;
            font-size: 14px;
            color: #742a2a;
            word-break: break-all;
        }

        .buttons {
            display: flex;
            gap: 12px;
            justify-content: center;
            flex-wrap: wrap;
        }

        .button {
            display: inline-block;
            padding: 12px 24px;
            border-radius: 8px;
            text-decoration: none;
            font-weight: 600;
            font-size: 16px;
            transition: all 0.2s;
            border: none;
            cursor: pointer;
        }

        .button-primary {
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: white;
        }

        .button-primary:hover {
            transform: translateY(-2px);
            box-shadow: 0 10px 20px rgba(102, 126, 234, 0.3);
        }

        .button-secondary {
            background: #edf2f7;
            color: #4a5568;
        }

        .button-secondary:hover {
            background: #e2e8f0;
        }

        .contact-info {
            margin-top: 32px;
            padding-top: 24px;
            border-top: 1px solid #e2e8f0;
            font-size: 14px;
            color: #718096;
        }

        @media (max-width: 600px) {
            .container {
                padding: 32px 24px;
            }

            h1 {
                font-size: 24px;
            }

            .subtitle {
                font-size: 16px;
            }

            .buttons {
                flex-direction: column;
            }

            .button {
                width: 100%;
            }
        }
    </style>
</head>
<body>
    <div class="container">
        <div class="icon">
            🔒
        </div>
        
        <h1>Insufficient Permissions</h1>
        
        <p class="subtitle">
            You don't have the required permissions to access this resource.
        </p>

        ${
          isAuthenticated
            ? `
        <div class="user-info">
            <strong>Signed in as:</strong> ${userName}${userEmail ? ` (${userEmail})` : ""}
        </div>
        `
            : ""
        }

        ${
          attemptedPath !== "the requested page"
            ? `
        <div class="attempted-path">
            <strong>Attempted to access:</strong> ${attemptedPath}
        </div>
        `
            : ""
        }

        <div class="info-box">
            <p><strong>Why am I seeing this?</strong></p>
            <p>This page or feature requires <strong>Editor</strong> or <strong>Administrator</strong> privileges. Your current account does not have these permissions.</p>
            <p><strong>What can I do?</strong></p>
            <p>• Contact your system administrator to request the appropriate role</p>
            <p>• Verify you're signed in with the correct account</p>
            <p>• Return to the home page to access features available to you</p>
        </div>

        <div class="buttons">
            <a href="/" class="button button-primary">Go to Home</a>
            ${
              isAuthenticated
                ? `
            <a href="/auth/logout" class="button button-secondary">Sign Out</a>
            `
                : `
            <a href="/auth/login" class="button button-secondary">Sign In</a>
            `
            }
        </div>

        <div class="contact-info">
            If you believe this is an error, please contact your system administrator.
        </div>
    </div>
</body>
</html>`;

  return {
    status: 403,
    headers: {
      "Content-Type": "text/html; charset=utf-8",
    },
    body: html,
  };
}

// Initialization function - called when script is loaded or updated
function init(context) {
  try {
    writeLog(
      `Initializing insufficient_permissions.js script at ${new Date().toISOString()}`,
    );
    writeLog(`Init context: ${JSON.stringify(context)}`);

    // Register the route
    register(
      "/insufficient-permissions",
      "serveInsufficientPermissions",
      "GET",
    );

    writeLog("Insufficient permissions script initialized successfully");

    return {
      success: true,
      message: "Insufficient permissions script initialized successfully",
      registeredEndpoints: 1,
    };
  } catch (error) {
    writeLog(
      `Insufficient permissions script initialization failed: ${error.message}`,
    );
    throw error;
  }
}
