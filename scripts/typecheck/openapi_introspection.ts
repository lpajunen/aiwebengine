/// <reference path="../../assets/aiwebengine-priv.d.ts" />

function assertOpenApiIntrospectionPayload(): void {
  const openapi = JSON.parse(routeRegistry.generateOpenApi()) as OpenApiSpec;

  const version: string = openapi.openapi;
  const title: string = openapi.info.title;
  const apiVersion: string = openapi.info.version;
  const paths: Record<string, Record<string, OpenApiOperation>> = openapi.paths;
  const tags: OpenApiTag[] | undefined = openapi.tags;

  void version;
  void title;
  void apiVersion;
  void tags;

  for (const [path, operations] of Object.entries(paths)) {
    const normalizedPath: string = path;
    void normalizedPath;

    for (const [method, operation] of Object.entries(operations)) {
      const normalizedMethod: string = method;
      const parameters: OpenApiParameter[] | undefined = operation.parameters;
      const requestBody: OpenApiRequestBody | undefined = operation.requestBody;
      const responses: Record<string, OpenApiResponse> = operation.responses;
      const scriptUri: string | undefined = operation["x-script-uri"] as
        | string
        | undefined;
      const handler: string | undefined = operation["x-handler"] as
        | string
        | undefined;

      if (parameters) {
        for (const parameter of parameters) {
          const parameterName: string = parameter.name;
          const parameterIn: string = parameter.in;
          const parameterRequired: boolean | undefined = parameter.required;
          const parameterSchema: OpenApiSchema | undefined = parameter.schema;

          void parameterName;
          void parameterIn;
          void parameterRequired;
          void parameterSchema;
        }
      }

      if (requestBody) {
        const requestBodyDescription: string | undefined =
          requestBody.description;
        const requestBodyRequired: boolean | undefined = requestBody.required;
        const requestBodyContent: Record<string, OpenApiMediaType> =
          requestBody.content;

        void requestBodyDescription;
        void requestBodyRequired;

        for (const mediaType of Object.values(requestBodyContent)) {
          const schema: OpenApiSchema | undefined = mediaType.schema;
          void schema;
        }
      }

      for (const response of Object.values(responses)) {
        const description: string = response.description;
        const content: Record<string, OpenApiMediaType> | undefined =
          response.content;

        void description;

        if (content) {
          for (const mediaType of Object.values(content)) {
            const schema: OpenApiSchema | undefined = mediaType.schema;
            const schemaType: string | undefined = schema?.type;
            const schemaFormat: string | undefined = schema?.format;

            void schema;
            void schemaType;
            void schemaFormat;
          }
        }
      }

      void normalizedMethod;
      void parameters;
      void requestBody;
      void responses;
      void scriptUri;
      void handler;
    }
  }
}

assertOpenApiIntrospectionPayload();
