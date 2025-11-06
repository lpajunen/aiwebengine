// Test script for form data handling
function form_handler(req) {
  let formInfo = "none";
  if (req.form && Object.keys(req.form).length > 0) {
    // req.form is now an object with parsed form data
    let params = [];
    for (let key in req.form) {
      params.push(`${key}=${req.form[key]}`);
    }
    formInfo = params.join(", ");
  }

  return {
    status: 200,
    body: `Path: ${req.path}, Method: ${req.method}, Form: ${formInfo}`,
    contentType: "text/plain",
  };
}

// Initialization function
function init(context) {
  console.log("Initializing form_test.js at " + new Date().toISOString());
  register("/api/form", "form_handler", "POST");
  console.log("Form test endpoint registered");
  return { success: true };
}
