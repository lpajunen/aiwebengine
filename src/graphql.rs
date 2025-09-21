use async_graphql::{
    EmptyMutation, EmptySubscription, Object, Schema,
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
pub fn build_schema() -> Schema<Query, EmptyMutation, EmptySubscription> {
    // For now, return a basic schema - dynamic schema building will be implemented later
    Schema::new(Query, EmptyMutation, EmptySubscription)
}

// Placeholder root types for basic schema
#[derive(Default)]
pub struct Query;

#[Object]
impl Query {
    async fn placeholder(&self) -> String {
        "GraphQL schema is being built dynamically".to_string()
    }
}