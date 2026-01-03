# Forms and Data Handling

Learn how to handle form submissions, file uploads, and data validation in aiwebengine. This guide covers everything from simple contact forms to complex multi-step wizards.

## Simple Contact Form

A basic form with GET (display) and POST (submit) handlers:

```javascript
function init() {
  routeRegistry.registerRoute("GET", "/contact", showContactForm);
  routeRegistry.registerRoute("POST", "/contact", handleContactForm);
}

function showContactForm(request) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>Contact Us</title>
      <style>
        body {
          font-family: Arial, sans-serif;
          max-width: 600px;
          margin: 50px auto;
          padding: 20px;
        }
        form {
          background: #f9f9f9;
          padding: 30px;
          border-radius: 8px;
        }
        label {
          display: block;
          margin: 15px 0 5px;
          font-weight: bold;
        }
        input, textarea {
          width: 100%;
          padding: 10px;
          border: 1px solid #ddd;
          border-radius: 4px;
          box-sizing: border-box;
        }
        button {
          background: #4CAF50;
          color: white;
          padding: 12px 30px;
          border: none;
          border-radius: 4px;
          cursor: pointer;
          margin-top: 20px;
        }
        button:hover {
          background: #45a049;
        }
      </style>
    </head>
    <body>
      <h1>Contact Us</h1>
      <form method="POST" action="/contact">
        <label>Name *</label>
        <input type="text" name="name" required>
        
        <label>Email *</label>
        <input type="email" name="email" required>
        
        <label>Subject</label>
        <input type="text" name="subject">
        
        <label>Message *</label>
        <textarea name="message" rows="5" required></textarea>
        
        <button type="submit">Send Message</button>
      </form>
    </body>
    </html>
  `;

  return {
    status: 200,
    headers: { "Content-Type": "text/html" },
    body: html,
  };
}

function handleContactForm(request) {
  // Parse form data
  const formData = parseFormData(request.body);

  console.log("Contact form submitted", {
    name: formData.name,
    email: formData.email,
    subject: formData.subject,
  });

  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>Thank You</title>
      <style>
        body {
          font-family: Arial, sans-serif;
          max-width: 600px;
          margin: 50px auto;
          padding: 20px;
          text-align: center;
        }
        .success {
          background: #d4edda;
          color: #155724;
          padding: 20px;
          border-radius: 8px;
          border: 1px solid #c3e6cb;
        }
        a {
          color: #007bff;
          text-decoration: none;
        }
      </style>
    </head>
    <body>
      <div class="success">
        <h1>✓ Thank You!</h1>
        <p>Your message has been received.</p>
        <p><strong>Name:</strong> ${formData.name}</p>
        <p><strong>Email:</strong> ${formData.email}</p>
      </div>
      <p><a href="/contact">Send another message</a></p>
    </body>
    </html>
  `;

  return {
    status: 200,
    headers: { "Content-Type": "text/html" },
    body: html,
  };
}

function parseFormData(body) {
  const data = {};
  const pairs = body.split("&");

  for (const pair of pairs) {
    const [key, value] = pair.split("=");
    data[decodeURIComponent(key)] = decodeURIComponent(
      value.replace(/\+/g, " "),
    );
  }

  return data;
}

init();
```

## Form with Validation

Add server-side validation:

```javascript
function init() {
  routeRegistry.registerRoute("GET", "/signup", showSignupForm);
  routeRegistry.registerRoute("POST", "/signup", handleSignup);
}

function showSignupForm(request) {
  const params = parseQueryParams(request.url);
  const errors = params.errors
    ? JSON.parse(decodeURIComponent(params.errors))
    : [];
  const values = params.values
    ? JSON.parse(decodeURIComponent(params.values))
    : {};

  const errorHtml =
    errors.length > 0
      ? `
    <div class="errors">
      <strong>Please fix the following errors:</strong>
      <ul>
        ${errors.map((e) => `<li>${e}</li>`).join("")}
      </ul>
    </div>
  `
      : "";

  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>Sign Up</title>
      <style>
        body {
          font-family: Arial, sans-serif;
          max-width: 500px;
          margin: 50px auto;
          padding: 20px;
        }
        .errors {
          background: #f8d7da;
          color: #721c24;
          padding: 15px;
          border-radius: 4px;
          margin-bottom: 20px;
          border: 1px solid #f5c6cb;
        }
        .errors ul {
          margin: 10px 0 0 0;
        }
        form {
          background: #f9f9f9;
          padding: 30px;
          border-radius: 8px;
        }
        label {
          display: block;
          margin: 15px 0 5px;
          font-weight: bold;
        }
        input, select {
          width: 100%;
          padding: 10px;
          border: 1px solid #ddd;
          border-radius: 4px;
          box-sizing: border-box;
        }
        button {
          background: #007bff;
          color: white;
          padding: 12px 30px;
          border: none;
          border-radius: 4px;
          cursor: pointer;
          margin-top: 20px;
        }
        .help-text {
          font-size: 0.9em;
          color: #666;
          margin-top: 5px;
        }
      </style>
    </head>
    <body>
      <h1>Sign Up</h1>
      ${errorHtml}
      <form method="POST" action="/signup">
        <label>Username *</label>
        <input type="text" name="username" value="${values.username || ""}" required>
        <div class="help-text">3-20 characters, letters and numbers only</div>
        
        <label>Email *</label>
        <input type="email" name="email" value="${values.email || ""}" required>
        
        <label>Password *</label>
        <input type="password" name="password" required>
        <div class="help-text">Minimum 8 characters</div>
        
        <label>Confirm Password *</label>
        <input type="password" name="confirmPassword" required>
        
        <label>Age *</label>
        <input type="number" name="age" value="${values.age || ""}" min="13" max="120" required>
        
        <label>Country *</label>
        <select name="country" required>
          <option value="">Select...</option>
          <option value="US" ${values.country === "US" ? "selected" : ""}>United States</option>
          <option value="UK" ${values.country === "UK" ? "selected" : ""}>United Kingdom</option>
          <option value="CA" ${values.country === "CA" ? "selected" : ""}>Canada</option>
          <option value="AU" ${values.country === "AU" ? "selected" : ""}>Australia</option>
        </select>
        
        <button type="submit">Create Account</button>
      </form>
    </body>
    </html>
  `;

  return {
    status: 200,
    headers: { "Content-Type": "text/html" },
    body: html,
  };
}

