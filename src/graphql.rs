use async_graphql::dynamic::*;
use async_stream;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::{debug, error};

/// Represents a GraphQL operation registration from JavaScript
#[derive(Debug, Clone)]
pub struct GraphQLOperation {
    /// The GraphQL SDL definition (type definitions, field definitions)
    pub sdl: String,
    /// The resolver function name in the JavaScript script
    pub resolver_function: String,
    /// The script URI that contains this operation
    pub script_uri: String,
}

/// Registry for storing GraphQL operations registered from JavaScript
#[derive(Debug, Clone, Default)]
pub struct GraphQLRegistry {
    /// Registered queries
    pub queries: HashMap<String, GraphQLOperation>,
    /// Registered mutations
    pub mutations: HashMap<String, GraphQLOperation>,
    /// Registered subscriptions
    pub subscriptions: HashMap<String, GraphQLOperation>,
}

impl GraphQLRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all registrations from a specific script URI
    pub fn clear_script_registrations(&mut self, script_uri: &str) {
        debug!("Clearing GraphQL registrations for script: {}", script_uri);

        // Remove queries from this script
        let queries_to_remove: Vec<String> = self
            .queries
            .iter()
            .filter(|(_, op)| op.script_uri == script_uri)
            .map(|(name, _)| name.clone())
            .collect();

        for query_name in queries_to_remove {
            self.queries.remove(&query_name);
            debug!(
                "Removed query '{}' from script '{}'",
                query_name, script_uri
            );
        }

        // Remove mutations from this script
        let mutations_to_remove: Vec<String> = self
            .mutations
            .iter()
            .filter(|(_, op)| op.script_uri == script_uri)
            .map(|(name, _)| name.clone())
            .collect();

        for mutation_name in mutations_to_remove {
            self.mutations.remove(&mutation_name);
            debug!(
                "Removed mutation '{}' from script '{}'",
                mutation_name, script_uri
            );
        }

        // Remove subscriptions from this script
        let subscriptions_to_remove: Vec<String> = self
            .subscriptions
            .iter()
            .filter(|(_, op)| op.script_uri == script_uri)
            .map(|(name, _)| name.clone())
            .collect();

        for subscription_name in subscriptions_to_remove {
            self.subscriptions.remove(&subscription_name);
            debug!(
                "Removed subscription '{}' from script '{}'",
                subscription_name, script_uri
            );
        }
    }

    /// Register a GraphQL query
    pub fn register_query(&mut self, name: String, operation: GraphQLOperation) {
        debug!("Registering GraphQL query: {}", name);
        self.queries.insert(name, operation);
    }

    /// Register a GraphQL mutation
    pub fn register_mutation(&mut self, name: String, operation: GraphQLOperation) {
        debug!("Registering GraphQL mutation: {}", name);
        self.mutations.insert(name, operation);
    }

    /// Register a GraphQL subscription
    pub fn register_subscription(&mut self, name: String, operation: GraphQLOperation) {
        debug!("Registering GraphQL subscription: {}", name);
        self.subscriptions.insert(name, operation);
    }

    /// Get all registered queries
    pub fn get_queries(&self) -> &HashMap<String, GraphQLOperation> {
        &self.queries
    }

    /// Get all registered mutations
    pub fn get_mutations(&self) -> &HashMap<String, GraphQLOperation> {
        &self.mutations
    }

    /// Get all registered subscriptions
    pub fn get_subscriptions(&self) -> &HashMap<String, GraphQLOperation> {
        &self.subscriptions
    }
}

lazy_static::lazy_static! {
    pub static ref GRAPHQL_REGISTRY: Arc<RwLock<GraphQLRegistry>> = Arc::new(RwLock::new(GraphQLRegistry::new()));
    pub static ref GRAPHQL_SCHEMA: Arc<RwLock<Option<Schema>>> = Arc::new(RwLock::new(None));
}

/// Get a reference to the global GraphQL registry
pub fn get_registry() -> Arc<RwLock<GraphQLRegistry>> {
    Arc::clone(&GRAPHQL_REGISTRY)
}

/// Get the current GraphQL schema, rebuilding it if necessary
pub fn get_schema() -> Result<Schema, async_graphql::Error> {
    // Try to get existing schema
    if let Ok(schema_guard) = GRAPHQL_SCHEMA.read()
        && let Some(ref schema) = *schema_guard
    {
        return Ok(schema.clone());
    }

    // Schema doesn't exist or needs rebuild, build it
    rebuild_schema()
}

