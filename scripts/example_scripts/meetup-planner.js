/// <reference path="../../assets/aiwebengine.d.ts" />

// Meetup Planner Example Script
// Demonstrates creating and managing meetups with public sharing and member responses

// ============================================
// Storage Helper Functions
// ============================================

function loadMeetups() {
  try {
    const data = sharedStorage.getItem("meetups");
    return data ? JSON.parse(data) : [];
  } catch (error) {
    console.error("Error loading meetups: " + error);
    return [];
  }
}

function saveMeetups(meetups) {
  try {
    sharedStorage.setItem("meetups", JSON.stringify(meetups));
    return true;
  } catch (error) {
    console.error("Error saving meetups: " + error);
    return false;
  }
}

function getMeetupById(id) {
  const meetups = loadMeetups();
  return meetups.find((m) => m.id === id);
}

function saveMeetup(meetup) {
  const meetups = loadMeetups();
  const index = meetups.findIndex((m) => m.id === meetup.id);
  if (index >= 0) {
    meetups[index] = meetup;
  } else {
    meetups.push(meetup);
  }
  return saveMeetups(meetups);
}

// ============================================
// HTTP Handlers
// ============================================

function meetup_handler(context) {
  const html = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Meetup Planner</title>
    <link rel="stylesheet" href="/engine.css">
    <style>
        body {
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            padding: 2rem 0;
        }
        .container {
            max-width: 800px;
            margin: 0 auto;
            background: rgba(255, 255, 255, 0.95);
            backdrop-filter: blur(10px);
            border-radius: var(--border-radius-lg);
            box-shadow: var(--shadow-lg);
            padding: 3rem;
            text-align: center;
        }
        h1 {
            color: var(--text-color);
            margin-bottom: 1rem;
        }
        p {
            color: var(--text-muted);
            font-size: 1.1rem;
            margin-bottom: 2rem;
        }
        .actions {
            display: flex;
            gap: 1rem;
            justify-content: center;
            flex-wrap: wrap;
        }
        .btn {
            padding: 1rem 2rem;
            border: none;
            border-radius: var(--border-radius);
            font-size: 1rem;
            cursor: pointer;
            text-decoration: none;
            display: inline-block;
            transition: var(--transition);
        }
        .btn-primary {
            background: var(--primary-color);
            color: white;
        }
        .btn-primary:hover {
            background: var(--primary-hover);
        }
        .btn-secondary {
            background: var(--bg-secondary);
            color: var(--text-color);
        }
        .btn-secondary:hover {
            background: var(--border-color);
        }
    </style>
</head>
<body>
    <div class="container">
        <h1>📅 Meetup Planner</h1>
        <p>Plan and organize meetups with friends and colleagues. Create events, share public links, and get responses from participants.</p>
        <div class="actions">
            <a href="/meetup/dashboard" class="btn btn-primary">Go to Dashboard</a>
            <a href="/auth/login?redirect=/meetup/dashboard" class="btn btn-secondary">Login to Get Started</a>
        </div>
    </div>
</body>
</html>`;

  return ResponseBuilder.html(html);
}

function meetup_dashboard_handler(context) {
  const req = context.request || {};

  // Require authentication
  if (!req.auth || !req.auth.isAuthenticated) {
    const currentPath = encodeURIComponent(req.path || "/meetup/dashboard");
    const loginUrl = "/auth/login?redirect=" + currentPath;
    return ResponseBuilder.redirect(loginUrl);
  }

  const user = req.auth.user;

  // Load user's meetup IDs from sharedStorage (personal data with user prefix)
  let userMeetupIds = [];
  try {
    const userKey = "personal_" + user.id + "_meetups";
    const stored = sharedStorage.getItem(userKey);
    userMeetupIds = stored ? JSON.parse(stored) : [];
  } catch (error) {
    console.error("Error loading meetup keys from storage:", error);
  }

  // Load meetup details from sharedStorage
  const allMeetups = loadMeetups();
  const userMeetups = allMeetups.filter((m) => userMeetupIds.includes(m.id));

  const meetupsHtml = userMeetups
    .map((meetup) => {
      const members = Object.values(meetup.members || {});
      const agreeCount = members.filter((m) => m.response === "agree").length;
      const totalMembers = members.length;

      return `
    <div class="meetup-card">
        <h3>${meetup.name}</h3>
        <p>${meetup.description}</p>
        <div class="meetup-meta">
            <small>Created by ${meetup.createdByName || "Unknown"} • ${agreeCount}/${totalMembers} agreed</small>
        </div>
        <div class="meetup-actions">
            <a href="/meetup/join/${meetup.id}" class="btn btn-secondary">View Details</a>
            <button onclick="copyLink('${meetup.id}')" class="btn btn-secondary">Copy Share Link</button>
        </div>
    </div>`;
    })
    .join("");

  const html = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Meetup Dashboard</title>
    <link rel="stylesheet" href="/engine.css">
    <style>
        body {
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            padding: 2rem 0;
        }
        .container {
            max-width: 1000px;
            margin: 0 auto;
            background: rgba(255, 255, 255, 0.95);
            backdrop-filter: blur(10px);
            border-radius: var(--border-radius-lg);
            box-shadow: var(--shadow-lg);
            padding: 3rem;
        }
        .header {
            display: flex;
            justify-content: space-between;
            align-items: center;
            margin-bottom: 2rem;
        }
        h1 {
            color: var(--text-color);
            margin: 0;
        }
        .user-info {
            color: var(--text-muted);
            font-size: 0.9rem;
        }
        .create-form {
            background: var(--bg-secondary);
            padding: 2rem;
            border-radius: var(--border-radius);
            margin-bottom: 2rem;
        }
        .form-group {
            margin-bottom: 1rem;
        }
        label {
            display: block;
            margin-bottom: 0.5rem;
            font-weight: 500;
        }
        input, textarea {
            width: 100%;
            padding: 0.75rem;
            border: 1px solid var(--border-color);
            border-radius: var(--border-radius);
            font-size: 1rem;
        }
        .btn {
            padding: 0.75rem 1.5rem;
            border: none;
            border-radius: var(--border-radius);
            font-size: 1rem;
            cursor: pointer;
            text-decoration: none;
            display: inline-block;
            transition: var(--transition);
        }
        .btn-primary {
            background: var(--primary-color);
            color: white;
        }
        .btn-primary:hover {
            background: var(--primary-hover);
        }
        .btn-secondary {
            background: var(--bg-secondary);
            color: var(--text-color);
        }
        .btn-secondary:hover {
            background: var(--border-color);
        }
        .meetups-grid {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
            gap: 1.5rem;
        }
        .meetup-card {
            background: var(--bg-secondary);
            padding: 1.5rem;
            border-radius: var(--border-radius);
            border-left: 4px solid var(--primary-color);
        }
        .meetup-card h3 {
            margin-top: 0;
            color: var(--text-color);
        }
        .meetup-card p {
            color: var(--text-muted);
            margin-bottom: 1rem;
        }
        .meetup-meta {
            margin-bottom: 1rem;
        }
        .meetup-actions {
            display: flex;
            gap: 0.5rem;
            flex-wrap: wrap;
        }
        .empty-state {
            text-align: center;
            padding: 3rem;
            color: var(--text-muted);
        }
    </style>
</head>
<body>
    <div class="container">
        <div class="header">
            <h1>📅 Your Meetups</h1>
            <div class="user-info">Logged in as: ${user.name || user.email}</div>
        </div>
        
        <div class="create-form">
            <h2>Create New Meetup</h2>
            <form id="create-form">
                <div class="form-group">
                    <label for="name">Meetup Name</label>
                    <input type="text" id="name" name="name" required maxlength="100">
                </div>
                <div class="form-group">
                    <label for="description">Description</label>
                    <textarea id="description" name="description" rows="3" required maxlength="500"></textarea>
                </div>
                <button type="submit" class="btn btn-primary">Create Meetup</button>
            </form>
        </div>
        
        <div class="meetups-grid">
            ${userMeetups.length > 0 ? meetupsHtml : '<div class="empty-state"><h3>No meetups yet</h3><p>Create your first meetup above!</p></div>'}
        </div>
    </div>

    <script>
        document.getElementById('create-form').addEventListener('submit', async (e) => {
            e.preventDefault();
            const formData = new FormData(e.target);
            const name = formData.get('name').trim();
            const description = formData.get('description').trim();
            
            if (!name || !description) return;
            
            try {
                const response = await fetch('/meetup/create', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ name, description })
                });
                
                if (response.ok) {
                    location.reload();
                } else {
                    alert('Failed to create meetup');
                }
            } catch (error) {
                console.error('Error:', error);
                alert('Error creating meetup');
            }
        });
        
        function copyLink(meetupId) {
            const link = window.location.origin + '/meetup/join/' + meetupId;
            navigator.clipboard.writeText(link).then(() => {
                alert('Link copied to clipboard!');
            });
        }
    </script>
</body>
</html>`;

  return ResponseBuilder.html(html);
}

