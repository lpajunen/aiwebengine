# AI Integration Examples

Learn how to integrate AI capabilities into your aiwebengine applications using the built-in AI assistant and external AI APIs.

## Using Built-in AI Assistant

The aiwebengine editor includes an AI assistant. You can also integrate AI-powered features into your scripts.

### AI-Powered Content Generator

Create a content generation tool:

```javascript
function init() {
  routeRegistry.registerRoute("GET", "/ai-writer", showAIWriter);
  routeRegistry.registerRoute("POST", "/api/generate", generateContent);
}

function showAIWriter(request) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>AI Content Generator</title>
      <style>
        body {
          font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
          max-width: 1000px;
          margin: 50px auto;
          padding: 20px;
          background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
          min-height: 100vh;
        }
        .container {
          background: white;
          border-radius: 12px;
          padding: 40px;
          box-shadow: 0 10px 40px rgba(0,0,0,0.2);
        }
        h1 {
          color: #667eea;
          margin-bottom: 10px;
        }
        .subtitle {
          color: #666;
          margin-bottom: 30px;
        }
        .form-group {
          margin: 25px 0;
        }
        label {
          display: block;
          font-weight: bold;
          margin-bottom: 8px;
          color: #333;
        }
        input, textarea, select {
          width: 100%;
          padding: 12px;
          border: 2px solid #e0e0e0;
          border-radius: 6px;
          font-size: 14px;
          box-sizing: border-box;
          transition: border-color 0.3s;
        }
        input:focus, textarea:focus, select:focus {
          outline: none;
          border-color: #667eea;
        }
        textarea {
          resize: vertical;
          min-height: 100px;
        }
        button {
          background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
          color: white;
          border: none;
          padding: 14px 32px;
          border-radius: 6px;
          font-size: 16px;
          font-weight: bold;
          cursor: pointer;
          transition: transform 0.2s;
        }
        button:hover {
          transform: translateY(-2px);
        }
        button:disabled {
          opacity: 0.6;
          cursor: not-allowed;
          transform: none;
        }
        .result {
          margin-top: 30px;
          padding: 20px;
          background: #f8f9fa;
          border-radius: 6px;
          border-left: 4px solid #667eea;
          display: none;
        }
        .result.show {
          display: block;
          animation: slideIn 0.3s ease-out;
        }
        @keyframes slideIn {
          from {
            opacity: 0;
            transform: translateY(20px);
          }
          to {
            opacity: 1;
            transform: translateY(0);
          }
        }
        .loading {
          display: none;
          text-align: center;
          margin: 20px 0;
        }
        .loading.show {
          display: block;
        }
        .spinner {
          border: 4px solid #f3f3f3;
          border-top: 4px solid #667eea;
          border-radius: 50%;
          width: 40px;
          height: 40px;
          animation: spin 1s linear infinite;
          margin: 0 auto;
        }
        @keyframes spin {
          0% { transform: rotate(0deg); }
          100% { transform: rotate(360deg); }
        }
        .examples {
          background: #e3f2fd;
          padding: 15px;
          border-radius: 6px;
          margin: 20px 0;
        }
        .examples h3 {
          margin-top: 0;
          color: #1976d2;
        }
        .example-item {
          cursor: pointer;
          padding: 8px;
          margin: 5px 0;
          background: white;
          border-radius: 4px;
          transition: background 0.2s;
        }
        .example-item:hover {
          background: #bbdefb;
        }
      </style>
    </head>
    <body>
      <div class="container">
        <h1>✨ AI Content Generator</h1>
        <p class="subtitle">Generate high-quality content using AI</p>
        
        <div class="examples">
          <h3>Quick Examples:</h3>
          <div class="example-item" onclick="fillExample('blog')">
            📝 Blog Post about Technology
          </div>
          <div class="example-item" onclick="fillExample('product')">
            🛍️ Product Description
          </div>
          <div class="example-item" onclick="fillExample('email')">
            ✉️ Professional Email
          </div>
        </div>
        
        <form id="generateForm">
          <div class="form-group">
            <label>Content Type</label>
            <select id="contentType">
              <option value="blog">Blog Post</option>
              <option value="product">Product Description</option>
              <option value="email">Email</option>
              <option value="social">Social Media Post</option>
              <option value="ad">Advertisement</option>
              <option value="essay">Essay</option>
            </select>
          </div>
          
          <div class="form-group">
            <label>Topic/Subject</label>
            <input type="text" id="topic" placeholder="e.g., The future of artificial intelligence" required>
          </div>
          
          <div class="form-group">
            <label>Tone</label>
            <select id="tone">
              <option value="professional">Professional</option>
              <option value="casual">Casual</option>
              <option value="formal">Formal</option>
              <option value="friendly">Friendly</option>
              <option value="persuasive">Persuasive</option>
              <option value="informative">Informative</option>
            </select>
          </div>
          
          <div class="form-group">
            <label>Target Length</label>
            <select id="length">
              <option value="short">Short (100-200 words)</option>
              <option value="medium">Medium (200-400 words)</option>
              <option value="long">Long (400+ words)</option>
            </select>
          </div>
          
          <div class="form-group">
            <label>Additional Instructions (Optional)</label>
            <textarea id="instructions" placeholder="e.g., Include statistics, mention recent trends"></textarea>
          </div>
          
          <button type="submit">Generate Content</button>
        </form>
        
        <div class="loading" id="loading">
          <div class="spinner"></div>
          <p>Generating content...</p>
        </div>
        
        <div class="result" id="result">
          <h3>Generated Content:</h3>
          <div id="generatedContent"></div>
          <button type="button" onclick="copyContent()">📋 Copy to Clipboard</button>
        </div>
      </div>
      
      <script>
        const form = document.getElementById('generateForm');
        const loading = document.getElementById('loading');
        const result = document.getElementById('result');
        const generatedContent = document.getElementById('generatedContent');
        
        form.addEventListener('submit', async function(e) {
          e.preventDefault();
          
          const data = {
            contentType: document.getElementById('contentType').value,
            topic: document.getElementById('topic').value,
            tone: document.getElementById('tone').value,
            length: document.getElementById('length').value,
            instructions: document.getElementById('instructions').value
          };
          
          loading.classList.add('show');
          result.classList.remove('show');
          
          try {
            const response = await fetch('/api/generate', {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify(data)
            });
            
            const resultData = await response.json();
            
            if (response.ok) {
              generatedContent.innerHTML = resultData.content.replace(/\\n/g, '<br>');
              result.classList.add('show');
            } else {
              alert('Error: ' + (resultData.error || 'Failed to generate content'));
            }
          } catch (error) {
            alert('Error: ' + error.message);
          } finally {
            loading.classList.remove('show');
          }
        });
        
        function fillExample(type) {
          const examples = {
            blog: {
              contentType: 'blog',
              topic: 'The Impact of AI on Modern Business',
              tone: 'professional',
              length: 'medium',
              instructions: 'Include real-world examples and future predictions'
            },
            product: {
              contentType: 'product',
              topic: 'Smart Wireless Headphones',
              tone: 'persuasive',
              length: 'short',
              instructions: 'Highlight features and benefits'
            },
            email: {
              contentType: 'email',
              topic: 'Project Status Update',
              tone: 'professional',
              length: 'short',
              instructions: 'Keep it concise and action-oriented'
            }
          };
          
          const example = examples[type];
          document.getElementById('contentType').value = example.contentType;
          document.getElementById('topic').value = example.topic;
          document.getElementById('tone').value = example.tone;
          document.getElementById('length').value = example.length;
          document.getElementById('instructions').value = example.instructions;
        }
        
        function copyContent() {
          const text = generatedContent.innerText;
          navigator.clipboard.writeText(text).then(() => {
            alert('Content copied to clipboard!');
          });
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

function generateContent(request) {
  const data = JSON.parse(request.body || "{}");

  console.log("AI content generation requested", {
    contentType: data.contentType,
    topic: data.topic,
  });

  // In a real implementation, you would call an AI API here
  // For demonstration, we'll generate a mock response

  const mockContent = generateMockContent(data);

  return {
    status: 200,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      success: true,
      content: mockContent,
    }),
  };
}

function generateMockContent(data) {
  // Mock AI-generated content
  const templates = {
    blog: `# ${data.topic}