/// Rebuild the GraphQL schema from the current registry
pub fn rebuild_schema() -> Result<Schema, async_graphql::Error> {
    let schema = build_schema()?;

    // Store the new schema
    if let Ok(mut schema_guard) = GRAPHQL_SCHEMA.write() {
        *schema_guard = Some(schema.clone());
        debug!("GraphQL schema rebuilt successfully");
    } else {
        error!("Failed to store rebuilt GraphQL schema");
    }

    Ok(schema)
}

/// Clear all GraphQL registrations from a specific script URI
pub fn clear_script_graphql_registrations(script_uri: &str) {
    debug!("Clearing GraphQL registrations for script: {}", script_uri);
    if let Ok(mut registry) = get_registry().write() {
        registry.clear_script_registrations(script_uri);
        debug!(
            "Successfully cleared GraphQL registrations for script: {}",
            script_uri
        );
    } else {
        error!("Failed to acquire write lock on GraphQL registry for clearing");
    }
}

/// Register a GraphQL query from JavaScript
pub fn register_graphql_query(
    name: String,
    sdl: String,
    resolver_function: String,
    script_uri: String,
) {
    debug!(
        "Registering GraphQL query: {} with resolver: {} from script: {}",
        name, resolver_function, script_uri
    );
    let operation = GraphQLOperation {
        sdl,
        resolver_function,
        script_uri: script_uri.clone(),
    };

    if let Ok(mut registry) = get_registry().write() {
        registry.register_query(name.clone(), operation);
        debug!(
            "Successfully registered GraphQL query: {} - total queries: {}",
            name,
            registry.get_queries().len()
        );
    } else {
        error!("Failed to acquire write lock on GraphQL registry");
    }
}

/// Register a GraphQL mutation from JavaScript
pub fn register_graphql_mutation(
    name: String,
    sdl: String,
    resolver_function: String,
    script_uri: String,
) {
    let operation = GraphQLOperation {
        sdl,
        resolver_function,
        script_uri: script_uri.clone(),
    };

    if let Ok(mut registry) = get_registry().write() {
        registry.register_mutation(name, operation);
    } else {
        error!("Failed to acquire write lock on GraphQL registry");
    }
}

/// Register a GraphQL subscription from JavaScript
pub fn register_graphql_subscription(
    name: String,
    sdl: String,
    resolver_function: String,
    script_uri: String,
) {
    debug!(
        "Registering GraphQL subscription: {} with resolver: {} from script: {}",
        name, resolver_function, script_uri
    );

    let operation = GraphQLOperation {
        sdl,
        resolver_function,
        script_uri: script_uri.clone(),
    };

    // With execute_stream, we still need stream paths for sendSubscriptionMessage compatibility
    // This ensures existing JavaScript APIs continue to work
    let stream_path = format!("/engine/graphql/subscription/{}", name);
    match crate::stream_registry::GLOBAL_STREAM_REGISTRY.register_stream(&stream_path, &script_uri)
    {
        Ok(()) => {
            debug!(
                "Registered compatibility stream path '{}' for GraphQL subscription '{}'",
                stream_path, name
            );
        }
        Err(e) => {
            error!(
                "Failed to register compatibility stream path '{}' for subscription '{}': {}",
                stream_path, name, e
            );
        }
    }

    if let Ok(mut registry) = get_registry().write() {
        registry.register_subscription(name.clone(), operation);
        debug!(
            "Successfully registered GraphQL subscription: {} - total subscriptions: {}",
            name,
            registry.get_subscriptions().len()
        );
    } else {
        error!("Failed to acquire write lock on GraphQL registry");
    }
}