function create_meetup_handler(context) {
  const req = context.request || {};

  if (!req.auth || !req.auth.isAuthenticated) {
    return ResponseBuilder.error(401, "Unauthorized");
  }

  if (req.method !== "POST") {
    return ResponseBuilder.error(405, "Method not allowed");
  }

  try {
    const body = JSON.parse(req.body || "{}");
    const { name, description } = body;

    if (!name || !description) {
      return ResponseBuilder.error(400, "Name and description required");
    }

    const user = req.auth.user;
    const meetupId =
      "meetup_" + Date.now() + "_" + Math.random().toString(36).substr(2, 9);

    const meetup = {
      id: meetupId,
      name: name.trim(),
      description: description.trim(),
      createdBy: user.id,
      createdByName: user.name || user.email,
      createdAt: new Date().toISOString(),
      members: {
        [user.id]: {
          name: user.name || user.email,
          email: user.email,
          response: "agree", // Creator automatically agrees to attend
        },
      },
    };

    saveMeetup(meetup);

    // Store meetup key in sharedStorage (using user prefix for personal data)
    try {
      const userKey = "personal_" + user.id + "_meetups";
      const existing = sharedStorage.getItem(userKey);
      const meetupIds = existing ? JSON.parse(existing) : [];
      meetupIds.push(meetupId);
      sharedStorage.setItem(userKey, JSON.stringify(meetupIds));
    } catch (error) {
      console.error("Error storing meetup key in storage:", error);
    }

    return ResponseBuilder.json({ id: meetupId }, 201);
  } catch (error) {
    console.error("Error creating meetup:", error);
    return ResponseBuilder.error(500, "Internal server error");
  }
}

