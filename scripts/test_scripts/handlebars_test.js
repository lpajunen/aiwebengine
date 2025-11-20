// Test script for the new render_handlebars_template function
// This script demonstrates the usage of convert.render_handlebars_template

function testHandlebarsTemplate(context) {
  const req = getRequest(context);

  try {
    // Test simple template
    const simpleTemplate = "Hello {{name}}!";
    const simpleData = JSON.stringify({ name: "World" });
    const simpleResult = convert.render_handlebars_template(
      simpleTemplate,
      simpleData,
    );

    // Test complex template with loops and conditionals
    const complexTemplate = `
<!DOCTYPE html>
<html>
<head><title>{{title}}</title></head>
<body>
  <h1>{{title}}</h1>
  {{#if showContent}}
  <div class="content">
    <p>{{content}}</p>
    {{#each items}}
    <div class="item">{{this.name}}: {{this.value}}</div>
    {{/each}}
  </div>
  {{/if}}
  <footer>Generated at {{timestamp}}</footer>
</body>
</html>`;

    const complexData = JSON.stringify({
      title: "Test Page",
      showContent: true,
      content: "This is dynamically generated content",
      items: [
        { name: "Item 1", value: "Value 1" },
        { name: "Item 2", value: "Value 2" },
      ],
      timestamp: new Date().toISOString(),
    });

    const complexResult = convert.render_handlebars_template(
      complexTemplate,
      complexData,
    );

    // Return results
    const response = {
      simple: {
        template: simpleTemplate,
        data: simpleData,
        result: simpleResult,
      },
      complex: {
        template: complexTemplate.trim(),
        data: complexData,
        result: complexResult,
      },
    };

    return {
      status: 200,
      body: JSON.stringify(response, null, 2),
      contentType: "application/json; charset=UTF-8",
    };
  } catch (error) {
    console.error("Error in testHandlebarsTemplate: " + error);
    return {
      status: 500,
      body: JSON.stringify({ error: error.message }),
      contentType: "application/json; charset=UTF-8",
    };
  }
}

function init(context) {
  console.log("Initializing handlebars test script");
  routeRegistry.registerRoute(
    "/test/handlebars",
    "testHandlebarsTemplate",
    "GET",
  );
  return { success: true };
}
