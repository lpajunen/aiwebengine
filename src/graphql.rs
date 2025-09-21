use async_graphql::{
    dynamic::*,
};
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

/// Global GraphQL registry instance
lazy_static::lazy_static! {
    pub static ref GRAPHQL_REGISTRY: Arc<RwLock<GraphQLRegistry>> = Arc::new(RwLock::new(GraphQLRegistry::new()));
}

/// Get a reference to the global GraphQL registry
pub fn get_registry() -> Arc<RwLock<GraphQLRegistry>> {
    Arc::clone(&GRAPHQL_REGISTRY)
}

/// Register a GraphQL query from JavaScript
pub fn register_graphql_query(name: String, sdl: String, resolver_function: String, script_uri: String) {
    let operation = GraphQLOperation {
        sdl,
        resolver_function,
        script_uri,
    };

    if let Ok(mut registry) = get_registry().write() {
        registry.register_query(name, operation);
    } else {
        error!("Failed to acquire write lock on GraphQL registry");
    }
}

/// Register a GraphQL mutation from JavaScript
pub fn register_graphql_mutation(name: String, sdl: String, resolver_function: String, script_uri: String) {
    let operation = GraphQLOperation {
        sdl,
        resolver_function,
        script_uri,
    };

    if let Ok(mut registry) = get_registry().write() {
        registry.register_mutation(name, operation);
    } else {
        error!("Failed to acquire write lock on GraphQL registry");
    }
}

/// Register a GraphQL subscription from JavaScript
pub fn register_graphql_subscription(name: String, sdl: String, resolver_function: String, script_uri: String) {
    let operation = GraphQLOperation {
        sdl,
        resolver_function,
        script_uri,
    };

    if let Ok(mut registry) = get_registry().write() {
        registry.register_subscription(name, operation);
    } else {
        error!("Failed to acquire write lock on GraphQL registry");
    }
}

/// Build a dynamic GraphQL schema from registered operations
pub fn build_schema() -> Result<Schema, async_graphql::Error> {
    let registry_arc = get_registry();
    let registry_guard = registry_arc.read().map_err(|e| {
        async_graphql::Error::new(format!("Failed to read GraphQL registry: {}", e))
    })?;

    // Collect the data we need before creating closures
    let queries: Vec<(String, GraphQLOperation)> = registry_guard.get_queries().clone().into_iter().collect();
    let mutations: Vec<(String, GraphQLOperation)> = registry_guard.get_mutations().clone().into_iter().collect();
    let subscriptions: Vec<(String, GraphQLOperation)> = registry_guard.get_subscriptions().clone().into_iter().collect();

    // Check if we have queries before building
    let has_queries = !queries.is_empty();
    let has_mutations = !mutations.is_empty();
    let has_subscriptions = !subscriptions.is_empty();

    // Drop the guard so we don't have borrowing issues
    drop(registry_guard);

    let mut builder = Schema::build("Query", None, None);

    // Build Query type
    let mut query_builder = Object::new("Query");

    // Add registered queries
    for (name, operation) in queries {
        // For now, create a simple string field - we'll enhance this to parse SDL and call JS resolvers
        let field_name = name.clone();
        let resolver_uri = operation.script_uri.clone();
        let resolver_fn = operation.resolver_function.clone();

        query_builder = query_builder.field(Field::new(
            field_name,
            TypeRef::named(TypeRef::STRING),
            move |_ctx| {
                let uri = resolver_uri.clone();
                let func = resolver_fn.clone();
                FieldFuture::new(async move {
                    // TODO: Call JavaScript resolver function
                    // For now, return a placeholder
                    Ok(Some(async_graphql::Value::String(format!("Resolver {}::{} called", uri, func))))
                })
            },
        ));
    }

    // If no queries registered, add a placeholder
    if !has_queries {
        query_builder = query_builder.field(Field::new(
            "placeholder",
            TypeRef::named(TypeRef::STRING),
            |_| FieldFuture::new(async {
                Ok(Some(async_graphql::Value::String("No queries registered yet".to_string())))
            }),
        ));
    }

    builder = builder.register(query_builder);

    // Build Mutation type if mutations exist
    if has_mutations {
        let mut mutation_builder = Object::new("Mutation");

        for (name, operation) in mutations {
            let field_name = name.clone();
            let resolver_uri = operation.script_uri.clone();
            let resolver_fn = operation.resolver_function.clone();

            mutation_builder = mutation_builder.field(Field::new(
                field_name,
                TypeRef::named(TypeRef::STRING),
                move |_ctx| {
                    let uri = resolver_uri.clone();
                    let func = resolver_fn.clone();
                    FieldFuture::new(async move {
                        // TODO: Call JavaScript resolver function
                        Ok(Some(async_graphql::Value::String(format!("Mutation {}::{} called", uri, func))))
                    })
                },
            ));
        }

        builder = builder.register(mutation_builder);
    }

    // Build Subscription type if subscriptions exist
    if has_subscriptions {
        let mut subscription_builder = Object::new("Subscription");

        for (name, operation) in subscriptions {
            let field_name = name.clone();
            let resolver_uri = operation.script_uri.clone();
            let resolver_fn = operation.resolver_function.clone();

            subscription_builder = subscription_builder.field(Field::new(
                field_name,
                TypeRef::named_nn(TypeRef::STRING), // Non-null for subscriptions
                move |_ctx| {
                    let uri = resolver_uri.clone();
                    let func = resolver_fn.clone();
                    FieldFuture::new(async move {
                        // TODO: Call JavaScript resolver function for streaming
                        Ok(Some(async_graphql::Value::String(format!("Subscription {}::{} started", uri, func))))
                    })
                },
            ));
        }

        builder = builder.register(subscription_builder);
    }

    builder.finish().map_err(|e| async_graphql::Error::new(format!("Schema build error: {}", e)))
}