In today's rapidly evolving digital landscape, ${data.topic.toLowerCase()} has become increasingly important. This trend represents a significant shift in how we approach modern challenges.

## Key Points

The impact of this development cannot be overstated. Industry experts predict that this will fundamentally transform the way we work and interact with technology.

## Looking Forward

As we move into the future, understanding and adapting to these changes will be crucial for success. Organizations that embrace this transformation early will have a competitive advantage.

## Conclusion

${data.topic} is not just a passing trend—it's a fundamental shift that will shape our future. By staying informed and adapting quickly, we can harness its potential for positive change.`,

    product: `Introducing ${data.topic}—the perfect solution for modern needs.

Premium Features:
• Advanced technology for superior performance
• Sleek, modern design that fits any lifestyle
• Easy to use with intuitive controls
• Durable construction for long-lasting value

Why Choose This Product?
This isn't just another product—it's a game-changer. Designed with you in mind, it combines cutting-edge features with exceptional quality.

Order Today!
Don't miss out on this opportunity to upgrade your experience. Limited stock available.`,

    email: `Subject: ${data.topic}

Hi Team,

I wanted to provide you with an update on ${data.topic.toLowerCase()}.

Current Status:
We've made significant progress and are on track to meet our objectives. The team has been working diligently to ensure quality and timely delivery.

Next Steps:
• Review current milestone achievements
• Address any outstanding items
• Plan for upcoming phase

Please let me know if you have any questions or concerns.

Best regards`,
  };

  return (
    templates[data.contentType] ||
    `Generated content about ${data.topic} with ${data.tone} tone.`
  );
}