/// Parse SDL to extract type definitions
fn parse_types_from_sdl(sdl: &str) -> HashMap<String, Object> {
    let mut types = HashMap::new();
    debug!("Parsing SDL for types: {}", sdl);

    // Simple regex-based parsing for type definitions
    // This is a basic implementation - a full SDL parser would be more robust
    // Use captures_iter to find all type definitions in the SDL
    let type_regex = regex::Regex::new(r"type\s+(\w+)\s*\{([^}]+)\}").unwrap();
    let field_regex = regex::Regex::new(r"(\w+):\s*(\[?\w+!?\]?!?)").unwrap();

    for captures in type_regex.captures_iter(sdl) {
        let type_name = &captures[1];
        let fields_str = &captures[2];

        if type_name != "Query" && type_name != "Mutation" && type_name != "Subscription" {
            let mut object_builder = Object::new(type_name);

            // Parse fields and create resolvers that extract from the parent object
            for field_match in field_regex.captures_iter(fields_str) {
                let field_name = &field_match[1];
                let field_type = &field_match[2];

                let type_ref = match field_type {
                    "String!" => TypeRef::named_nn(TypeRef::STRING),
                    "String" => TypeRef::named(TypeRef::STRING),
                    "Int!" => TypeRef::named_nn(TypeRef::INT),
                    "Int" => TypeRef::named(TypeRef::INT),
                    "Boolean!" => TypeRef::named_nn(TypeRef::BOOLEAN),
                    "Boolean" => TypeRef::named(TypeRef::BOOLEAN),
                    _ => TypeRef::named(TypeRef::STRING), // Default to String for unknown types
                };

                // Create a field resolver that extracts the field value from the parent context
                // We need to access the field value from the JSON object that was passed as parent
                let field_name_owned = field_name.to_string();
                object_builder =
                    object_builder.field(Field::new(field_name, type_ref, move |ctx| {
                        let field_name = field_name_owned.clone();
                        FieldFuture::new(async move {
                            // Try to access the field from the parent value
                            // The parent should be a JSON object with the field data
                            if let Ok(parent_map) = ctx.parent_value.try_to_value()
                                && let async_graphql::Value::Object(obj) = parent_map
                                && let Some(field_value) =
                                    obj.get(&async_graphql::Name::new(&field_name))
                            {
                                return Ok(Some(field_value.clone()));
                            }
                            Ok(Some(async_graphql::Value::Null))
                        })
                    }));
            }

            types.insert(type_name.to_string(), object_builder);
        }
    }

    types
}

/// Extract return type from SDL field definition
fn extract_return_type(sdl: &str, field_name: &str) -> TypeRef {
    debug!(
        "Extracting return type for field '{}' from SDL: {}",
        field_name, sdl
    );
    let pattern = format!(
        r"{}\s*(?:\([^)]*\))?\s*:\s*(\[?\w+!?\]?!?)",
        regex::escape(field_name)
    );
    if let Some(captures) = regex::Regex::new(&pattern).unwrap().captures(sdl) {
        let type_str = &captures[1];
        debug!("Found type string: '{}'", type_str);
        match type_str {
            "String!" => TypeRef::named_nn(TypeRef::STRING),
            "String" => TypeRef::named(TypeRef::STRING),
            "Int!" => TypeRef::named_nn(TypeRef::INT),
            "Int" => TypeRef::named(TypeRef::INT),
            s if s.starts_with('[') && s.contains(']') => {
                // Handle array types like [ScriptInfo!]! or [ScriptInfo]
                let inner_type = s.trim_matches(|c| c == '[' || c == ']' || c == '!');
                debug!("Detected array type with inner type: '{}'", inner_type);
                TypeRef::named_nn_list_nn(inner_type)
            }
            _ => {
                // Check if it's a custom type
                debug!(
                    "Checking if SDL contains type definitions: {}",
                    regex::Regex::new(r"type\s+").unwrap().is_match(sdl)
                );
                if regex::Regex::new(r"type\s+").unwrap().is_match(sdl) {
                    let clean_type = type_str.trim_matches(|c| c == '[' || c == ']' || c == '!');
                    debug!("Using custom type: '{}'", clean_type);
                    if type_str.ends_with('!') {
                        TypeRef::named_nn(clean_type)
                    } else {
                        TypeRef::named(clean_type)
                    }
                } else {
                    debug!("Falling back to String type");
                    TypeRef::named(TypeRef::STRING)
                }
            }
        }
    } else {
        TypeRef::named(TypeRef::STRING)
    }
}