function handleSignup(request) {
  const formData = parseFormData(request.body);

  // Validate
  const errors = validateSignupForm(formData);

  if (errors.length > 0) {
    // Redirect back with errors
    const errorParam = encodeURIComponent(JSON.stringify(errors));
    const valuesParam = encodeURIComponent(
      JSON.stringify({
        username: formData.username,
        email: formData.email,
        age: formData.age,
        country: formData.country,
      }),
    );

    return {
      status: 302,
      headers: {
        Location: `/signup?errors=${errorParam}&values=${valuesParam}`,
      },
      body: "",
    };
  }

  // Success - create account (mock)
  console.log("New user signup", {
    username: formData.username,
    email: formData.email,
    country: formData.country,
  });

  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>Welcome!</title>
      <style>
        body {
          font-family: Arial, sans-serif;
          max-width: 500px;
          margin: 50px auto;
          padding: 20px;
          text-align: center;
        }
        .success {
          background: #d4edda;
          color: #155724;
          padding: 30px;
          border-radius: 8px;
          border: 1px solid #c3e6cb;
        }
      </style>
    </head>
    <body>
      <div class="success">
        <h1>✓ Welcome, ${formData.username}!</h1>
        <p>Your account has been created successfully.</p>
        <p>A confirmation email has been sent to ${formData.email}</p>
      </div>
    </body>
    </html>
  `;

  return {
    status: 200,
    headers: { "Content-Type": "text/html" },
    body: html,
  };
}

function validateSignupForm(data) {
  const errors = [];

  // Username validation
  if (!data.username || data.username.length < 3) {
    errors.push("Username must be at least 3 characters");
  } else if (data.username.length > 20) {
    errors.push("Username must be no more than 20 characters");
  } else if (!/^[a-zA-Z0-9]+$/.test(data.username)) {
    errors.push("Username can only contain letters and numbers");
  }

  // Email validation
  if (!data.email || !data.email.includes("@")) {
    errors.push("Valid email is required");
  }

  // Password validation
  if (!data.password || data.password.length < 8) {
    errors.push("Password must be at least 8 characters");
  }

  if (data.password !== data.confirmPassword) {
    errors.push("Passwords do not match");
  }

  // Age validation
  const age = parseInt(data.age);
  if (isNaN(age) || age < 13) {
    errors.push("You must be at least 13 years old");
  } else if (age > 120) {
    errors.push("Please enter a valid age");
  }

  // Country validation
  if (!data.country) {
    errors.push("Please select a country");
  }

  return errors;
}

function parseFormData(body) {
  const data = {};
  const pairs = body.split("&");

  for (const pair of pairs) {
    const [key, value] = pair.split("=");
    data[decodeURIComponent(key)] = decodeURIComponent(
      value.replace(/\+/g, " "),
    );
  }

  return data;
}

function parseQueryParams(url) {
  const params = {};
  const queryString = url.split("?")[1];
  if (!queryString) return params;

  const pairs = queryString.split("&");
  for (const pair of pairs) {
    const [key, value] = pair.split("=");
    params[decodeURIComponent(key)] = decodeURIComponent(value || "");
  }
  return params;
}

init();
```

## AJAX Form Submission

Modern async form handling:

```javascript
function init() {
  routeRegistry.registerRoute("GET", "/feedback", showFeedbackForm);
  routeRegistry.registerRoute("POST", "/api/feedback", handleFeedbackAPI);
}

function showFeedbackForm(request) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>Feedback Form</title>
      <style>
        body {
          font-family: Arial, sans-serif;
          max-width: 600px;
          margin: 50px auto;
          padding: 20px;
        }
        .form-group {
          margin: 20px 0;
        }
        label {
          display: block;
          margin-bottom: 5px;
          font-weight: bold;
        }
        input, textarea, select {
          width: 100%;
          padding: 10px;
          border: 1px solid #ddd;
          border-radius: 4px;
          box-sizing: border-box;
        }
        button {
          background: #28a745;
          color: white;
          padding: 12px 30px;
          border: none;
          border-radius: 4px;
          cursor: pointer;
        }
        button:disabled {
          background: #ccc;
          cursor: not-allowed;
        }
        .message {
          padding: 15px;
          border-radius: 4px;
          margin: 20px 0;
          display: none;
        }
        .message.success {
          background: #d4edda;
          color: #155724;
          border: 1px solid #c3e6cb;
        }
        .message.error {
          background: #f8d7da;
          color: #721c24;
          border: 1px solid #f5c6cb;
        }
        .rating {
          display: flex;
          gap: 10px;
          font-size: 2em;
        }
        .star {
          cursor: pointer;
          color: #ddd;
        }
        .star.selected {
          color: #ffc107;
        }
      </style>
    </head>
    <body>
      <h1>We'd Love Your Feedback!</h1>
      
      <div id="successMessage" class="message success">
        <strong>✓ Thank you!</strong> Your feedback has been submitted.
      </div>
      
      <div id="errorMessage" class="message error">
        <strong>✗ Error:</strong> <span id="errorText"></span>
      </div>
      
      <form id="feedbackForm">
        <div class="form-group">
          <label>Your Name</label>
          <input type="text" id="name" required>
        </div>
        
        <div class="form-group">
          <label>Email</label>
          <input type="email" id="email" required>
        </div>
        
        <div class="form-group">
          <label>Rating</label>
          <div class="rating" id="rating">
            <span class="star" data-value="1">★</span>
            <span class="star" data-value="2">★</span>
            <span class="star" data-value="3">★</span>
            <span class="star" data-value="4">★</span>
            <span class="star" data-value="5">★</span>
          </div>
          <input type="hidden" id="ratingValue" value="0">
        </div>
        
        <div class="form-group">
          <label>Category</label>
          <select id="category" required>
            <option value="">Select...</option>
            <option value="general">General Feedback</option>
            <option value="bug">Bug Report</option>
            <option value="feature">Feature Request</option>
            <option value="support">Support</option>
          </select>
        </div>
        
        <div class="form-group">
          <label>Comments</label>
          <textarea id="comments" rows="5" required></textarea>
        </div>
        
        <button type="submit" id="submitBtn">Submit Feedback</button>
      </form>
      
      <script>
        // Rating stars
        const stars = document.querySelectorAll('.star');
        const ratingValue = document.getElementById('ratingValue');
        
        stars.forEach(star => {
          star.addEventListener('click', function() {
            const value = parseInt(this.dataset.value);
            ratingValue.value = value;
            
            stars.forEach((s, index) => {
              if (index < value) {
                s.classList.add('selected');
              } else {
                s.classList.remove('selected');
              }
            });
          });
        });
        
        // Form submission
        const form = document.getElementById('feedbackForm');
        const submitBtn = document.getElementById('submitBtn');
        const successMessage = document.getElementById('successMessage');
        const errorMessage = document.getElementById('errorMessage');
        const errorText = document.getElementById('errorText');
        
        form.addEventListener('submit', async function(e) {
          e.preventDefault();
          
          // Hide messages
          successMessage.style.display = 'none';
          errorMessage.style.display = 'none';
          
          // Disable button
          submitBtn.disabled = true;
          submitBtn.textContent = 'Submitting...';
          
          // Collect form data
          const data = {
            name: document.getElementById('name').value,
            email: document.getElementById('email').value,
            rating: parseInt(ratingValue.value),
            category: document.getElementById('category').value,
            comments: document.getElementById('comments').value
          };
          
          // Validate rating
          if (data.rating === 0) {
            errorText.textContent = 'Please select a rating';
            errorMessage.style.display = 'block';
            submitBtn.disabled = false;
            submitBtn.textContent = 'Submit Feedback';
            return;
          }
          
          try {
            // Submit via AJAX
            const response = await fetch('/api/feedback', {
              method: 'POST',
              headers: {
                'Content-Type': 'application/json'
              },
              body: JSON.stringify(data)
            });
            
            const result = await response.json();
            
            if (response.ok) {
              // Success
              successMessage.style.display = 'block';
              form.reset();
              ratingValue.value = 0;
              stars.forEach(s => s.classList.remove('selected'));
              
              // Scroll to message
              successMessage.scrollIntoView({ behavior: 'smooth' });
            } else {
              // Error from server
              errorText.textContent = result.error || 'Something went wrong';
              errorMessage.style.display = 'block';
            }
          } catch (error) {
            errorText.textContent = 'Network error. Please try again.';
            errorMessage.style.display = 'block';
          } finally {
            submitBtn.disabled = false;
            submitBtn.textContent = 'Submit Feedback';
          }
        });
      </script>
    </body>
    </html>
  `;

  return {
    status: 200,
    headers: { "Content-Type": "text/html" },
    body: html,
  };
}