function join_meetup_handler(context) {
  const req = context.request || {};
  const path = req.path || "";
  const meetupId = path.split("/meetup/join/")[1];

  if (!meetupId) {
    return ResponseBuilder.error(404, "Meetup not found");
  }

  const meetup = getMeetupById(meetupId);
  if (!meetup) {
    return ResponseBuilder.error(404, "Meetup not found");
  }

  // Require authentication
  if (!req.auth || !req.auth.isAuthenticated) {
    const currentPath = encodeURIComponent(req.path);
    const loginUrl = "/auth/login?redirect=" + currentPath;
    return ResponseBuilder.redirect(loginUrl);
  }

  const user = req.auth.user;

  // Add user to members if not already
  if (!meetup.members) meetup.members = {};
  if (!meetup.members[user.id]) {
    meetup.members[user.id] = {
      name: user.name || user.email,
      email: user.email,
      response: "pending",
    };
    saveMeetup(meetup);

    // Store meetup key in sharedStorage (using user prefix for personal data)
    try {
      const userKey = "personal_" + user.id + "_meetups";
      const existing = sharedStorage.getItem(userKey);
      const meetupIds = existing ? JSON.parse(existing) : [];
      if (!meetupIds.includes(meetupId)) {
        meetupIds.push(meetupId);
        sharedStorage.setItem(userKey, JSON.stringify(meetupIds));
      }
    } catch (error) {
      console.error("Error storing meetup key in storage:", error);
    }
  }

  const member = meetup.members[user.id];
  const members = Object.values(meetup.members || {});
  const agreeCount = members.filter((m) => m.response === "agree").length;
  const disagreeCount = members.filter((m) => m.response === "disagree").length;
  const pendingCount = members.filter((m) => m.response === "pending").length;

  const membersHtml = members
    .map(
      (m) => `
    <div class="member">
        <span class="member-name">${m.name || m.email}</span>
        <span class="member-response response-${m.response}">${m.response}</span>
    </div>
  `,
    )
    .join("");

  const html = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>${meetup.name}</title>
    <link rel="stylesheet" href="/engine.css">
    <style>
        body {
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            padding: 2rem 0;
        }
        .container {
            max-width: 800px;
            margin: 0 auto;
            background: rgba(255, 255, 255, 0.95);
            backdrop-filter: blur(10px);
            border-radius: var(--border-radius-lg);
            box-shadow: var(--shadow-lg);
            padding: 3rem;
        }
        .meetup-header {
            text-align: center;
            margin-bottom: 2rem;
        }
        h1 {
            color: var(--text-color);
            margin-bottom: 0.5rem;
        }
        .meetup-meta {
            color: var(--text-muted);
            font-size: 0.9rem;
        }
        .meetup-description {
            background: var(--bg-secondary);
            padding: 1.5rem;
            border-radius: var(--border-radius);
            margin-bottom: 2rem;
        }
        .response-section {
            background: var(--bg-secondary);
            padding: 2rem;
            border-radius: var(--border-radius);
            margin-bottom: 2rem;
        }
        .response-buttons {
            display: flex;
            gap: 1rem;
            justify-content: center;
        }
        .btn {
            padding: 0.75rem 1.5rem;
            border: none;
            border-radius: var(--border-radius);
            font-size: 1rem;
            cursor: pointer;
            text-decoration: none;
            display: inline-block;
            transition: var(--transition);
        }
        .btn-agree {
            background: #28a745;
            color: white;
        }
        .btn-agree:hover {
            background: #218838;
        }
        .btn-disagree {
            background: #dc3545;
            color: white;
        }
        .btn-disagree:hover {
            background: #c82333;
        }
        .btn-secondary {
            background: var(--bg-secondary);
            color: var(--text-color);
        }
        .btn-secondary:hover {
            background: var(--border-color);
        }
        .members-section {
            background: var(--bg-secondary);
            padding: 2rem;
            border-radius: var(--border-radius);
        }
        .members-summary {
            display: flex;
            justify-content: space-around;
            margin-bottom: 1rem;
            text-align: center;
        }
        .summary-item {
            font-weight: 500;
        }
        .summary-number {
            font-size: 1.5rem;
            color: var(--primary-color);
        }
        .members-list {
            max-height: 300px;
            overflow-y: auto;
        }
        .member {
            display: flex;
            justify-content: space-between;
            align-items: center;
            padding: 0.5rem 0;
            border-bottom: 1px solid var(--border-color);
        }
        .member-name {
            font-weight: 500;
        }
        .member-response {
            font-size: 0.9rem;
            padding: 0.25rem 0.5rem;
            border-radius: 0.25rem;
        }
        .response-pending {
            background: #fff3cd;
            color: #856404;
        }
        .response-agree {
            background: #d4edda;
            color: #155724;
        }
        .response-disagree {
            background: #f8d7da;
            color: #721c24;
        }
    </style>
</head>
<body>
    <div class="container">
        <div class="meetup-header">
            <h1>${meetup.name}</h1>
            <div class="meetup-meta">Created by ${meetup.createdByName} on ${new Date(meetup.createdAt).toLocaleDateString()}</div>
        </div>
        
        <div class="meetup-description">
            <p>${meetup.description}</p>
        </div>
        
        <div class="response-section">
            <h2>Your Response</h2>
            <p>Let others know if you can make it to this meetup.</p>
            <div class="response-buttons">
                <button onclick="respond('agree')" class="btn btn-agree" ${member.response === "agree" ? "disabled" : ""}>I'll Attend</button>
                <button onclick="respond('disagree')" class="btn btn-disagree" ${member.response === "disagree" ? "disabled" : ""}>Can't Make It</button>
            </div>
        </div>
        
        <div class="members-section">
            <h2>Members (${members.length})</h2>
            <div class="members-summary">
                <div class="summary-item">
                    <div class="summary-number">${agreeCount}</div>
                    <div>Attending</div>
                </div>
                <div class="summary-item">
                    <div class="summary-number">${pendingCount}</div>
                    <div>Pending</div>
                </div>
                <div class="summary-item">
                    <div class="summary-number">${disagreeCount}</div>
                    <div>Not Attending</div>
                </div>
            </div>
            <div class="members-list">
                ${membersHtml}
            </div>
        </div>
        
        <div style="text-align: center; margin-top: 2rem;">
            <a href="/meetup/dashboard" class="btn btn-secondary">Back to Dashboard</a>
        </div>
    </div>

    <script>
        async function respond(response) {
            try {
                const res = await fetch('/meetup/${meetupId}/response', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ response })
                });
                
                if (res.ok) {
                    location.reload();
                } else {
                    alert('Failed to update response');
                }
            } catch (error) {
                console.error('Error:', error);
                alert('Error updating response');
            }
        }
    </script>
