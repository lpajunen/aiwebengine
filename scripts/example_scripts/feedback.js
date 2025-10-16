// Example feedback script demonstrating form handling
// This script registers a /feedback endpoint with GET (form) and POST (submission) handlers

function feedback_form_handler(req) {
  const html = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>aiwebengine Feedback</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            margin: 0;
            padding: 20px;
            min-height: 100vh;
        }
        .form-container {
            max-width: 600px;
            margin: 0 auto;
            background: white;
            border-radius: 10px;
            padding: 40px;
            box-shadow: 0 10px 30px rgba(0,0,0,0.1);
        }
        h1 {
            color: #2c3e50;
            text-align: center;
            margin-bottom: 30px;
        }
        .form-group {
            margin-bottom: 20px;
        }
        label {
            display: block;
            margin-bottom: 5px;
            color: #2c3e50;
            font-weight: 500;
        }
        input, textarea, select {
            width: 100%;
            padding: 12px;
            border: 2px solid #e1e8ed;
            border-radius: 5px;
            font-size: 16px;
            transition: border-color 0.3s;
            box-sizing: border-box;
        }
        input:focus, textarea:focus, select:focus {
            outline: none;
            border-color: #3498db;
        }
        textarea {
            resize: vertical;
            min-height: 100px;
        }
        .rating-group {
            display: flex;
            gap: 10px;
            flex-wrap: wrap;
        }
        .rating-option {
            flex: 1;
            min-width: 80px;
        }
        .rating-option input[type="radio"] {
            display: none;
        }
        .rating-option label {
            display: block;
            padding: 10px;
            text-align: center;
            background: #f8f9fa;
            border: 2px solid #e1e8ed;
            border-radius: 5px;
            cursor: pointer;
            transition: all 0.3s;
        }
        .rating-option input[type="radio"]:checked + label {
            background: #3498db;
            color: white;
            border-color: #3498db;
        }
        button {
            width: 100%;
            padding: 15px;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: white;
            border: none;
            border-radius: 5px;
            font-size: 16px;
            font-weight: 600;
            cursor: pointer;
            transition: transform 0.2s;
        }
        button:hover {
            transform: translateY(-2px);
        }
        .back-link {
            text-align: center;
            margin-top: 20px;
        }
        .back-link a {
            color: #7f8c8d;
            text-decoration: none;
        }
        .back-link a:hover {
            text-decoration: underline;
        }
    </style>
</head>
<body>
    <div class="form-container">
        <h1>💬 Share Your Feedback</h1>
        <form method="POST" action="/feedback">
            <div class="form-group">
                <label for="name">Name *</label>
                <input type="text" id="name" name="name" required>
            </div>

            <div class="form-group">
                <label for="email">Email *</label>
                <input type="email" id="email" name="email" required>
            </div>

            <div class="form-group">
                <label>Overall Experience</label>
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
                <label for="category">Category</label>
                <select id="category" name="category">
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
                <label for="message">Message *</label>
                <textarea id="message" name="message" placeholder="Tell us what you think..." required></textarea>
            </div>

            <button type="submit">Submit Feedback</button>
        </form>

        <div class="back-link">
            <a href="/blog">← Back to Blog</a>
        </div>
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
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            margin: 0;
            padding: 20px;
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
        }
        .thank-you {
            background: white;
            border-radius: 10px;
            padding: 40px;
            text-align: center;
            box-shadow: 0 10px 30px rgba(0,0,0,0.1);
            max-width: 500px;
        }
        h1 {
            color: #2c3e50;
            margin-bottom: 20px;
        }
        p {
            color: #7f8c8d;
            margin-bottom: 30px;
            line-height: 1.6;
        }
        .emoji {
            font-size: 3em;
            margin-bottom: 20px;
        }
        .actions {
            display: flex;
            gap: 15px;
            justify-content: center;
            flex-wrap: wrap;
        }
        a {
            padding: 12px 24px;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: white;
            text-decoration: none;
            border-radius: 5px;
            transition: transform 0.2s;
        }
        a:hover {
            transform: translateY(-2px);
        }
    </style>
</head>
<body>
    <div class="thank-you">
        <div class="emoji">🙏</div>
        <h1>Thank You!</h1>
        <p>Thank you for your feedback, <strong>${name}</strong>! We appreciate you taking the time to share your thoughts about aiwebengine.</p>
        <p>Your input helps us improve and build better tools for developers like you.</p>

        <div class="actions">
            <a href="/blog">Read the Blog</a>
            <a href="/feedback">Submit More Feedback</a>
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

// Register both GET (form) and POST (submission) handlers
register("/feedback", "feedback_form_handler", "GET");
register("/feedback", "feedback_submit_handler", "POST");