function handleFeedbackAPI(request) {
  let data;
  try {
    data = JSON.parse(request.body);
  } catch (e) {
    return {
      status: 400,
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ error: "Invalid JSON" }),
    };
  }

  // Validate
  const errors = [];
  if (!data.name || data.name.trim() === "") {
    errors.push("Name is required");
  }
  if (!data.email || !data.email.includes("@")) {
    errors.push("Valid email is required");
  }
  if (!data.rating || data.rating < 1 || data.rating > 5) {
    errors.push("Rating must be between 1 and 5");
  }
  if (!data.category) {
    errors.push("Category is required");
  }
  if (!data.comments || data.comments.trim() === "") {
    errors.push("Comments are required");
  }

  if (errors.length > 0) {
    return {
      status: 400,
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        error: "Validation failed",
        errors: errors,
      }),
    };
  }

  // Log feedback
  console.log("Feedback received", {
    name: data.name,
    email: data.email,
    rating: data.rating,
    category: data.category,
  });

  return {
    status: 200,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      success: true,
      message: "Thank you for your feedback!",
    }),
  };
}

init();
```

## File Upload Handling

Handle file uploads (for when assets API is used):

```javascript
function init() {
  routeRegistry.registerRoute("GET", "/upload", showUploadForm);
  routeRegistry.registerRoute("POST", "/upload", handleUpload);
}

