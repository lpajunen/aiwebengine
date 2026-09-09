use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::{debug, error};

/// Represents a registered message listener
#[derive(Debug, Clone)]
pub struct MessageListener {
    /// Script URI that registered this listener
    pub script_uri: String,
    /// Handler function name to invoke
    pub handler_name: String,
    /// Timestamp when registered (milliseconds since epoch)
    pub registered_at: u64,
}

impl MessageListener {
    pub fn new(script_uri: String, handler_name: String) -> Self {
        let registered_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        Self {
            script_uri,
            handler_name,
            registered_at,
        }
    }
}

/// Statistics about message dispatch
#[derive(Debug, Clone, Default)]
pub struct DispatchStats {
    pub successful_handlers: usize,
    pub failed_handlers: usize,
    pub total_listeners: usize,
}

/// Internal state for the message dispatcher
#[derive(Debug)]
struct DispatcherState {
    /// Map of message type to list of listeners
    /// Key: message type (e.g., "user.created")
    /// Value: Vec of (script_uri, handler_name)
    listeners: HashMap<String, Vec<MessageListener>>,
    /// What a script's `init()` has registered so far, for the scripts
    /// currently being initialized. Keyed by script URI; see
    /// [`MessageDispatcher::begin_script_registration`].
    staged: HashMap<String, Vec<(String, MessageListener)>>,
}

impl DispatcherState {
    fn new() -> Self {
        Self {
            listeners: HashMap::new(),
            staged: HashMap::new(),
        }
    }

    /// Register a listener for a message type.
    ///
    /// While the registering script has a staging area open, the listener goes
    /// there rather than into the live table: what a re-initializing script
    /// listens to should change once, when its `init()` has finished, and not
    /// one registration at a time.
    fn register_listener(&mut self, message_type: String, listener: MessageListener) {
        debug!(
            "Registering listener for message type '{}': script={}, handler={}",
            message_type, listener.script_uri, listener.handler_name
        );

        if let Some(staged) = self.staged.get_mut(&listener.script_uri) {
            staged.push((message_type, listener));
            return;
        }

        self.listeners
            .entry(message_type)
            .or_default()
            .push(listener);
    }

    /// Open a staging area for `script_uri`, discarding any left over from a
    /// previous pass. The script's live listeners are untouched until
    /// [`DispatcherState::commit_staged`] replaces them.
    fn begin_staged(&mut self, script_uri: &str) {
        self.staged.insert(script_uri.to_string(), Vec::new());
    }

    /// Replace `script_uri`'s live listeners with what it staged, and close the
    /// staging area. Returns how many listeners the script now has.
    fn commit_staged(&mut self, script_uri: &str) -> usize {
        let staged = self.staged.remove(script_uri).unwrap_or_default();
        self.remove_listeners_for_script(script_uri);
        let count = staged.len();
        for (message_type, listener) in staged {
            self.listeners
                .entry(message_type)
                .or_default()
                .push(listener);
        }
        count
    }

    /// Get all listeners for a message type
    fn get_listeners(&self, message_type: &str) -> Option<Vec<MessageListener>> {
        self.listeners.get(message_type).cloned()
    }

    /// Remove all listeners for a specific script URI, staged ones included:
    /// a script that is going away is not one whose in-flight `init()` should
    /// still get to install anything.
    fn remove_listeners_for_script(&mut self, script_uri: &str) -> usize {
        self.staged.remove(script_uri);
        let mut removed_count = 0;

        for listeners in self.listeners.values_mut() {
            let original_len = listeners.len();
            listeners.retain(|listener| listener.script_uri != script_uri);
            removed_count += original_len - listeners.len();
        }

        // Clean up empty message type entries
        self.listeners.retain(|_, listeners| !listeners.is_empty());

        if removed_count > 0 {
            debug!(
                "Removed {} listener(s) for script '{}'",
                removed_count, script_uri
            );
        }

        removed_count
    }

    /// Get statistics about registered listeners
    fn get_stats(&self) -> HashMap<String, usize> {
        let mut stats = HashMap::new();
        stats.insert("total_message_types".to_string(), self.listeners.len());
        stats.insert(
            "total_listeners".to_string(),
            self.listeners.values().map(|v| v.len()).sum(),
        );
        stats
    }

    /// Get all message types with listener counts
    fn get_message_types(&self) -> HashMap<String, usize> {
        self.listeners
            .iter()
            .map(|(k, v)| (k.clone(), v.len()))
            .collect()
    }
}

/// Global message dispatcher for inter-script communication
#[derive(Debug, Clone)]
pub struct MessageDispatcher {
    state: Arc<Mutex<DispatcherState>>,
}