init();
```

## AI-Powered Chatbot

Create an interactive AI chatbot:

```javascript
function init() {
  routeRegistry.registerRoute("GET", "/chatbot", showChatbot);
  routeRegistry.registerRoute("POST", "/api/chat", handleChat);
}

function showChatbot(request) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>AI Chatbot</title>
      <style>
        * {
          margin: 0;
          padding: 0;
          box-sizing: border-box;
        }
        body {
          font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
          background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
          height: 100vh;
          display: flex;
          justify-content: center;
          align-items: center;
        }
        .chat-container {
          width: 90%;
          max-width: 500px;
          height: 80vh;
          background: white;
          border-radius: 20px;
          box-shadow: 0 10px 40px rgba(0,0,0,0.3);
          display: flex;
          flex-direction: column;
          overflow: hidden;
        }
        .chat-header {
          background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
          color: white;
          padding: 20px;
          text-align: center;
        }
        .chat-header h2 {
          margin-bottom: 5px;
        }
        .status {
          font-size: 0.85em;
          opacity: 0.9;
        }
        .chat-messages {
          flex: 1;
          overflow-y: auto;
          padding: 20px;
          background: #f5f7fa;
        }
        .message {
          margin: 15px 0;
          display: flex;
          animation: fadeIn 0.3s;
        }
        @keyframes fadeIn {
          from {
            opacity: 0;
            transform: translateY(10px);
          }
          to {
            opacity: 1;
            transform: translateY(0);
          }
        }
        .message.user {
          justify-content: flex-end;
        }
        .message-content {
          max-width: 70%;
          padding: 12px 16px;
          border-radius: 18px;
          word-wrap: break-word;
        }
        .message.bot .message-content {
          background: white;
          color: #333;
          border: 1px solid #e0e0e0;
        }
        .message.user .message-content {
          background: #667eea;
          color: white;
        }
        .message-time {
          font-size: 0.75em;
          opacity: 0.7;
          margin-top: 5px;
        }
        .typing-indicator {
          display: none;
          padding: 12px 16px;
          background: white;
          border-radius: 18px;
          width: fit-content;
          border: 1px solid #e0e0e0;
        }
        .typing-indicator.show {
          display: block;
        }
        .typing-indicator span {
          height: 8px;
          width: 8px;
          background: #667eea;
          display: inline-block;
          border-radius: 50%;
          margin: 0 2px;
          animation: bounce 1.4s infinite ease-in-out;
        }
        .typing-indicator span:nth-child(1) {
          animation-delay: -0.32s;
        }
        .typing-indicator span:nth-child(2) {
          animation-delay: -0.16s;
        }
        @keyframes bounce {
          0%, 80%, 100% {
            transform: scale(0);
          }
          40% {
            transform: scale(1);
          }
        }
        .chat-input {
          display: flex;
          padding: 20px;
          background: white;
          border-top: 1px solid #e0e0e0;
        }
        .chat-input input {
          flex: 1;
          padding: 12px 16px;
          border: 2px solid #e0e0e0;
          border-radius: 25px;
          font-size: 14px;
          transition: border-color 0.3s;
        }
        .chat-input input:focus {
          outline: none;
          border-color: #667eea;
        }
        .chat-input button {
          margin-left: 10px;
          padding: 12px 24px;
          background: #667eea;
          color: white;
          border: none;
          border-radius: 25px;
          font-weight: bold;
          cursor: pointer;
          transition: background 0.3s;
        }
        .chat-input button:hover {
          background: #5568d3;
        }
        .chat-input button:disabled {
          background: #ccc;
          cursor: not-allowed;
        }
        .suggestions {
          display: flex;
          gap: 10px;
          padding: 10px 20px;
          overflow-x: auto;
          background: #f5f7fa;
        }
        .suggestion-chip {
          background: white;
          padding: 8px 16px;
          border-radius: 20px;
          cursor: pointer;
          white-space: nowrap;
          font-size: 0.85em;
          border: 1px solid #e0e0e0;
          transition: all 0.3s;
        }
        .suggestion-chip:hover {
          background: #667eea;
          color: white;
          border-color: #667eea;
        }
      </style>
    </head>
    <body>
      <div class="chat-container">
        <div class="chat-header">
          <h2>🤖 AI Assistant</h2>
          <div class="status">Online • Ready to help</div>
        </div>
        
        <div class="suggestions" id="suggestions">
          <div class="suggestion-chip" onclick="sendSuggestion('What can you help me with?')">
            What can you help me with?
          </div>
          <div class="suggestion-chip" onclick="sendSuggestion('Tell me a fun fact')">
            Tell me a fun fact
          </div>
          <div class="suggestion-chip" onclick="sendSuggestion('How does AI work?')">
            How does AI work?
          </div>
        </div>
        
        <div class="chat-messages" id="messages">
          <div class="message bot">
            <div class="message-content">
              Hello! I'm your AI assistant. How can I help you today?
              <div class="message-time">Just now</div>
            </div>
          </div>
        </div>
        
        <div style="padding: 0 20px;">
          <div class="typing-indicator" id="typingIndicator">
            <span></span>
            <span></span>
            <span></span>
          </div>
        </div>
        
        <div class="chat-input">
          <input type="text" id="userInput" placeholder="Type your message..." autocomplete="off">
          <button id="sendBtn">Send</button>
        </div>
      </div>
      
      <script>
        const messagesContainer = document.getElementById('messages');
        const userInput = document.getElementById('userInput');
        const sendBtn = document.getElementById('sendBtn');
        const typingIndicator = document.getElementById('typingIndicator');
        
        function addMessage(text, isUser) {
          const messageDiv = document.createElement('div');
          messageDiv.className = \`message \${isUser ? 'user' : 'bot'}\`;
          
          const contentDiv = document.createElement('div');
          contentDiv.className = 'message-content';
          contentDiv.innerHTML = \`
            \${text}
            <div class="message-time">\${new Date().toLocaleTimeString()}</div>
          \`;
          
          messageDiv.appendChild(contentDiv);
          messagesContainer.appendChild(messageDiv);
          messagesContainer.scrollTop = messagesContainer.scrollHeight;
        }
        
        async function sendMessage(message) {
          if (!message.trim()) return;
          
          // Add user message
          addMessage(message, true);
          userInput.value = '';
          
          // Show typing indicator
          typingIndicator.classList.add('show');
          sendBtn.disabled = true;
          
          try {
            const response = await fetch('/api/chat', {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify({ message: message })
            });
            
            const data = await response.json();
            
            // Simulate typing delay
            setTimeout(() => {
              typingIndicator.classList.remove('show');
              addMessage(data.response, false);
              sendBtn.disabled = false;
              userInput.focus();
            }, 1000);
          } catch (error) {
            typingIndicator.classList.remove('show');
            addMessage('Sorry, I encountered an error. Please try again.', false);
            sendBtn.disabled = false;
          }
        }
        
        sendBtn.addEventListener('click', () => {
          sendMessage(userInput.value);
        });
        
        userInput.addEventListener('keypress', (e) => {
          if (e.key === 'Enter') {
            sendMessage(userInput.value);
          }
        });
        
        function sendSuggestion(text) {
          userInput.value = text;
          sendMessage(text);
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

function handleChat(request) {
  const data = JSON.parse(request.body || "{}");
  const userMessage = data.message;

  console.log("Chat message received", { message: userMessage });

  // Mock AI response logic
  const response = generateChatResponse(userMessage);

  return {
    status: 200,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      success: true,
      response: response,
    }),
  };
}

function generateChatResponse(message) {
  const lowerMessage = message.toLowerCase();

  // Simple keyword-based responses
  if (lowerMessage.includes("hello") || lowerMessage.includes("hi")) {
    return "Hello! How can I assist you today?";
  }

  if (lowerMessage.includes("help")) {
    return "I can help you with various tasks like answering questions, providing information, and having conversations. What would you like to know?";
  }

  if (
    lowerMessage.includes("ai") ||
    lowerMessage.includes("artificial intelligence")
  ) {
    return "Artificial Intelligence refers to computer systems that can perform tasks that typically require human intelligence, such as learning, problem-solving, and decision-making.";
  }

  if (lowerMessage.includes("fun fact")) {
    const facts = [
      "The first computer programmer was Ada Lovelace in the 1840s!",
      "Honey never spoils. Archaeologists have found 3000-year-old honey that's still edible!",
      "Octopuses have three hearts and blue blood!",
      "The word 'robot' comes from the Czech word 'robota,' meaning forced labor.",
    ];
    return facts[Math.floor(Math.random() * facts.length)];
  }

  // Default response
  return "That's interesting! I'm here to help answer your questions and assist with various tasks. Feel free to ask me anything!";
}

init();
```

## AI Image Description

Generate descriptions for images:

```javascript
function init() {
  routeRegistry.registerRoute("GET", "/image-analyzer", showImageAnalyzer);
  routeRegistry.registerRoute("POST", "/api/analyze-image", analyzeImage);
}

function showImageAnalyzer(request) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>AI Image Analyzer</title>
      <style>
        body {
          font-family: Arial, sans-serif;
          max-width: 800px;
          margin: 50px auto;
          padding: 20px;
          background: #f5f5f5;
        }
        .container {
          background: white;
          padding: 40px;
          border-radius: 12px;
          box-shadow: 0 4px 6px rgba(0,0,0,0.1);
        }
        h1 {
          color: #333;
          margin-bottom: 10px;
        }
        .upload-area {
          border: 3px dashed #ccc;
          border-radius: 8px;
          padding: 60px 20px;
          text-align: center;
          cursor: pointer;
          transition: all 0.3s;
          margin: 30px 0;
        }
        .upload-area:hover {
          border-color: #007bff;
          background: #f8f9fa;
        }
        .upload-area.dragover {
          border-color: #28a745;
          background: #d4edda;
        }
        #imagePreview {
          max-width: 100%;
          max-height: 400px;
          margin: 20px 0;
          border-radius: 8px;
          display: none;
        }
        #imagePreview.show {
          display: block;
        }
        .results {
          margin-top: 30px;
          padding: 20px;
          background: #f8f9fa;
          border-radius: 8px;
          display: none;
        }
        .results.show {
          display: block;
        }
        .result-item {
          margin: 15px 0;
          padding: 15px;
          background: white;
          border-radius: 6px;
          border-left: 4px solid #007bff;
        }
        .result-label {
          font-weight: bold;
          color: #666;
          margin-bottom: 5px;
        }
        button {
          background: #007bff;
          color: white;
          border: none;
          padding: 12px 30px;
          border-radius: 6px;
          cursor: pointer;
          font-size: 16px;
          font-weight: bold;
        }
        button:hover {
          background: #0056b3;
        }
        button:disabled {
          background: #ccc;
          cursor: not-allowed;
        }
        .loading {
          text-align: center;
          padding: 20px;
          display: none;
        }
        .loading.show {
          display: block;
        }
        .spinner {
          border: 4px solid #f3f3f3;
          border-top: 4px solid #007bff;
          border-radius: 50%;
          width: 40px;
          height: 40px;
          animation: spin 1s linear infinite;
          margin: 0 auto 10px;
        }
        @keyframes spin {
          0% { transform: rotate(0deg); }
          100% { transform: rotate(360deg); }
        }
      </style>
    </head>
    <body>
      <div class="container">
        <h1>🖼️ AI Image Analyzer</h1>
        <p>Upload an image to get AI-powered description and analysis</p>
        
        <div class="upload-area" id="uploadArea">
          <p style="font-size: 48px; margin: 0;">📷</p>
          <p>Click to upload or drag and drop an image</p>
          <input type="file" id="fileInput" accept="image/*" style="display: none;">
        </div>
        
        <img id="imagePreview" alt="Preview">
        
        <div style="text-align: center;">
          <button id="analyzeBtn" style="display: none;">Analyze Image</button>
        </div>
        
        <div class="loading" id="loading">
          <div class="spinner"></div>
          <p>Analyzing image...</p>
        </div>
        
        <div class="results" id="results">
          <h3>Analysis Results:</h3>
          <div class="result-item">
            <div class="result-label">Description:</div>
            <div id="description"></div>
          </div>
          <div class="result-item">
            <div class="result-label">Detected Objects:</div>
            <div id="objects"></div>
          </div>
          <div class="result-item">
            <div class="result-label">Colors:</div>
            <div id="colors"></div>
          </div>
          <div class="result-item">
            <div class="result-label">Suggested Tags:</div>
            <div id="tags"></div>
          </div>
        </div>
      </div>
      
      <script>
        const uploadArea = document.getElementById('uploadArea');
        const fileInput = document.getElementById('fileInput');
        const imagePreview = document.getElementById('imagePreview');
        const analyzeBtn = document.getElementById('analyzeBtn');
        const loading = document.getElementById('loading');
        const results = document.getElementById('results');
        
        let selectedFile = null;
        
        uploadArea.addEventListener('click', () => fileInput.click());
        
        fileInput.addEventListener('change', (e) => {
          const file = e.target.files[0];
          if (file && file.type.startsWith('image/')) {
            selectedFile = file;
            displayImage(file);
          }
        });
        
        uploadArea.addEventListener('dragover', (e) => {
          e.preventDefault();
          uploadArea.classList.add('dragover');
        });
        
        uploadArea.addEventListener('dragleave', () => {
          uploadArea.classList.remove('dragover');
        });
        
        uploadArea.addEventListener('drop', (e) => {
          e.preventDefault();
          uploadArea.classList.remove('dragover');
          
          const file = e.dataTransfer.files[0];
          if (file && file.type.startsWith('image/')) {
            selectedFile = file;
            fileInput.files = e.dataTransfer.files;
            displayImage(file);
          }
        });
        
        function displayImage(file) {
          const reader = new FileReader();
          reader.onload = (e) => {
            imagePreview.src = e.target.result;
            imagePreview.classList.add('show');
            analyzeBtn.style.display = 'inline-block';
            results.classList.remove('show');
          };
          reader.readAsDataURL(file);
        }
        
        analyzeBtn.addEventListener('click', async () => {
          if (!selectedFile) return;
          
          loading.classList.add('show');
          results.classList.remove('show');
          analyzeBtn.disabled = true;
          
          // Convert image to base64
          const reader = new FileReader();
          reader.onload = async (e) => {
            try {
              const response = await fetch('/api/analyze-image', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                  image: e.target.result,
                  filename: selectedFile.name
                })
              });
              
              const data = await response.json();
              
              if (response.ok) {
                displayResults(data);
              } else {
                alert('Error: ' + (data.error || 'Analysis failed'));
              }
            } catch (error) {
              alert('Error: ' + error.message);
            } finally {
              loading.classList.remove('show');
              analyzeBtn.disabled = false;
            }
          };
          reader.readAsDataURL(selectedFile);
        });
        
        function displayResults(data) {
          document.getElementById('description').textContent = data.description;
          document.getElementById('objects').textContent = data.objects.join(', ');
          
          const colorsHTML = data.colors.map(color => 
            \`<span style="display: inline-block; width: 30px; height: 30px; background: \${color}; border-radius: 4px; margin: 2px;"></span>\`
          ).join('');
          document.getElementById('colors').innerHTML = colorsHTML;
          
          const tagsHTML = data.tags.map(tag => 
            \`<span style="display: inline-block; background: #e3f2fd; padding: 5px 10px; border-radius: 15px; margin: 2px;">\${tag}</span>\`
          ).join('');
          document.getElementById('tags').innerHTML = tagsHTML;
          
          results.classList.add('show');
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

function analyzeImage(request) {
  const data = JSON.parse(request.body || "{}");

  console.log("Image analysis requested", {
    filename: data.filename,
  });

  // Mock AI image analysis
  // In real implementation, you would:
  // 1. Decode base64 image
  // 2. Call AI vision API (OpenAI, Google Vision, etc.)
  // 3. Return actual analysis results

  const mockAnalysis = {
    description:
      "A vibrant outdoor scene featuring natural elements. The image shows good composition with balanced colors and lighting.",
    objects: ["sky", "trees", "landscape", "nature"],
    colors: ["#4A90E2", "#7ED321", "#F5A623", "#8B572A"],
    tags: ["outdoor", "nature", "scenic", "daylight", "landscape photography"],
  };

  return {
    status: 200,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(mockAnalysis),
  };
}

init();
```

## AI-Powered Search

Intelligent search with AI:

```javascript
function init() {
  routeRegistry.registerRoute("GET", "/smart-search", showSmartSearch);
  routeRegistry.registerRoute("POST", "/api/search", performSearch);
}

function showSmartSearch(request) {
  const html = `
    <!DOCTYPE html>
    <html>
    <head>
      <title>AI Smart Search</title>
      <style>
        body {
          font-family: Arial, sans-serif;
          margin: 0;
          padding: 40px 20px;
          background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
          min-height: 100vh;
        }
        .search-container {
          max-width: 800px;
          margin: 0 auto;
        }
        h1 {
          text-align: center;
          color: white;
          margin-bottom: 40px;
          font-size: 2.5em;
        }
        .search-box {
          background: white;
          padding: 10px;
          border-radius: 50px;
          box-shadow: 0 4px 20px rgba(0,0,0,0.2);
          display: flex;
          margin-bottom: 30px;
        }
        .search-box input {
          flex: 1;
          border: none;
          padding: 15px 20px;
          font-size: 16px;
          border-radius: 50px;
        }
        .search-box input:focus {
          outline: none;
        }
        .search-box button {
          background: #667eea;
          color: white;
          border: none;
          padding: 15px 30px;
          border-radius: 50px;
          cursor: pointer;
          font-weight: bold;
          margin-left: 10px;
        }
        .results {
          background: white;
          border-radius: 12px;
          padding: 30px;
          box-shadow: 0 4px 20px rgba(0,0,0,0.2);
          display: none;
        }
        .results.show {
          display: block;
        }
        .result-item {
          padding: 20px;
          margin: 15px 0;
          background: #f8f9fa;
          border-radius: 8px;
          border-left: 4px solid #667eea;
          cursor: pointer;
          transition: all 0.3s;
        }
        .result-item:hover {
          transform: translateX(5px);
          box-shadow: 0 2px 8px rgba(0,0,0,0.1);
        }
        .result-title {
          font-size: 1.2em;
          font-weight: bold;
          color: #667eea;
          margin-bottom: 10px;
        }
        .result-snippet {
          color: #666;
          line-height: 1.6;
        }
        .result-relevance {
          font-size: 0.85em;
          color: #999;
          margin-top: 10px;
        }
        .ai-summary {
          background: #e3f2fd;
          padding: 20px;
          border-radius: 8px;
          margin-bottom: 20px;
          border-left: 4px solid #2196f3;
        }
        .ai-summary h3 {
          margin-top: 0;
          color: #1976d2;
        }
        .loading {
          text-align: center;
          color: white;
          display: none;
        }
        .loading.show {
          display: block;
        }
        .suggestions {
          color: white;
          text-align: center;
          margin-top: 20px;
        }
        .suggestion-chip {
          display: inline-block;
          background: rgba(255,255,255,0.2);
          padding: 8px 16px;
          margin: 5px;
          border-radius: 20px;
          cursor: pointer;
          transition: all 0.3s;
        }
        .suggestion-chip:hover {
          background: rgba(255,255,255,0.3);
        }
      </style>
    </head>
    <body>
      <div class="search-container">
        <h1>🔍 AI Smart Search</h1>
        
        <div class="search-box">
          <input type="text" id="searchInput" placeholder="Ask me anything..." autocomplete="off">
          <button id="searchBtn">Search</button>
        </div>
        
        <div class="suggestions">
          <p>Try asking:</p>
          <span class="suggestion-chip" onclick="search('How does machine learning work?')">
            How does machine learning work?
          </span>
          <span class="suggestion-chip" onclick="search('Best programming languages 2024')">
            Best programming languages 2024
          </span>
          <span class="suggestion-chip" onclick="search('Climate change solutions')">
            Climate change solutions
          </span>
        </div>
        
        <div class="loading" id="loading">
          <p>🤔 Searching and analyzing...</p>
        </div>
        
        <div class="results" id="results">
          <div class="ai-summary" id="aiSummary"></div>
          <h3>Related Results:</h3>
          <div id="resultsList"></div>
        </div>
      </div>
      
      <script>
        const searchInput = document.getElementById('searchInput');
        const searchBtn = document.getElementById('searchBtn');
        const loading = document.getElementById('loading');
        const results = document.getElementById('results');
        const aiSummary = document.getElementById('aiSummary');
        const resultsList = document.getElementById('resultsList');
        
        function search(query = null) {
          const searchQuery = query || searchInput.value.trim();
          if (!searchQuery) return;
          
          if (query) searchInput.value = query;
          
          loading.classList.add('show');
          results.classList.remove('show');
          
          fetch('/api/search', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ query: searchQuery })
          })
          .then(response => response.json())
          .then(data => {
            displayResults(data);
            loading.classList.remove('show');
            results.classList.add('show');
          })
          .catch(error => {
            loading.classList.remove('show');
            alert('Search error: ' + error.message);
          });
        }
        
        searchBtn.addEventListener('click', () => search());
        searchInput.addEventListener('keypress', (e) => {
          if (e.key === 'Enter') search();
        });
        
        function displayResults(data) {
          // Display AI summary
          aiSummary.innerHTML = \`
            <h3>💡 AI Summary</h3>
            <p>\${data.summary}</p>
          \`;
          
          // Display results
          resultsList.innerHTML = data.results.map(result => \`
            <div class="result-item">
              <div class="result-title">\${result.title}</div>
              <div class="result-snippet">\${result.snippet}</div>
              <div class="result-relevance">Relevance: \${result.relevance}%</div>
            </div>
          \`).join('');
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

function performSearch(request) {
  const data = JSON.parse(request.body || "{}");
  const query = data.query;

  console.log("AI search performed", { query });

  // Mock AI-powered search results
  const mockResults = {
    summary: `Based on your query "${query}", here's what I found: This topic involves multiple aspects including technical implementation, best practices, and recent developments in the field. The information below provides comprehensive coverage of the subject.`,
    results: [
      {
        title: "Comprehensive Guide to " + query,
        snippet:
          "This detailed guide covers everything you need to know about " +
          query +
          ". Learn from basics to advanced concepts with practical examples.",
        relevance: 95,
      },
      {
        title: query + ": Best Practices and Tips",
        snippet:
          "Discover industry-standard best practices and expert tips for working with " +
          query +
          ". Updated with latest trends and methodologies.",
        relevance: 88,
      },
      {
        title: "Common Challenges with " + query,
        snippet:
          "Understanding and overcoming common challenges when dealing with " +
          query +
          ". Real-world solutions and troubleshooting advice.",
        relevance: 82,
      },
      {
        title: "Future of " + query,
        snippet:
          "Explore emerging trends and future developments in " +
          query +
          ". Expert predictions and industry insights for the coming years.",
        relevance: 75,
      },
    ],
  };

  return {
    status: 200,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(mockResults),
  };
}