</body>
</html>`;

  return ResponseBuilder.html(html);
}

function update_response_handler(context) {
  const req = context.request || {};
  const path = req.path || "";
  const meetupId = path.split("/meetup/")[1]?.split("/response")[0];

  if (!meetupId || req.method !== "POST") {
    return ResponseBuilder.error(400, "Bad request");
  }

  if (!req.auth || !req.auth.isAuthenticated) {
    return ResponseBuilder.error(401, "Unauthorized");
  }

  try {
    const body = JSON.parse(req.body || "{}");
    const { response } = body;

    if (!["agree", "disagree"].includes(response)) {
      return ResponseBuilder.error(400, "Invalid response");
    }

    const user = req.auth.user;
    const meetup = getMeetupById(meetupId);

    if (!meetup || !meetup.members || !meetup.members[user.id]) {
      return ResponseBuilder.error(404, "Meetup or membership not found");
    }

    meetup.members[user.id].response = response;
    saveMeetup(meetup);

    return ResponseBuilder.json({ success: true });
  } catch (error) {
    console.error("Error updating response:", error);
    return ResponseBuilder.error(500, "Internal server error");
  }
}

// ============================================
// Initialization
// ============================================

function init(context) {
  console.log("Initializing meetup-planner.js at " + new Date().toISOString());

  try {
    // Register HTTP routes
    routeRegistry.registerRoute("/meetup", "meetup_handler", "GET");
    routeRegistry.registerRoute(
      "/meetup/dashboard",
      "meetup_dashboard_handler",
      "GET",
    );
    routeRegistry.registerRoute(
      "/meetup/create",
      "create_meetup_handler",
      "POST",
    );
    routeRegistry.registerRoute(
      "/meetup/join/:id",
      "join_meetup_handler",
      "GET",
    );
    routeRegistry.registerRoute(
      "/meetup/:id/response",
      "update_response_handler",
      "POST",
    );

    console.log("Meetup planner initialized successfully");
    console.log("Routes registered:");
    console.log("- GET /meetup (public landing page)");
    console.log("- GET /meetup/dashboard (requires login)");
    console.log("- POST /meetup/create (create new meetup)");
    console.log("- GET /meetup/join/:id (join meetup via public link)");
    console.log("- POST /meetup/:id/response (update attendance response)");

    return {
      success: true,
      message: "Meetup planner initialized",
      routes: [
        "/meetup",
        "/meetup/dashboard",
        "/meetup/create",
        "/meetup/join/:id",
        "/meetup/:id/response",
      ],
    };
  } catch (error) {
    console.error("Error initializing meetup planner: " + error);
    throw error;
  }
}