/// Extract arguments from SDL field definition
fn extract_arguments(sdl: &str, field_name: &str) -> Vec<(String, TypeRef)> {
    debug!(
        "Extracting arguments for field '{}' from SDL: {}",
        field_name, sdl
    );

    // Pattern to match field with arguments: fieldName(arg1: Type1, arg2: Type2): ReturnType
    let pattern = format!(r"{}\s*\(([^)]*)\)", regex::escape(field_name));

    if let Some(captures) = regex::Regex::new(&pattern).unwrap().captures(sdl) {
        let args_str = &captures[1];
        debug!("Found arguments string: '{}'", args_str);

        let mut arguments = Vec::new();

        // Split by comma and parse each argument
        for arg_part in args_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            if let Some(colon_pos) = arg_part.find(':') {
                let arg_name = arg_part[..colon_pos].trim().to_string();
                let type_str = arg_part[colon_pos + 1..].trim();

                debug!("Parsing argument: {} : {}", arg_name, type_str);

                let type_ref = match type_str {
                    "String!" => TypeRef::named_nn(TypeRef::STRING),
                    "String" => TypeRef::named(TypeRef::STRING),
                    "Int!" => TypeRef::named_nn(TypeRef::INT),
                    "Int" => TypeRef::named(TypeRef::INT),
                    "Boolean!" => TypeRef::named_nn(TypeRef::BOOLEAN),
                    "Boolean" => TypeRef::named(TypeRef::BOOLEAN),
                    "Float!" => TypeRef::named_nn(TypeRef::FLOAT),
                    "Float" => TypeRef::named(TypeRef::FLOAT),
                    s if s.starts_with('[') && s.contains(']') => {
                        // Handle array types like [String!]! or [String]
                        let inner_type = s.trim_matches(|c| c == '[' || c == ']' || c == '!');
                        debug!(
                            "Detected array argument type with inner type: '{}'",
                            inner_type
                        );
                        TypeRef::named_nn_list_nn(inner_type)
                    }
                    _ => {
                        // Check if it's a custom type
                        let clean_type =
                            type_str.trim_matches(|c| c == '[' || c == ']' || c == '!');
                        debug!("Using custom type for argument: '{}'", clean_type);
                        if type_str.ends_with('!') {
                            TypeRef::named_nn(clean_type)
                        } else {
                            TypeRef::named(clean_type)
                        }
                    }
                };

                arguments.push((arg_name, type_ref));
            }
        }

        debug!("Extracted {} arguments", arguments.len());
        arguments
    } else {
        debug!("No arguments found for field '{}'", field_name);
        Vec::new()
    }
}

/// Parse JSON result to appropriate GraphQL value
fn parse_json_to_graphql_value(json_str: &str) -> Result<async_graphql::Value, serde_json::Error> {
    let json_value: serde_json::Value = serde_json::from_str(json_str)?;

    fn convert_json_value(value: serde_json::Value) -> async_graphql::Value {
        match value {
            serde_json::Value::Null => async_graphql::Value::Null,
            serde_json::Value::Bool(b) => async_graphql::Value::Boolean(b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    async_graphql::Value::Number(async_graphql::Number::from(i))
                } else if let Some(f) = n.as_f64() {
                    // Convert f64 to i64 for now, or use string representation
                    async_graphql::Value::Number(async_graphql::Number::from(f as i64))
                } else {
                    async_graphql::Value::String(n.to_string())
                }
            }
            serde_json::Value::String(s) => async_graphql::Value::String(s),
            serde_json::Value::Array(arr) => {
                let graphql_array: Vec<async_graphql::Value> =
                    arr.into_iter().map(convert_json_value).collect();
                async_graphql::Value::List(graphql_array)
            }
            serde_json::Value::Object(obj) => {
                let mut graphql_object = indexmap::IndexMap::new();
                for (k, v) in obj {
                    graphql_object.insert(async_graphql::Name::new(k), convert_json_value(v));
                }
                async_graphql::Value::Object(graphql_object)
            }
        }
    }

    Ok(convert_json_value(json_value))
}