init();
```

## Integrating External AI APIs

Example of integrating with external AI services:

```javascript
// Example: OpenAI Integration (conceptual)
function callOpenAI(prompt) {
  // Note: You would need to handle API keys securely
  // This is a conceptual example

  const apiKey = "your-api-key"; // Should be stored securely
  const endpoint = "https://api.openai.com/v1/chat/completions";

  const response = fetch(endpoint, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: "Bearer " + apiKey,
    },
    body: JSON.stringify({
      model: "gpt-3.5-turbo",
      messages: [{ role: "user", content: prompt }],
    }),
  });

  // Handle response
  // Note: fetch() API usage depends on aiwebengine's implementation
}

// Example: Anthropic Claude Integration
function callClaude(prompt) {
  // Similar pattern for Claude API
  const response = fetch("https://api.anthropic.com/v1/messages", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "x-api-key": "your-api-key",
      "anthropic-version": "2023-06-01",
    },
    body: JSON.stringify({
      model: "claude-3-sonnet-20240229",
      max_tokens: 1024,
      messages: [{ role: "user", content: prompt }],
    }),
  });
}
```

## Best Practices

### 1. Handle AI API Errors Gracefully

```javascript
async function callAI(prompt) {
  try {
    const response = await fetch(aiEndpoint, {
      method: "POST",
      body: JSON.stringify({ prompt }),
    });

    if (!response.ok) {
      throw new Error("AI API error");
    }

    return await response.json();
  } catch (error) {
    console.error("AI API failed", { error: error.message });
    return { error: "AI service temporarily unavailable" };
  }
}
```

### 2. Implement Rate Limiting

```javascript
let lastRequest = 0;
const MIN_INTERVAL = 1000; // 1 second between requests

