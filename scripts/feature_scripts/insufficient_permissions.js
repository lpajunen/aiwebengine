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
    <title>Insufficient Permissions - aiwebengine</title>
    <link rel="stylesheet" href="/engine.css">
    <link rel="icon" type="image/x-icon" href="/favicon.ico">
    <style>
        /* Insufficient permissions page specific overrides */
        body {
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
            padding: 2rem 0;
        }

        .permissions-container {
            max-width: 600px;
            margin: 0 auto;
            background: rgba(255, 255, 255, 0.95);
            backdrop-filter: blur(10px);
            border-radius: var(--border-radius-lg);
            box-shadow: var(--shadow-lg);
            overflow: hidden;
        }

        .permissions-content {
            padding: 3rem 2rem;
            text-align: center;
        }

        .permissions-icon {
            width: 80px;
            height: 80px;
            margin: 0 auto 1.5rem;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            border-radius: 50%;
            display: flex;
            align-items: center;
            justify-content: center;
            font-size: 40px;
            color: white;
        }

        .permissions-content h1 {
            color: var(--text-color);
            margin-bottom: 1rem;
            font-size: 2rem;
        }

        .permissions-subtitle {
            color: var(--text-muted);
            margin-bottom: 2rem;
            font-size: 1.1rem;
            line-height: 1.6;
        }

        .info-box {
            background: var(--bg-secondary);
            border-left: 4px solid var(--primary-color);
            border-radius: var(--border-radius);
            padding: 1.5rem;
            margin-bottom: 2rem;
            text-align: left;
        }

        .info-box p {
            color: var(--text-muted);
            line-height: 1.6;
            margin-bottom: 0.75rem;
        }

        .info-box p:last-child {
            margin-bottom: 0;
        }

        .info-box strong {
            color: var(--text-color);
        }

        .user-info {
            background: var(--info-bg);
            border: 1px solid var(--info-border);
            border-radius: var(--border-radius);
            padding: 1rem;
            margin-bottom: 1.5rem;
            font-size: 0.9rem;
            color: var(--info-color);
        }

        .user-info strong {
            color: var(--text-color);
        }

        .attempted-path {
            background: var(--error-bg);
            border-left: 4px solid var(--error-color);
            border-radius: var(--border-radius);
            padding: 1rem;
            margin-bottom: 1.5rem;
            text-align: left;
            font-size: 0.9rem;
            color: var(--error-color);
            word-break: break-all;
        }

        .permissions-actions {
            display: flex;
            gap: 1rem;
            justify-content: center;
            flex-wrap: wrap;
            margin-bottom: 2rem;
        }

        .permissions-actions a {
            padding: 0.75rem 1.5rem;
            border-radius: var(--border-radius);
            text-decoration: none;
            font-weight: 600;
            font-size: 1rem;
            transition: var(--transition);
            display: inline-block;
            text-align: center;
        }

        .permissions-actions a:first-child {
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: white;
        }

        .permissions-actions a:first-child:hover {
            transform: translateY(-2px);
            box-shadow: var(--shadow);
        }

        .permissions-actions a:last-child {
            background: var(--bg-secondary);
            color: var(--text-muted);
            border: 1px solid var(--border-color);
        }

        .permissions-actions a:last-child:hover {
            background: var(--bg-primary);
        }

        .contact-info {
            margin-top: 2rem;
            padding-top: 1.5rem;
            border-top: 1px solid var(--border-color);
            font-size: 0.9rem;
            color: var(--text-muted);
        }

        @media (max-width: 768px) {
            .permissions-content {
                padding: 2rem 1rem;
            }

            .permissions-content h1 {
                font-size: 1.75rem;
            }

            .permissions-actions {
                flex-direction: column;
            }

            .permissions-actions a {
                width: 100%;
            }
        }
    </style>
</head>
<body>
    <div class="permissions-container">
        <div class="permissions-content">
            <div class="permissions-icon">
                🔒
            </div>

            <h1>Insufficient Permissions</h1>

            <p class="permissions-subtitle">
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

            <div class="permissions-actions">
                <a href="/">Go to Home</a>
                ${
                  isAuthenticated
                    ? `
                <a href="/auth/logout">Sign Out</a>
                `
                    : `
                <a href="/auth/login">Sign In</a>
                `
                }
            </div>

            <div class="contact-info">
                If you believe this is an error, please contact your system administrator.
            </div>
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