function showUploadForm(request) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>File Upload</title>
      <style>
        body {
          font-family: Arial, sans-serif;
          max-width: 600px;
          margin: 50px auto;
          padding: 20px;
        }
        .upload-area {
          border: 2px dashed #007bff;
          border-radius: 8px;
          padding: 40px;
          text-align: center;
          background: #f8f9fa;
          cursor: pointer;
        }
        .upload-area:hover {
          background: #e9ecef;
        }
        .file-info {
          margin: 20px 0;
          padding: 15px;
          background: #e7f3ff;
          border-radius: 4px;
          display: none;
        }
        button {
          background: #007bff;
          color: white;
          padding: 12px 30px;
          border: none;
          border-radius: 4px;
          cursor: pointer;
          margin-top: 20px;
        }
        .progress {
          width: 100%;
          height: 30px;
          background: #e9ecef;
          border-radius: 4px;
          overflow: hidden;
          margin: 20px 0;
          display: none;
        }
        .progress-bar {
          height: 100%;
          background: #28a745;
          width: 0%;
          transition: width 0.3s;
          text-align: center;
          color: white;
          line-height: 30px;
        }
      </style>
    </head>
    <body>
      <h1>Upload File</h1>
      
      <form id="uploadForm" enctype="multipart/form-data">
        <div class="upload-area" id="uploadArea">
          <p>📁 Click to select file or drag and drop</p>
          <input type="file" id="fileInput" name="file" style="display: none;">
        </div>
        
        <div class="file-info" id="fileInfo">
          <strong>Selected:</strong> <span id="fileName"></span><br>
          <strong>Size:</strong> <span id="fileSize"></span><br>
          <strong>Type:</strong> <span id="fileType"></span>
        </div>
        
        <div class="progress" id="progress">
          <div class="progress-bar" id="progressBar">0%</div>
        </div>
        
        <button type="submit" id="uploadBtn" style="display: none;">Upload File</button>
      </form>
      
      <div id="result"></div>
      
      <script>
        const uploadArea = document.getElementById('uploadArea');
        const fileInput = document.getElementById('fileInput');
        const fileInfo = document.getElementById('fileInfo');
        const fileName = document.getElementById('fileName');
        const fileSize = document.getElementById('fileSize');
        const fileType = document.getElementById('fileType');
        const uploadBtn = document.getElementById('uploadBtn');
        const progress = document.getElementById('progress');
        const progressBar = document.getElementById('progressBar');
        const result = document.getElementById('result');
        
        uploadArea.addEventListener('click', () => fileInput.click());
        
        fileInput.addEventListener('change', function(e) {
          const file = e.target.files[0];
          if (file) {
            displayFileInfo(file);
          }
        });
        
        uploadArea.addEventListener('dragover', function(e) {
          e.preventDefault();
          uploadArea.style.borderColor = '#0056b3';
        });
        
        uploadArea.addEventListener('dragleave', function() {
          uploadArea.style.borderColor = '#007bff';
        });
        
        uploadArea.addEventListener('drop', function(e) {
          e.preventDefault();
          uploadArea.style.borderColor = '#007bff';
          
          const file = e.dataTransfer.files[0];
          if (file) {
            fileInput.files = e.dataTransfer.files;
            displayFileInfo(file);
          }
        });
        
        function displayFileInfo(file) {
          fileName.textContent = file.name;
          fileSize.textContent = formatFileSize(file.size);
          fileType.textContent = file.type || 'unknown';
          
          fileInfo.style.display = 'block';
          uploadBtn.style.display = 'inline-block';
        }
        
        function formatFileSize(bytes) {
          if (bytes === 0) return '0 Bytes';
          const k = 1024;
          const sizes = ['Bytes', 'KB', 'MB', 'GB'];
          const i = Math.floor(Math.log(bytes) / Math.log(k));
          return Math.round(bytes / Math.pow(k, i) * 100) / 100 + ' ' + sizes[i];
        }
        
        document.getElementById('uploadForm').addEventListener('submit', async function(e) {
          e.preventDefault();
          
          const file = fileInput.files[0];
          if (!file) return;
          
          const formData = new FormData();
          formData.append('file', file);
          
          uploadBtn.disabled = true;
          progress.style.display = 'block';
          
          try {
            // Simulate upload with fetch
            // Note: In real implementation, use upsertAsset() API
            const response = await fetch('/upload', {
              method: 'POST',
              body: formData
            });
            
            if (response.ok) {
              progressBar.style.width = '100%';
              progressBar.textContent = '100%';
              
              result.innerHTML = '<div style="background: #d4edda; padding: 15px; border-radius: 4px; color: #155724; margin-top: 20px;"><strong>✓ Upload successful!</strong></div>';
              
              // Reset form
              setTimeout(() => {
                fileInput.value = '';
                fileInfo.style.display = 'none';
                uploadBtn.style.display = 'none';
                progress.style.display = 'none';
                progressBar.style.width = '0%';
                uploadBtn.disabled = false;
                result.innerHTML = '';
              }, 3000);
            } else {
              result.innerHTML = '<div style="background: #f8d7da; padding: 15px; border-radius: 4px; color: #721c24; margin-top: 20px;"><strong>✗ Upload failed!</strong></div>';
            }
          } catch (error) {
            result.innerHTML = '<div style="background: #f8d7da; padding: 15px; border-radius: 4px; color: #721c24; margin-top: 20px;"><strong>✗ Error:</strong> ' + error.message + '</div>';
          }
        });
      </script>
    </body>
    </html>
  `;

  return {
    status: 200,
    headers: { "Content-Type": "text/html" },
    body: html,
  };
}

function handleUpload(request) {
  console.log("File upload received", {
    contentType: request.headers["content-type"],
    bodyLength: request.body ? request.body.length : 0,
  });

  // In real implementation, you would:
  // 1. Parse multipart form data
  // 2. Extract file content
  // 3. Use upsertAsset() to save the file

  return {
    status: 200,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      success: true,
      message: "File uploaded successfully",
    }),
  };
}

init();
```

