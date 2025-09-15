// Test script for form data handling
function form_handler(req) {
    let formInfo = 'none';
    if (req.form && Object.keys(req.form).length > 0) {
        // req.form is now an object with parsed form data
        let params = [];
        for (let key in req.form) {
            params.push(`${key}=${req.form[key]}`);
        }
        formInfo = params.join(', ');
    }

    return {
        status: 200,
        body: `Path: ${req.path}, Method: ${req.method}, Form: ${formInfo}`,
        contentType: "text/plain"
    };
}

// Register handler for form test
register('/api/form', 'form_handler', 'POST');