impl MessageDispatcher {
    /// Create a new message dispatcher
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(DispatcherState::new())),
        }
    }

    /// Register a listener for a message type
    ///
    /// # Arguments
    /// * `message_type` - The type of message to listen for (e.g., "user.created")
    /// * `script_uri` - The URI of the script registering the listener
    /// * `handler_name` - The name of the handler function to invoke
    ///
    /// # Returns
    /// `Ok(())` if successful, `Err(String)` on failure
    pub fn register_listener(
        &self,
        message_type: String,
        script_uri: String,
        handler_name: String,
    ) -> Result<(), String> {
        if message_type.is_empty() {
            return Err("Message type cannot be empty".to_string());
        }

        if handler_name.is_empty() {
            return Err("Handler name cannot be empty".to_string());
        }

        let listener = MessageListener::new(script_uri, handler_name);

        let mut state = self.state.lock().map_err(|e| {
            error!("Failed to lock dispatcher state: {}", e);
            format!("Failed to lock dispatcher state: {}", e)
        })?;

        state.register_listener(message_type, listener);
        Ok(())
    }

    /// Get all listeners for a message type
    ///
    /// # Arguments
    /// * `message_type` - The type of message
    ///
    /// # Returns
    /// `Ok(Vec<MessageListener>)` with the listeners (empty if none), `Err(String)` on lock failure
    pub fn get_listeners(&self, message_type: &str) -> Result<Vec<MessageListener>, String> {
        let state = self.state.lock().map_err(|e| {
            error!("Failed to lock dispatcher state: {}", e);
            format!("Failed to lock dispatcher state: {}", e)
        })?;

        Ok(state.get_listeners(message_type).unwrap_or_default())
    }

    /// Start a registration pass for `script_uri`.
    ///
    /// Until [`MessageDispatcher::commit_script_registration`] closes it, the
    /// script's `registerListener` calls are staged instead of applied, so what
    /// the script listens to changes once — when its `init()` has finished
    /// saying what that is — rather than one registration at a time with a gap
    /// in the middle where it listens to nothing.
    ///
    /// Callers hold [`crate::script_init`]'s per-script gate, so a pass cannot
    /// open over another script's open one.
    pub fn begin_script_registration(&self, script_uri: &str) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|e| {
            error!("Failed to lock dispatcher state: {}", e);
            format!("Failed to lock dispatcher state: {}", e)
        })?;

        state.begin_staged(script_uri);
        Ok(())
    }

    /// Close `script_uri`'s registration pass, replacing its listeners with
    /// what the pass staged. Returns how many it now has.
    ///
    /// Safe to call for a script that never opened a pass: it then registered
    /// nothing, and replacing its listeners with nothing is what the caller
    /// means.
    pub fn commit_script_registration(&self, script_uri: &str) -> Result<usize, String> {
        let mut state = self.state.lock().map_err(|e| {
            error!("Failed to lock dispatcher state: {}", e);
            format!("Failed to lock dispatcher state: {}", e)
        })?;

        Ok(state.commit_staged(script_uri))
    }

    /// Remove all listeners registered by a specific script
    ///
    /// This is useful for cleanup when a script is deleted or reloaded
    ///
    /// # Arguments
    /// * `script_uri` - The URI of the script whose listeners should be removed
    ///
    /// # Returns
    /// Number of listeners removed
    pub fn remove_listeners_for_script(&self, script_uri: &str) -> Result<usize, String> {
        let mut state = self.state.lock().map_err(|e| {
            error!("Failed to lock dispatcher state: {}", e);
            format!("Failed to lock dispatcher state: {}", e)
        })?;

        Ok(state.remove_listeners_for_script(script_uri))
    }

    /// Get statistics about the dispatcher
    pub fn get_stats(&self) -> Result<HashMap<String, usize>, String> {
        let state = self.state.lock().map_err(|e| {
            error!("Failed to lock dispatcher state: {}", e);
            format!("Failed to lock dispatcher state: {}", e)
        })?;

        Ok(state.get_stats())
    }

    /// Get all message types and their listener counts
    pub fn get_message_types(&self) -> Result<HashMap<String, usize>, String> {
        let state = self.state.lock().map_err(|e| {
            error!("Failed to lock dispatcher state: {}", e);
            format!("Failed to lock dispatcher state: {}", e)
        })?;

        Ok(state.get_message_types())
    }

    /// Clear all listeners (useful for testing)
    #[cfg(test)]
    pub fn clear(&self) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|e| {
            error!("Failed to lock dispatcher state: {}", e);
            format!("Failed to lock dispatcher state: {}", e)
        })?;

        state.listeners.clear();
        Ok(())
    }
}

impl Default for MessageDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

