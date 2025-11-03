// Example feedback script demonstrating form handling
// This script registers a /feedback endpoint with GET (form) and POST (submission) handlers

function feedback_form_handler(req) {
  const html = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>aiwebengine Feedback</title>
    <link rel="stylesheet" href="/engine.css">
    <link rel="icon" type="image/x-icon" href="/favicon.ico">
    <style>
        /* Feedback form specific overrides */
        body {
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            padding: 2rem 0;
        }

        .feedback-container {
            max-width: 600px;
            margin: 0 auto;
            background: rgba(255, 255, 255, 0.95);
            backdrop-filter: blur(10px);
            border-radius: var(--border-radius-lg);
            box-shadow: var(--shadow-lg);
            overflow: hidden;
        }

        .feedback-header {
            background: var(--bg-secondary);
            padding: 2rem;
            text-align: center;
            border-bottom: 1px solid var(--border-color);
        }

        .feedback-content {
            padding: 2rem;
        }

        .rating-group {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(100px, 1fr));
            gap: 0.75rem;
            margin-top: 0.5rem;
        }

        .rating-option input[type="radio"] {
            display: none;
        }

        .rating-option label {
            display: block;
            padding: 0.75rem;
            text-align: center;
            background: var(--bg-secondary);
            border: 2px solid var(--border-color);
            border-radius: var(--border-radius);
            cursor: pointer;
            transition: var(--transition);
            font-weight: 500;
        }

        .rating-option input[type="radio"]:checked + label {
            background: var(--primary-color);
            color: white;
            border-color: var(--primary-color);
        }

        .rating-option label:hover {
            border-color: var(--primary-color);
        }

        .submit-btn {
            width: 100%;
            padding: 1rem;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: white;
            border: none;
            border-radius: var(--border-radius);
            font-size: 1rem;
            font-weight: 600;
            cursor: pointer;
            transition: var(--transition);
            margin-top: 1rem;
        }

        .submit-btn:hover {
            transform: translateY(-2px);
            box-shadow: var(--shadow);
        }

        .back-link {
            text-align: center;
            margin-top: 2rem;
            padding-top: 1rem;
            border-top: 1px solid var(--border-color);
        }

        .back-link a {
            color: var(--text-muted);
            text-decoration: none;
            font-weight: 500;
        }

        .back-link a:hover {
            color: var(--primary-color);
            text-decoration: underline;
        }

        @media (max-width: 768px) {
            .feedback-content {
                padding: 1rem;
            }

            .feedback-header {
                padding: 1rem;
            }

            .rating-group {
                grid-template-columns: 1fr;
            }
        }
    </style>
</head>
<body>
    <div class="feedback-container">
        <header class="feedback-header">
            <h1>💬 Share Your Feedback</h1>
        </header>

        <main class="feedback-content">
            <form method="POST" action="/feedback">
                <div class="form-group">
                    <label for="name" class="form-label">Name *</label>
                    <input type="text" id="name" name="name" class="form-control" required>
                </div>

                <div class="form-group">
                    <label for="email" class="form-label">Email *</label>
                    <input type="email" id="email" name="email" class="form-control" required>
                </div>

                <div class="form-group">
                    <label class="form-label">Overall Experience</label>
                    <div class="rating-group">
                        <div class="rating-option">
                            <input type="radio" id="rating-1" name="rating" value="1">
                            <label for="rating-1">😞 Poor</label>
                        </div>
                        <div class="rating-option">
                            <input type="radio" id="rating-2" name="rating" value="2">
                            <label for="rating-2">😐 Fair</label>
                        </div>
                        <div class="rating-option">
                            <input type="radio" id="rating-3" name="rating" value="3">
                            <label for="rating-3">🙂 Good</label>
                        </div>
                        <div class="rating-option">
                            <input type="radio" id="rating-4" name="rating" value="4">
                            <label for="rating-4">😊 Great</label>
                        </div>
                        <div class="rating-option">
                            <input type="radio" id="rating-5" name="rating" value="5" checked>
                            <label for="rating-5">🤩 Excellent</label>
                        </div>
                    </div>
                </div>

                <div class="form-group">
                    <label for="category" class="form-label">Category</label>
                    <select id="category" name="category" class="form-control">
                        <option value="">Select a category</option>
                        <option value="bug">🐛 Bug Report</option>
                        <option value="feature">✨ Feature Request</option>
                        <option value="documentation">📚 Documentation</option>
                        <option value="performance">⚡ Performance</option>
                        <option value="usability">🎯 Usability</option>
                        <option value="other">❓ Other</option>
                    </select>
                </div>

                <div class="form-group">
                    <label for="message" class="form-label">Message *</label>
                    <textarea id="message" name="message" class="form-control" placeholder="Tell us what you think..." required></textarea>
                </div>

                <button type="submit" class="submit-btn">Submit Feedback</button>
            </form>

            <div class="back-link">
                <a href="/blog">← Back to Blog</a>
            </div>
        </main>
    </div>