## Multi-Step Form (Wizard)

Create a multi-step registration process:

```javascript
function init() {
  routeRegistry.registerRoute("GET", "/wizard", showWizard);
  routeRegistry.registerRoute("POST", "/api/wizard/step", handleWizardStep);
}

function showWizard(request) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>Registration Wizard</title>
      <style>
        body {
          font-family: Arial, sans-serif;
          max-width: 700px;
          margin: 50px auto;
          padding: 20px;
        }
        .steps {
          display: flex;
          justify-content: space-between;
          margin-bottom: 40px;
        }
        .step {
          flex: 1;
          text-align: center;
          padding: 10px;
          background: #e9ecef;
          margin: 0 5px;
          border-radius: 4px;
        }
        .step.active {
          background: #007bff;
          color: white;
        }
        .step.completed {
          background: #28a745;
          color: white;
        }
        .form-step {
          display: none;
        }
        .form-step.active {
          display: block;
        }
        .form-group {
          margin: 20px 0;
        }
        label {
          display: block;
          margin-bottom: 5px;
          font-weight: bold;
        }
        input, select, textarea {
          width: 100%;
          padding: 10px;
          border: 1px solid #ddd;
          border-radius: 4px;
          box-sizing: border-box;
        }
        .buttons {
          margin-top: 30px;
          display: flex;
          justify-content: space-between;
        }
        button {
          padding: 12px 30px;
          border: none;
          border-radius: 4px;
          cursor: pointer;
        }
        .btn-prev {
          background: #6c757d;
          color: white;
        }
        .btn-next, .btn-submit {
          background: #007bff;
          color: white;
        }
        .summary {
          background: #f8f9fa;
          padding: 20px;
          border-radius: 8px;
          margin: 20px 0;
        }
        .summary h3 {
          margin-top: 0;
        }
        .summary-item {
          margin: 10px 0;
          padding: 10px;
          background: white;
          border-radius: 4px;
        }
      </style>
    </head>
    <body>
      <h1>Complete Registration</h1>
      
      <div class="steps">
        <div class="step active" id="step-indicator-1">
          <strong>1</strong><br>Personal Info
        </div>
        <div class="step" id="step-indicator-2">
          <strong>2</strong><br>Account Details
        </div>
        <div class="step" id="step-indicator-3">
          <strong>3</strong><br>Preferences
        </div>
        <div class="step" id="step-indicator-4">
          <strong>4</strong><br>Review
        </div>
      </div>
      
      <form id="wizardForm">
        <!-- Step 1: Personal Info -->
        <div class="form-step active" id="step-1">
          <h2>Personal Information</h2>
          <div class="form-group">
            <label>First Name *</label>
            <input type="text" id="firstName" required>
          </div>
          <div class="form-group">
            <label>Last Name *</label>
            <input type="text" id="lastName" required>
          </div>
          <div class="form-group">
            <label>Date of Birth *</label>
            <input type="date" id="dob" required>
          </div>
          <div class="form-group">
            <label>Phone</label>
            <input type="tel" id="phone">
          </div>
        </div>
        
        <!-- Step 2: Account Details -->
        <div class="form-step" id="step-2">
          <h2>Account Details</h2>
          <div class="form-group">
            <label>Email *</label>
            <input type="email" id="email" required>
          </div>
          <div class="form-group">
            <label>Username *</label>
            <input type="text" id="username" required>
          </div>
          <div class="form-group">
            <label>Password *</label>
            <input type="password" id="password" required>
          </div>
          <div class="form-group">
            <label>Confirm Password *</label>
            <input type="password" id="confirmPassword" required>
          </div>
        </div>
        
        <!-- Step 3: Preferences -->
        <div class="form-step" id="step-3">
          <h2>Preferences</h2>
          <div class="form-group">
            <label>Preferred Language</label>
            <select id="language">
              <option value="en">English</option>
              <option value="es">Spanish</option>
              <option value="fr">French</option>
              <option value="de">German</option>
            </select>
          </div>
          <div class="form-group">
            <label>Interests</label>
            <textarea id="interests" rows="3"></textarea>
          </div>
          <div class="form-group">
            <label>
              <input type="checkbox" id="newsletter"> Subscribe to newsletter
            </label>
          </div>
          <div class="form-group">
            <label>
              <input type="checkbox" id="terms" required> I agree to the terms and conditions *
            </label>
          </div>
        </div>
        
        <!-- Step 4: Review -->
        <div class="form-step" id="step-4">
          <h2>Review Your Information</h2>
          <div class="summary">
            <h3>Personal Information</h3>
            <div class="summary-item">
              <strong>Name:</strong> <span id="summary-name"></span><br>
              <strong>Date of Birth:</strong> <span id="summary-dob"></span><br>
              <strong>Phone:</strong> <span id="summary-phone"></span>
            </div>
            
            <h3>Account Details</h3>
            <div class="summary-item">
              <strong>Email:</strong> <span id="summary-email"></span><br>
              <strong>Username:</strong> <span id="summary-username"></span>
            </div>
            
            <h3>Preferences</h3>
            <div class="summary-item">
              <strong>Language:</strong> <span id="summary-language"></span><br>
              <strong>Interests:</strong> <span id="summary-interests"></span><br>
              <strong>Newsletter:</strong> <span id="summary-newsletter"></span>
            </div>
          </div>
        </div>
        
        <div class="buttons">
          <button type="button" class="btn-prev" id="prevBtn" style="display: none;">← Previous</button>
          <div></div>
          <button type="button" class="btn-next" id="nextBtn">Next →</button>
          <button type="submit" class="btn-submit" id="submitBtn" style="display: none;">Submit Registration</button>
        </div>
      </form>
      
      <script>
        let currentStep = 1;
        const totalSteps = 4;
        
        const formData = {
          personal: {},
          account: {},
          preferences: {}
        };
        
        document.getElementById('nextBtn').addEventListener('click', function() {
          if (validateStep(currentStep)) {
            saveStepData(currentStep);
            currentStep++;
            showStep(currentStep);
            
            if (currentStep === totalSteps) {
              updateSummary();
            }
          }
        });
        
        document.getElementById('prevBtn').addEventListener('click', function() {
          currentStep--;
          showStep(currentStep);
        });
        
        document.getElementById('wizardForm').addEventListener('submit', async function(e) {
          e.preventDefault();
          
          const submitBtn = document.getElementById('submitBtn');
          submitBtn.disabled = true;
          submitBtn.textContent = 'Submitting...';
          
          try {
            const response = await fetch('/api/wizard/step', {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify(formData)
            });
            
            if (response.ok) {
              alert('Registration complete! Welcome aboard!');
              window.location.href = '/';
            } else {
              alert('Registration failed. Please try again.');
            }
          } catch (error) {
            alert('Error: ' + error.message);
          } finally {
            submitBtn.disabled = false;
            submitBtn.textContent = 'Submit Registration';
          }
        });
        
        function showStep(step) {
          // Hide all steps
          document.querySelectorAll('.form-step').forEach(s => s.classList.remove('active'));
          document.querySelectorAll('.step').forEach(s => {
            s.classList.remove('active');
            if (parseInt(s.id.split('-')[2]) < step) {
              s.classList.add('completed');
            } else {
              s.classList.remove('completed');
            }
          });
          
          // Show current step
          document.getElementById(\`step-\${step}\`).classList.add('active');
          document.getElementById(\`step-indicator-\${step}\`).classList.add('active');
          
          // Update buttons
          document.getElementById('prevBtn').style.display = step > 1 ? 'block' : 'none';
          document.getElementById('nextBtn').style.display = step < totalSteps ? 'block' : 'none';
          document.getElementById('submitBtn').style.display = step === totalSteps ? 'block' : 'none';
        }
        
        function validateStep(step) {
          const stepElement = document.getElementById(\`step-\${step}\`);
          const inputs = stepElement.querySelectorAll('input[required], select[required], textarea[required]');
          
          for (const input of inputs) {
            if (!input.value) {
              alert('Please fill in all required fields');
              input.focus();
              return false;
            }
          }
          
          if (step === 2) {
            const password = document.getElementById('password').value;
            const confirmPassword = document.getElementById('confirmPassword').value;
            if (password !== confirmPassword) {
              alert('Passwords do not match');
              return false;
            }
          }
          
          return true;
        }
        
        function saveStepData(step) {
          if (step === 1) {
            formData.personal = {
              firstName: document.getElementById('firstName').value,
              lastName: document.getElementById('lastName').value,
              dob: document.getElementById('dob').value,
              phone: document.getElementById('phone').value
            };
          } else if (step === 2) {
            formData.account = {
              email: document.getElementById('email').value,
              username: document.getElementById('username').value,
              password: document.getElementById('password').value
            };
          } else if (step === 3) {
            formData.preferences = {
              language: document.getElementById('language').value,
              interests: document.getElementById('interests').value,
              newsletter: document.getElementById('newsletter').checked,
              terms: document.getElementById('terms').checked
            };
          }
        }
        
        function updateSummary() {
          document.getElementById('summary-name').textContent = 
            \`\${formData.personal.firstName} \${formData.personal.lastName}\`;
          document.getElementById('summary-dob').textContent = formData.personal.dob;
          document.getElementById('summary-phone').textContent = formData.personal.phone || 'Not provided';
          
          document.getElementById('summary-email').textContent = formData.account.email;
          document.getElementById('summary-username').textContent = formData.account.username;
          
          document.getElementById('summary-language').textContent = formData.preferences.language;
          document.getElementById('summary-interests').textContent = formData.preferences.interests || 'None';
          document.getElementById('summary-newsletter').textContent = formData.preferences.newsletter ? 'Yes' : 'No';
        }
      </script>
    </body>
    </html>
  `;

  return {
    status: 200,
    headers: { "Content-Type": "text/html" },
    body: html,
  };
}

