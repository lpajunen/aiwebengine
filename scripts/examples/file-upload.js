/**
 * File Upload Example
 *
 * This example demonstrates handling file uploads via multipart form data.
 * Files are received as base64-encoded data with metadata (filename, content type, size).
 *
 * Usage:
 * POST /upload with multipart/form-data containing:
 * - file field with uploaded file
 * - optional text fields (name, description, etc.)
 *
 * Maximum file size: 10MB (configurable via max_upload_size_bytes)
 */

function handleUpload(context) {
  const { request } = context;

  // Check if any files were uploaded
  if (!request.files || request.files.length === 0) {
    return {
      status: 400,
      body: JSON.stringify({
        error: "No files uploaded",
        message: "Please include a file in your multipart form data",
      }),
      contentType: "application/json",
    };
  }

  // Process uploaded files
  const filesInfo = request.files.map((file) => {
    // Decode base64 data if needed (for text files)
    let preview = null;
    if (file.contentType && file.contentType.startsWith("text/")) {
      try {
        // Note: In JavaScript, you would use atob() or Buffer to decode base64
        // For this example, we'll just show the metadata
        preview = `${file.size} bytes of text`;
      } catch (e) {
        preview = "Unable to preview";
      }
    }

    return {
      field: file.field,
      filename: file.filename || "unnamed",
      contentType: file.contentType || "unknown",
      size: file.size,
      preview: preview,
      // In a real application, you might:
      // - Save the decoded data to storage using storeAsset()
      // - Process image files
      // - Validate file types
      // - Scan for malware
    };
  });

  // Extract form fields (non-file data)
  const formFields = {};
  for (const [key, value] of Object.entries(request.form)) {
    formFields[key] = value;
  }

  return {
    status: 200,
    body: JSON.stringify(
      {
        message: "Files uploaded successfully",
        filesCount: request.files.length,
        files: filesInfo,
        formFields: formFields,
        metadata: {
          path: request.path,
          method: request.method,
        },
      },
      null,
      2,
    ),
    contentType: "application/json",
  };
}

// Register the route
registerRoute("POST", "/upload", handleUpload);
