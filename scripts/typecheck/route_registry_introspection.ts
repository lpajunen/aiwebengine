/// <reference path="../../assets/aiwebengine-priv.d.ts" />

function assertRouteIntrospectionPayload(): void {
  const routes = JSON.parse(
    routeRegistry.listRoutes(),
  ) as RouteIntrospectionEntry[];

  for (const route of routes) {
    const path: string = route.path;
    const method: string = route.method;
    const handler: string | null = route.handler;
    const scriptUri: string = route.script_uri;
    const summary: string | null = route.summary;
    const description: string | null = route.description;
    const tags: string[] = route.tags;

    void path;
    void method;
    void handler;
    void scriptUri;
    void summary;
    void description;
    void tags;
  }
}

function assertStreamIntrospectionPayload(): void {
  const streams = JSON.parse(
    routeRegistry.listStreams(),
  ) as StreamIntrospectionEntry[];

  for (const stream of streams) {
    const path: string = stream.path;
    const scriptUri: string = stream.script_uri;

    void path;
    void scriptUri;
  }
}

assertRouteIntrospectionPayload();
assertStreamIntrospectionPayload();
