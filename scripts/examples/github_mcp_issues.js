/**
 * GitHub MCP Client Example
 *
 * This example demonstrates how to use the McpClient to connect to GitHub's MCP server
 * and fetch issues from a repository.
 *
 * Prerequisites:
 * 1. Set SECRET_GITHUB_TOKEN in your .env file
 *    Get token from: https://github.com/settings/tokens
 *    Required scopes: 'repo' (for private repos) or 'public_repo' (for public repos only)
 *
 * 2. GitHub MCP Server URL: https://api.githubcopilot.com/mcp/
 *
 * Usage:
 * - This script can be used as a reference for implementing MCP client functionality
 * - The McpClient class is available globally in privileged scripts
 */

/**
 * McpClient wrapper class for easier JavaScript usage
 */
class GitHubMcpClient {
  constructor(serverUrl, secretIdentifier) {
    // Call the native constructor which returns JSON string
    const clientDataJson = McpClient.constructor(serverUrl, secretIdentifier);
    this._clientData = JSON.parse(clientDataJson);
  }

  /**
   * List all available tools from the MCP server
   * @returns {Array} Array of tool objects with name, description, and inputSchema
   */
  listTools() {
    const clientDataJson = JSON.stringify(this._clientData);
    const toolsJson = McpClient._listTools(clientDataJson);
    return JSON.parse(toolsJson);
  }

  /**
   * Call a tool on the MCP server
   * @param {string} toolName - Name of the tool to call
   * @param {object} args - Tool arguments
   * @returns {object} Tool result or error object
   */
  callTool(toolName, args) {
    const clientDataJson = JSON.stringify(this._clientData);
    const argsJson = JSON.stringify(args);
    const resultJson = McpClient._callTool(clientDataJson, toolName, argsJson);
    const result = JSON.parse(resultJson);

    // Check for JSON-RPC errors
    if (result.error) {
      console.error(
        `MCP Tool Error [${result.error.code}]: ${result.error.message}`,
      );
      return result;
    }

    return result;
  }
}

/**
 * Main function to demonstrate GitHub MCP integration
 */
function demonstrateGitHubMcp() {
  try {
    console.log("=== GitHub MCP Client Demo ===\n");

    // 1. Create MCP client connected to GitHub's MCP server
    console.log("1. Connecting to GitHub MCP server...");
    const client = new GitHubMcpClient(
      "https://api.githubcopilot.com/mcp/",
      "github_token",
    );
    console.log("   ✓ Connected\n");

    // 2. List available tools
    console.log("2. Discovering available tools...");
    const tools = client.listTools();
    console.log(`   ✓ Found ${tools.length} tools\n`);

    // Display first few tools
    console.log("   Available tools:");
    tools.slice(0, 10).forEach((tool) => {
      console.log(`   - ${tool.name}: ${tool.description || "No description"}`);
    });
    if (tools.length > 10) {
      console.log(`   ... and ${tools.length - 10} more tools\n`);
    }

    // 3. Fetch a specific issue from GitHub MCP Server repository
    console.log("\n3. Fetching issue #1 from github/github-mcp-server...");
    const issueResult = client.callTool("issue_read:get", {
      owner: "github",
      repo: "github-mcp-server",
      issue_number: 1,
    });

    // Check for errors
    if (issueResult.error) {
      console.log(`   ✗ Error: ${issueResult.error.message}`);
      return {
        success: false,
        error: issueResult.error.message,
      };
    }

    // Display issue details
    const issue = issueResult;
    console.log("   ✓ Issue fetched successfully\n");
    console.log("   Issue Details:");
    console.log(`   - Title: ${issue.title || "N/A"}`);
    console.log(`   - State: ${issue.state || "N/A"}`);
    console.log(`   - Author: ${issue.user?.login || "N/A"}`);
    console.log(`   - Created: ${issue.created_at || "N/A"}`);
    console.log(`   - Comments: ${issue.comments || 0}`);

    if (issue.body) {
      const bodyPreview = issue.body.substring(0, 200);
      console.log(
        `   - Body: ${bodyPreview}${issue.body.length > 200 ? "..." : ""}`,
      );
    }

    console.log("\n=== Demo Complete ===");

    return {
      success: true,
      toolCount: tools.length,
      issue: {
        number: issue.number,
        title: issue.title,
        state: issue.state,
        author: issue.user?.login,
      },
    };
  } catch (error) {
    console.error("\n✗ Error:", error.message);
    console.error("Stack:", error.stack);

    return {
      success: false,
      error: error.message,
    };
  }
}

/**
 * Example: List all open issues from a repository
 */
function listRepositoryIssues(owner, repo, state = "open") {
  try {
    const client = new GitHubMcpClient(
      "https://api.githubcopilot.com/mcp/",
      "github_token",
    );

    console.log(`\nListing ${state} issues for ${owner}/${repo}...\n`);

    // Note: The exact tool name and arguments depend on the GitHub MCP server implementation
    // This is an example - adjust based on actual available tools
    const result = client.callTool("issue_read:list", {
      owner: owner,
      repo: repo,
      state: state,
    });

    if (result.error) {
      console.error(`Error: ${result.error.message}`);
      return [];
    }

    const issues = result.issues || [];

    console.log(`Found ${issues.length} ${state} issues:\n`);
    issues.forEach((issue, index) => {
      console.log(`${index + 1}. #${issue.number} - ${issue.title}`);
      console.log(`   Author: ${issue.user?.login}, State: ${issue.state}`);
    });

    return issues;
  } catch (error) {
    console.error("Error:", error.message);
    return [];
  }
}

/**
 * Export functions for use in other scripts or as HTTP handlers
 */
// Uncomment to register as HTTP endpoint:
// routeRegistry.registerRoute('/github-mcp-demo', 'demonstrateGitHubMcp', 'GET');
// routeRegistry.registerRoute('/github-issues/:owner/:repo', 'listRepositoryIssues', 'GET');

// For testing, run the demo
// demonstrateGitHubMcp();