function rateLimitedAICall(prompt) {
  const now = Date.now();
  if (now - lastRequest < MIN_INTERVAL) {
    return { error: "Please wait before making another request" };
  }

  lastRequest = now;
  return callAI(prompt);
}
```

### 3. Cache AI Responses

```javascript
const aiCache = {};

function getCachedAIResponse(prompt) {
  if (aiCache[prompt]) {
    return aiCache[prompt];
  }

  const response = callAI(prompt);
  aiCache[prompt] = response;
  return response;
}
```

### 4. Sanitize User Input

```javascript
function sanitizePrompt(userInput) {
  // Remove potentially harmful content
  return userInput
    .replace(/<script>/gi, "")
    .replace(/javascript:/gi, "")
    .trim()
    .slice(0, 1000); // Limit length
}
```

### 5. Log AI Interactions

```javascript
function logAIInteraction(prompt, response, duration) {
  console.log("AI interaction", {
    promptLength: prompt.length,
    responseLength: response.length,
    duration: duration,
    timestamp: new Date().toISOString(),
  });
}
```

## Next Steps

- **[Basic API Examples](basic-api.md)** - REST API patterns
- **[Forms and Data](forms-and-data.md)** - Form handling
- **[Real-Time Features](real-time-features.md)** - Live updates
- **[AI Development Guide](../guides/ai-development.md)** - AI assistant usage
- **[JavaScript APIs](../reference/javascript-apis.md)** - Available functions

## Quick Reference

```javascript
// Mock AI response (for prototyping)
function generateAIResponse(prompt) {
  return "AI-generated response to: " + prompt;
}

// Call external AI API
const response = fetch(aiEndpoint, {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify({ prompt: userInput }),
});

// Handle AI errors
try {
  const result = await callAI(prompt);
} catch (error) {
  console.error("AI failed", { error });
}
```

Build intelligent applications with AI integration! 🤖✨