function handleWizardStep(request) {
  const data = JSON.parse(request.body);

  console.log("Wizard registration completed", {
    username: data.account.username,
    email: data.account.email,
  });

  return {
    status: 200,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      success: true,
      message: "Registration completed successfully",
    }),
  };
}

init();
```

## Best Practices

### 1. Always Validate on Server

Never trust client-side validation alone:

```javascript
// Client-side validation is UX
// Server-side validation is security
function validateInput(data) {
  const errors = [];

  // Always validate
  if (!data.email || !data.email.includes("@")) {
    errors.push("Invalid email");
  }

  return errors;
}
```

### 2. Provide Clear Feedback

```javascript
// Good: Specific error messages
"Email must contain an @ symbol";

// Bad: Generic errors
"Invalid input";
```

### 3. Preserve User Input

When validation fails, preserve what the user entered:

```javascript
// Return form with values filled in
value = "${values.username || ''}";
```

### 4. Use Appropriate Input Types

```html
<input type="email" name="email" />
<input type="tel" name="phone" />
<input type="date" name="dob" />
<input type="number" name="age" />
```

### 5. Handle Errors Gracefully

```javascript
try {
  const data = JSON.parse(request.body);
} catch (e) {
  return errorResponse("Invalid JSON");
}
```

## Next Steps

- **[Basic API Examples](basic-api.md)** - RESTful API patterns
- **[Real-Time Features](real-time-features.md)** - Live updates
- **[AI Integration](ai-integration.md)** - AI-powered forms
- **[Asset Management](../guides/assets.md)** - Handle file uploads
- **[Script Development](../guides/scripts.md)** - Advanced techniques

## Quick Reference

```javascript
// Parse form data
const formData = parseFormData(request.body);

// Parse JSON body
const data = JSON.parse(request.body);

// Validate
const errors = validateForm(data);

// Return with errors
if (errors.length > 0) {
  return errorResponse(errors);
}

// Success response
return successResponse(data);
```

Build powerful forms with aiwebengine! 📝