// Global instance for the message dispatcher
lazy_static::lazy_static! {
    /// Global message dispatcher instance
    pub static ref GLOBAL_DISPATCHER: MessageDispatcher = MessageDispatcher::new();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_listener_creation() {
        let listener =
            MessageListener::new("test_script.js".to_string(), "handleMessage".to_string());
        assert_eq!(listener.script_uri, "test_script.js");
        assert_eq!(listener.handler_name, "handleMessage");
        assert!(listener.registered_at > 0);
    }

    #[test]
    fn test_dispatcher_register_listener() {
        let dispatcher = MessageDispatcher::new();
        let result = dispatcher.register_listener(
            "test.event".to_string(),
            "test_script.js".to_string(),
            "handleTest".to_string(),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_dispatcher_register_listener_validation() {
        let dispatcher = MessageDispatcher::new();

        // Empty message type
        let result = dispatcher.register_listener(
            "".to_string(),
            "test_script.js".to_string(),
            "handleTest".to_string(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Message type cannot be empty"));

        // Empty handler name
        let result = dispatcher.register_listener(
            "test.event".to_string(),
            "test_script.js".to_string(),
            "".to_string(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Handler name cannot be empty"));
    }

    #[test]
    fn test_dispatcher_get_listeners() {
        let dispatcher = MessageDispatcher::new();

        // Register listeners
        dispatcher
            .register_listener(
                "user.created".to_string(),
                "user_script.js".to_string(),
                "onUserCreated".to_string(),
            )
            .unwrap();

        dispatcher
            .register_listener(
                "user.created".to_string(),
                "notification_script.js".to_string(),
                "notifyUserCreated".to_string(),
            )
            .unwrap();

        // Get listeners
        let listeners = dispatcher.get_listeners("user.created").unwrap();
        assert_eq!(listeners.len(), 2);

        // Get listeners for non-existent type
        let listeners = dispatcher.get_listeners("nonexistent").unwrap();
        assert_eq!(listeners.len(), 0);
    }

    /// A registration pass leaves the script listening to what the pass
    /// registered, and to nothing it registered last time.
    #[test]
    fn a_registration_pass_replaces_what_the_script_listens_to() {
        let dispatcher = MessageDispatcher::new();

        dispatcher
            .begin_script_registration("script.js")
            .expect("first pass should open");
        dispatcher
            .register_listener(
                "event1".to_string(),
                "script.js".to_string(),
                "handler1".to_string(),
            )
            .unwrap();
        assert_eq!(
            dispatcher.commit_script_registration("script.js").unwrap(),
            1
        );
        assert_eq!(dispatcher.get_listeners("event1").unwrap().len(), 1);

        // A second pass registering elsewhere leaves one listener, not two, and
        // the message type the script no longer names is not still its.
        dispatcher
            .begin_script_registration("script.js")
            .expect("second pass should open");
        dispatcher
            .register_listener(
                "event2".to_string(),
                "script.js".to_string(),
                "handler2".to_string(),
            )
            .unwrap();
        assert_eq!(
            dispatcher.commit_script_registration("script.js").unwrap(),
            1
        );
        assert_eq!(dispatcher.get_listeners("event1").unwrap().len(), 0);
        assert_eq!(dispatcher.get_listeners("event2").unwrap().len(), 1);
    }

    /// The point of staging: what a script listens to changes when its pass
    /// commits, not while the pass is running. A message dispatched mid-pass
    /// reaches the listener the script had, rather than nothing at all.
    #[test]
    fn a_pass_in_progress_does_not_disturb_what_the_script_already_listens_to() {
        let dispatcher = MessageDispatcher::new();

        dispatcher
            .register_listener(
                "event".to_string(),
                "script.js".to_string(),
                "old".to_string(),
            )
            .unwrap();

        dispatcher.begin_script_registration("script.js").unwrap();
        let mid_pass = dispatcher.get_listeners("event").unwrap();
        assert_eq!(mid_pass.len(), 1, "still listening while the pass runs");
        assert_eq!(mid_pass[0].handler_name, "old");

        dispatcher
            .register_listener(
                "event".to_string(),
                "script.js".to_string(),
                "new".to_string(),
            )
            .unwrap();
        let mid_pass = dispatcher.get_listeners("event").unwrap();
        assert_eq!(mid_pass.len(), 1, "and only once, not once per pass");
        assert_eq!(mid_pass[0].handler_name, "old");

        dispatcher.commit_script_registration("script.js").unwrap();
        let after = dispatcher.get_listeners("event").unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].handler_name, "new");
    }

    /// A pass that registered nothing — a script whose `init()` was removed, or
    /// one that failed before its first `registerListener` — leaves the script
    /// listening to nothing.
    #[test]
    fn a_pass_that_registers_nothing_clears_what_was_there() {
        let dispatcher = MessageDispatcher::new();

        dispatcher
            .register_listener(
                "event".to_string(),
                "script.js".to_string(),
                "handler".to_string(),
            )
            .unwrap();

        dispatcher.begin_script_registration("script.js").unwrap();
        assert_eq!(
            dispatcher.commit_script_registration("script.js").unwrap(),
            0
        );
        assert_eq!(dispatcher.get_listeners("event").unwrap().len(), 0);
    }

    /// A script that goes away mid-pass does not get to install what the pass
    /// staged: the deletion is the later word.
    #[test]
    fn removing_a_scripts_listeners_discards_a_pass_in_progress() {
        let dispatcher = MessageDispatcher::new();

        dispatcher.begin_script_registration("script.js").unwrap();
        dispatcher
            .register_listener(
                "event".to_string(),
                "script.js".to_string(),
                "handler".to_string(),
            )
            .unwrap();
        dispatcher.remove_listeners_for_script("script.js").unwrap();
        assert_eq!(
            dispatcher.commit_script_registration("script.js").unwrap(),
            0
        );
        assert_eq!(dispatcher.get_listeners("event").unwrap().len(), 0);
    }

    #[test]
    fn test_dispatcher_remove_listeners_for_script() {
        let dispatcher = MessageDispatcher::new();

        // Register multiple listeners for different message types
        dispatcher
            .register_listener(
                "event1".to_string(),
                "script1.js".to_string(),
                "handler1".to_string(),
            )
            .unwrap();

        dispatcher
            .register_listener(
                "event2".to_string(),
                "script1.js".to_string(),
                "handler2".to_string(),
            )
            .unwrap();

        dispatcher
            .register_listener(
                "event1".to_string(),
                "script2.js".to_string(),
                "handler3".to_string(),
            )
            .unwrap();

        // Remove listeners for script1
        let removed = dispatcher
            .remove_listeners_for_script("script1.js")
            .unwrap();
        assert_eq!(removed, 2);

        // Verify script1 listeners are gone
        let listeners = dispatcher.get_listeners("event1").unwrap();
        assert_eq!(listeners.len(), 1);
        assert_eq!(listeners[0].script_uri, "script2.js");

        let listeners = dispatcher.get_listeners("event2").unwrap();
        assert_eq!(listeners.len(), 0);
    }

    #[test]
    fn test_dispatcher_get_stats() {
        let dispatcher = MessageDispatcher::new();

        dispatcher
            .register_listener(
                "event1".to_string(),
                "script1.js".to_string(),
                "handler1".to_string(),
            )
            .unwrap();

        dispatcher
            .register_listener(
                "event1".to_string(),
                "script2.js".to_string(),
                "handler2".to_string(),
            )
            .unwrap();

        dispatcher
            .register_listener(
                "event2".to_string(),
                "script3.js".to_string(),
                "handler3".to_string(),
            )
            .unwrap();

        let stats = dispatcher.get_stats().unwrap();
        assert_eq!(*stats.get("total_message_types").unwrap(), 2);
        assert_eq!(*stats.get("total_listeners").unwrap(), 3);
    }

    #[test]
    fn test_dispatcher_get_message_types() {
        let dispatcher = MessageDispatcher::new();

        dispatcher
            .register_listener(
                "event1".to_string(),
                "script1.js".to_string(),
                "handler1".to_string(),
            )
            .unwrap();

        dispatcher
            .register_listener(
                "event1".to_string(),
                "script2.js".to_string(),
                "handler2".to_string(),
            )
            .unwrap();

        dispatcher
            .register_listener(
                "event2".to_string(),
                "script3.js".to_string(),
                "handler3".to_string(),
            )
            .unwrap();

        let message_types = dispatcher.get_message_types().unwrap();
        assert_eq!(message_types.len(), 2);
        assert_eq!(*message_types.get("event1").unwrap(), 2);
        assert_eq!(*message_types.get("event2").unwrap(), 1);
    }

    #[test]
    fn test_multiple_handlers_same_script() {
        let dispatcher = MessageDispatcher::new();

        // Same script can register multiple handlers for same message type
        dispatcher
            .register_listener(
                "event1".to_string(),
                "script1.js".to_string(),
                "handler1".to_string(),
            )
            .unwrap();

        dispatcher
            .register_listener(
                "event1".to_string(),
                "script1.js".to_string(),
                "handler2".to_string(),
            )
            .unwrap();

        let listeners = dispatcher.get_listeners("event1").unwrap();
        assert_eq!(listeners.len(), 2);
        assert_eq!(listeners[0].script_uri, "script1.js");
        assert_eq!(listeners[1].script_uri, "script1.js");
        assert_eq!(listeners[0].handler_name, "handler1");
        assert_eq!(listeners[1].handler_name, "handler2");
    }
}