/// Build a dynamic GraphQL schema from registered operations
pub fn build_schema() -> Result<Schema, async_graphql::Error> {
    let registry_arc = get_registry();
    let registry_guard = registry_arc.read().map_err(|e| {
        async_graphql::Error::new(format!("Failed to read GraphQL registry: {}", e))
    })?;

    // Collect the data we need before creating closures
    let queries: Vec<(String, GraphQLOperation)> =
        registry_guard.get_queries().clone().into_iter().collect();
    let mutations: Vec<(String, GraphQLOperation)> =
        registry_guard.get_mutations().clone().into_iter().collect();
    let subscriptions: Vec<(String, GraphQLOperation)> = registry_guard
        .get_subscriptions()
        .clone()
        .into_iter()
        .collect();

    debug!(
        "Building GraphQL schema with {} queries, {} mutations, {} subscriptions",
        queries.len(),
        mutations.len(),
        subscriptions.len()
    );

    // Check if we have queries before building
    let has_queries = !queries.is_empty();
    let has_mutations = !mutations.is_empty();
    let has_subscriptions = !subscriptions.is_empty();

    // Drop the guard so we don't have borrowing issues
    drop(registry_guard);

    let mut builder = Schema::build(
        "Query",
        if has_mutations {
            Some("Mutation")
        } else {
            None
        },
        if has_subscriptions {
            Some("Subscription")
        } else {
            None
        },
    );

    // Register custom types from all SDL definitions
    let mut registered_types = std::collections::HashSet::new();
    for (_, operation) in &queries {
        for (type_name, custom_type) in parse_types_from_sdl(&operation.sdl) {
            debug!("Registering custom type from query: '{}'", type_name);
            if !registered_types.contains(&type_name) {
                builder = builder.register(custom_type);
                registered_types.insert(type_name.clone());
                debug!("Successfully registered type: '{}'", type_name);
            } else {
                debug!("Type '{}' already registered", type_name);
            }
        }
    }
    // Also register custom types from mutations
    for (_, operation) in &mutations {
        for (type_name, custom_type) in parse_types_from_sdl(&operation.sdl) {
            debug!("Registering custom type from mutation: '{}'", type_name);
            if !registered_types.contains(&type_name) {
                builder = builder.register(custom_type);
                registered_types.insert(type_name.clone());
                debug!("Successfully registered type: '{}'", type_name);
            } else {
                debug!("Type '{}' already registered", type_name);
            }
        }
    }
    // Also register custom types from subscriptions
    for (_, operation) in &subscriptions {
        for (type_name, custom_type) in parse_types_from_sdl(&operation.sdl) {
            debug!("Registering custom type from subscription: '{}'", type_name);
            if !registered_types.contains(&type_name) {
                builder = builder.register(custom_type);
                registered_types.insert(type_name.clone());
                debug!("Successfully registered type: '{}'", type_name);
            } else {
                debug!("Type '{}' already registered", type_name);
            }
        }
    }

    // Build Query type
    let mut query_builder = Object::new("Query");

    // Add registered queries
    for (name, operation) in queries {
        let field_name = name.clone();
        debug!("Adding query field: {}", field_name);
        let resolver_uri = operation.script_uri.clone();
        let resolver_fn = operation.resolver_function.clone();

        // Handle queries with dynamic argument parsing
        let return_type = extract_return_type(&operation.sdl, &field_name);
        let arguments = extract_arguments(&operation.sdl, &field_name);
        let arguments_clone = arguments.clone();

        let mut query_field = Field::new(field_name.clone(), return_type, move |ctx| {
            let uri = resolver_uri.clone();
            let func = resolver_fn.clone();
            let args_defs = arguments_clone.clone();
            FieldFuture::new(async move {
                // Extract auth context from GraphQL context
                let auth_context = ctx.data::<crate::auth::JsAuthContext>().ok().cloned();

                // Extract arguments from GraphQL context
                let mut args_json = serde_json::Map::new();
                for (arg_name, _arg_type) in &args_defs {
                    if let Some(accessor) = ctx.args.get(arg_name)
                        && let Ok(value) = accessor.deserialize::<serde_json::Value>()
                    {
                        args_json.insert(arg_name.clone(), value);
                    }
                }

                let args = if args_json.is_empty() {
                    None
                } else {
                    Some(serde_json::Value::Object(args_json))
                };

                // Call JavaScript resolver function
                match crate::js_engine::execute_graphql_resolver(&uri, &func, args, auth_context) {
                    Ok(result) => {
                        debug!("GraphQL resolver result: {}", &result);
                        // Special handling for JSON responses - parse and return as GraphQL value
                        if result.trim().starts_with('[') || result.trim().starts_with('{') {
                            debug!("Parsing JSON result from GraphQL resolver: {}", &result);
                            match parse_json_to_graphql_value(&result) {
                                Ok(json_value) => {
                                    debug!("Successfully parsed JSON value: {:?}", json_value);
                                    Ok(Some(json_value))
                                }
                                Err(e) => {
                                    error!("Failed to parse JSON from resolver: {}", e);
                                    Ok(Some(async_graphql::Value::String(result)))
                                }
                            }
                        } else {
                            Ok(Some(async_graphql::Value::String(result)))
                        }
                    }
                    Err(e) => {
                        error!("GraphQL resolver error for {}::{}: {}", uri, func, e);
                        Ok(Some(async_graphql::Value::String(format!("Error: {}", e))))
                    }
                }
            })
        });

        // Add arguments to the field
        for (arg_name, arg_type) in arguments {
            query_field = query_field.argument(InputValue::new(&arg_name, arg_type));
        }

        query_builder = query_builder.field(query_field);
        debug!("Added field {} to query builder", field_name);
    }
    if !has_queries {
        query_builder = query_builder.field(Field::new(
            "placeholder",
            TypeRef::named(TypeRef::STRING),
            |_| {
                FieldFuture::new(async {
                    Ok(Some(async_graphql::Value::String(
                        "No queries registered yet".to_string(),
                    )))
                })
            },
        ));
    }

    builder = builder.register(query_builder);

    // Build Mutation type (always create it, even if empty)
    let mut mutation_builder = Object::new("Mutation");

    if has_mutations {
        for (name, operation) in mutations {
            let field_name = name.clone();
            let resolver_uri = operation.script_uri.clone();
            let resolver_fn = operation.resolver_function.clone();

            // Handle mutations with dynamic argument parsing
            let return_type = extract_return_type(&operation.sdl, &field_name);
            let arguments = extract_arguments(&operation.sdl, &field_name);
            let arguments_clone = arguments.clone();
            let mut mutation_field = Field::new(field_name.clone(), return_type, move |ctx| {
                let uri = resolver_uri.clone();
                let func = resolver_fn.clone();
                let args_defs = arguments_clone.clone();
                FieldFuture::new(async move {
                    // Extract auth context from GraphQL context
                    let auth_context = ctx.data::<crate::auth::JsAuthContext>().ok().cloned();

                    // Extract arguments from GraphQL context
                    let mut args_json = serde_json::Map::new();
                    for (arg_name, _arg_type) in &args_defs {
                        if let Some(accessor) = ctx.args.get(arg_name)
                            && let Ok(value) = accessor.deserialize::<serde_json::Value>()
                        {
                            args_json.insert(arg_name.clone(), value);
                        }
                    }

                    let args = if args_json.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::Object(args_json))
                    };

                    // Call JavaScript resolver function
                    match crate::js_engine::execute_graphql_resolver(
                        &uri,
                        &func,
                        args,
                        auth_context,
                    ) {
                        Ok(result) => {
                            // Try to parse as JSON first, then as string
                            if result.trim().starts_with('{') || result.trim().starts_with('[') {
                                match serde_json::from_str::<serde_json::Value>(&result) {
                                    Ok(json_value) => {
                                        match async_graphql::Value::from_json(json_value) {
                                            Ok(graphql_value) => Ok(Some(graphql_value)),
                                            Err(_) => {
                                                Ok(Some(async_graphql::Value::String(result)))
                                            }
                                        }
                                    }
                                    Err(_) => Ok(Some(async_graphql::Value::String(result))),
                                }
                            } else {
                                Ok(Some(async_graphql::Value::String(result)))
                            }
                        }
                        Err(e) => {
                            error!("GraphQL resolver error for {}::{}: {}", uri, func, e);
                            Ok(Some(async_graphql::Value::String(format!("Error: {}", e))))
                        }
                    }
                })
            });

            // Add arguments to the field
            for (arg_name, arg_type) in arguments {
                mutation_field = mutation_field.argument(InputValue::new(&arg_name, arg_type));
            }

            mutation_builder = mutation_builder.field(mutation_field);
        }
    } else {
        // Add a placeholder mutation if no mutations are registered
        mutation_builder = mutation_builder.field(Field::new(
            "placeholder",
            TypeRef::named(TypeRef::STRING),
            |_| {
                FieldFuture::new(async {
                    Ok(Some(async_graphql::Value::String(
                        "No mutations registered yet".to_string(),
                    )))
                })
            },
        ));
    }

    builder = builder.register(mutation_builder);

    // Build Subscription type if subscriptions exist
    if has_subscriptions {
        let mut subscription_builder = Subscription::new("Subscription");

        for (name, operation) in subscriptions {
            let subscription_name = name.clone();
            let field_name = name.clone();
            let resolver_uri = operation.script_uri.clone();
            let resolver_fn = operation.resolver_function.clone();

            // Extract return type from SDL
            let return_type = extract_return_type(&operation.sdl, &field_name);

            // Create a proper streaming subscription field for execute_stream
            let subscription_field = SubscriptionField::new(
                field_name.clone(),
                return_type, // Use dynamic return type from SDL
                move |ctx| {
                    let subscription_name = subscription_name.clone();
                    let uri = resolver_uri.clone();
                    let func = resolver_fn.clone();

                    // Return a SubscriptionFieldFuture directly
                    SubscriptionFieldFuture::new(async move {
                        // Extract auth context from GraphQL context
                        let auth_context = ctx.data::<crate::auth::JsAuthContext>().ok().cloned();

                        // Initialize the subscription by calling the JavaScript resolver
                        let initial_result = match crate::js_engine::execute_graphql_resolver(
                            &uri,
                            &func,
                            None,
                            auth_context,
                        ) {
                            Ok(result) => {
                                debug!(
                                    "Subscription '{}' initialized: {}",
                                    subscription_name, result
                                );
                                // Try to parse as JSON object, fallback to string
                                match parse_json_to_graphql_value(&result) {
                                    Ok(graphql_value) => graphql_value,
                                    Err(_) => async_graphql::Value::String(result),
                                }
                            }
                            Err(e) => {
                                error!(
                                    "Failed to initialize subscription '{}': {}",
                                    subscription_name, e
                                );
                                async_graphql::Value::String(format!("Error: {}", e))
                            }
                        };

                        // For execute_stream compatibility, we need to maintain the legacy bridge
                        // where sendSubscriptionMessage still works via the stream registry
                        let stream_path =
                            format!("/engine/graphql/subscription/{}", subscription_name);

                        // Create a unified stream using boxed trait objects
                        use std::pin::Pin;

                        let stream: Pin<
                            Box<
                                dyn futures::Stream<
                                        Item = Result<async_graphql::Value, async_graphql::Error>,
                                    > + Send,
                            >,
                        > = if let Ok(connection) =
                            crate::stream_manager::StreamConnectionManager::new()
                                .create_connection(&stream_path, None)
                                .await
                        {
                            let mut receiver = connection.receiver;
                            let stream = async_stream::stream! {
                                // Yield initial result
                                yield Ok(initial_result);

                                // Then listen for broadcast messages from sendSubscriptionMessage
                                while let Ok(message) = receiver.recv().await {
                                    // Try to parse incoming message as JSON object, fallback to string
                                    let parsed_message = match parse_json_to_graphql_value(&message) {
                                        Ok(graphql_value) => graphql_value,
                                        Err(_) => async_graphql::Value::String(message)
                                    };
                                    yield Ok(parsed_message);
                                }
                            };
                            Box::pin(stream)
                        } else {
                            error!(
                                "Failed to create connection for subscription '{}'",
                                subscription_name
                            );
                            // Fallback: just emit the initial value
                            let stream = async_stream::stream! {
                                yield Ok(initial_result);
                            };
                            Box::pin(stream)
                        };

                        Ok(stream)
                    })
                },
            );

            subscription_builder = subscription_builder.field(subscription_field);
        }

        builder = builder.register(subscription_builder);
    }

    builder
        .finish()
        .map_err(|e| async_graphql::Error::new(format!("Schema build error: {}", e)))
}

/// Execute a GraphQL query synchronously and return the result as a JSON string.
/// This is used by the JavaScript executeGraphQL function.
pub fn execute_graphql_query_sync(
    query: &str,
    variables: Option<serde_json::Value>,
) -> Result<String, String> {
    debug!("Executing GraphQL query synchronously: {}", query);

    // Get the current schema
    let schema = get_schema().map_err(|e| format!("Failed to get GraphQL schema: {:?}", e))?;

    // Create the GraphQL request
    let mut request = async_graphql::Request::new(query);

    // Add variables if provided
    if let Some(vars) = variables {
        request = request.variables(async_graphql::Variables::from_json(vars));
    }

    // Execute the query synchronously
    // Since we're in a synchronous context but async_graphql requires async,
    // we need to create a minimal runtime to execute this
    let rt =
        tokio::runtime::Runtime::new().map_err(|e| format!("Failed to create runtime: {}", e))?;

    let response = rt.block_on(async { schema.execute(request).await });

    // Convert the response to JSON
    let json_result = serde_json::to_string(&response)
        .map_err(|e| format!("Failed to serialize GraphQL response: {}", e))?;

    debug!("GraphQL execution completed successfully");
    Ok(json_result)
}