</body>
</html>`;

  return {
    status: 200,
    body: html,
    contentType: "text/html",
  };
}

function feedback_submit_handler(req) {
  // Extract form data
  let name = req.form?.name || "Anonymous";
  let email = req.form?.email || "";
  let rating = req.form?.rating || "5";
  let category = req.form?.category || "general";
  let message = req.form?.message || "";

  // Log the feedback (in a real app, you'd store this in a database)
  writeLog("Feedback received:");
  writeLog("Name: " + name);
  writeLog("Email: " + email);
  writeLog("Rating: " + rating);
  writeLog("Category: " + category);
  writeLog("Message: " + message);

  const thankYouHtml = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Thank You - aiwebengine</title>
    <link rel="stylesheet" href="/engine.css">
    <link rel="icon" type="image/x-icon" href="/favicon.ico">
    <style>
        /* Thank you page specific overrides */
        body {
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
            padding: 2rem 0;
        }

        .thank-you-container {
            max-width: 500px;
            margin: 0 auto;
            background: rgba(255, 255, 255, 0.95);
            backdrop-filter: blur(10px);
            border-radius: var(--border-radius-lg);
            box-shadow: var(--shadow-lg);
            overflow: hidden;
        }

        .thank-you-content {
            padding: 3rem 2rem;
            text-align: center;
        }

        .thank-you-emoji {
            font-size: 4rem;
            margin-bottom: 1rem;
        }

        .thank-you-content h1 {
            color: var(--text-color);
            margin-bottom: 1rem;
        }

        .thank-you-content p {
            color: var(--text-muted);
            margin-bottom: 1.5rem;
            line-height: 1.6;
        }

        .thank-you-actions {
            display: flex;
            gap: 1rem;
            justify-content: center;
            flex-wrap: wrap;
            margin-top: 2rem;
        }

        .thank-you-actions a {
            padding: 0.75rem 1.5rem;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: white;
            text-decoration: none;
            border-radius: var(--border-radius);
            font-weight: 500;
            transition: var(--transition);
        }

        .thank-you-actions a:hover {
            transform: translateY(-2px);
            box-shadow: var(--shadow);
        }

        @media (max-width: 768px) {
            .thank-you-content {
                padding: 2rem 1rem;
            }

            .thank-you-actions {
                flex-direction: column;
            }

            .thank-you-actions a {
                width: 100%;
                text-align: center;
            }
        }
    </style>
</head>
<body>
    <div class="thank-you-container">
        <div class="thank-you-content">
            <div class="thank-you-emoji">🙏</div>
            <h1>Thank You!</h1>
            <p>Thank you for your feedback, <strong>${name}</strong>! We appreciate you taking the time to share your thoughts about aiwebengine.</p>
            <p>Your input helps us improve and build better tools for developers like you.</p>

            <div class="thank-you-actions">
                <a href="/blog">Read the Blog</a>
                <a href="/feedback">Submit More Feedback</a>
            </div>
        </div>
    </div>
</body>
</html>`;

  return {
    status: 200,
    body: thankYouHtml,
    contentType: "text/html",
  };
}

// Initialization function - called when script is loaded or updated
function init(context) {
  try {
    writeLog(`Initializing feedback.js script at ${new Date().toISOString()}`);
    writeLog(`Init context: ${JSON.stringify(context)}`);

    // Register both GET (form) and POST (submission) handlers
    register("/feedback", "feedback_form_handler", "GET");
    register("/feedback", "feedback_submit_handler", "POST");

    writeLog("Feedback script initialized successfully");

    return {
      success: true,
      message: "Feedback script initialized successfully",
      registeredEndpoints: 2,
    };
  } catch (error) {
    writeLog(`Feedback script initialization failed: ${error.message}`);
    throw error;
  }
}
