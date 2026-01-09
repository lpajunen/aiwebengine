/// <reference path="../../assets/aiwebengine.d.ts" />

// Test TSX/JSX rendering with multiple children
function tsxHandler(context) {
  const items = ["Apple", "Banana", "Cherry"];
  
  const html = (
    <div className="container">
      <h1>Fruit List</h1>
      <ul>
        {items.map(item => (
          <li key={item}>{item}</li>
        ))}
      </ul>
      <p>Total: {items.length} items</p>
    </div>
  );
  
  return {
    status: 200,
    body: html,
    contentType: "text/html",
  };
}

// Register the route
function init(context) {
  console.log("Initializing tsx_test.tsx at " + new Date().toISOString());
  routeRegistry.registerRoute("/tsx", "tsxHandler", "GET");
  console.log("TSX test endpoint registered");
  return { success: true };
}
