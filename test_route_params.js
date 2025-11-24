routeRegistry.registerRoute("/api/users/:id", "handleUser", "GET");

function handleUser(request) {
  const userId = request.params.id;
  return {
    status: 200,
    body: JSON.stringify({
      message: "User retrieved successfully",
      userId: userId,
      path: request.path,
      method: request.method,
    }),
    contentType: "application/json",
  };
}
