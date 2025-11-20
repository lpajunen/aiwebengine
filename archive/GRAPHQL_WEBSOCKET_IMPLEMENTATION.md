# GraphQL WebSocket Support Implementation

## Summary

Added WebSocket support for GraphQL subscriptions at `/graphql/ws` endpoint using the graphql-transport-ws protocol.

## Implementation Details

### 1. Dependencies Added (`Cargo.toml`)

- Enabled `ws` feature for `axum` v0.8.4
- Added `futures-util = "0.3"` for stream utilities

### 2. New Module: `src/graphql_ws.rs`

Implements the complete graphql-transport-ws protocol:

**Features:**

- Full protocol message types (ConnectionInit, ConnectionAck, Ping, Pong, Subscribe, Next, Error, Complete)
- Connection lifecycle management
- Multiple concurrent subscriptions per connection (max 20)
- Automatic keep-alive with ping/pong (30 second interval)
- Graceful error handling and cleanup
- Authentication support via `AuthUser` from request extensions

**Key Components:**

- `ProtocolMessage` - Message serialization/deserialization
- `SubscriptionState` - Tracks active subscriptions per connection
- `handle_websocket_connection()` - Main WebSocket handler

### 3. Router Configuration (`src/lib.rs`)

- Added `graphql_ws()` upgrade handler
- Registered `/graphql/ws` endpoint in both authenticated and unauthenticated routers
- Updated GraphiQL to use WebSocket endpoint for subscriptions

### 4. Example Script

Created `scripts/example_scripts/graphql_ws_demo.js`:

- Demonstrates WebSocket subscription usage
- Includes live demo page at `/ws-demo`
- Shows protocol implementation in browser JavaScript
- Supports both GraphQL mutations and HTTP triggers

## Usage

### GraphiQL

Access GraphiQL at `http://localhost:3000/engine/graphql` - it now uses WebSocket for subscriptions by default.

### Client Example (JavaScript)

```javascript
const ws = new WebSocket(
  "ws://localhost:3000/graphql/ws",
  "graphql-transport-ws",
);

ws.onopen = () => {
  // Initialize connection
  ws.send(
    JSON.stringify({
      type: "connection_init",
      payload: {},
    }),
  );
};

ws.onmessage = (event) => {
  const message = JSON.parse(event.data);

  if (message.type === "connection_ack") {
    // Connection ready, subscribe to a query
    ws.send(
      JSON.stringify({
        id: "sub-1",
        type: "subscribe",
        payload: {
          query: "subscription { liveMessages }",
        },
      }),
    );
  }

  if (message.type === "next") {
    console.log("Subscription data:", message.payload);
  }
};
```

### Demo Pages

- **WebSocket Demo:** `http://localhost:3000/ws-demo`
- **SSE Demo:** `http://localhost:3000/subscription-demo`

Both demos share the same GraphQL subscription backend and can broadcast to each other.

## Protocol Details

### graphql-transport-ws Protocol

Specification: https://github.com/enisdenjo/graphql-ws/blob/master/PROTOCOL.md

**Message Flow:**

1. Client connects to `/graphql/ws`
2. Client sends `connection_init`
3. Server responds with `connection_ack`
4. Client sends `subscribe` with operation
5. Server streams `next` messages with data
6. Server sends `complete` when subscription ends
7. Server sends `ping` every 30 seconds
8. Client responds with `pong`

### Configuration

- **Max Subscriptions:** 20 per connection (configurable in `graphql_ws.rs`)
- **Keep-Alive Interval:** 30 seconds (configurable in `graphql_ws.rs`)
- **Authentication:** Extracted from request extensions (supports JWT/session tokens)

## Backward Compatibility

The existing SSE endpoint at `/graphql/sse` remains fully functional. Applications can use either:

- **WebSocket** (`/graphql/ws`) - Better for bidirectional communication, mobile apps, and multiple subscriptions
- **SSE** (`/graphql/sse`) - Simpler for server-to-client streaming, works with standard HTTP

## Testing

Build and run:

```bash
cargo build
cargo run --bin server
```

Access demo:

1. Visit `http://localhost:3000/ws-demo`
2. Open browser console to see protocol messages
3. Send messages and watch real-time updates
4. Open multiple tabs to test multi-client broadcasting

## Files Changed

- `Cargo.toml` - Added dependencies
- `src/lib.rs` - Added module declaration, WebSocket handler, and route registration
- `src/graphql_ws.rs` - New protocol implementation (426 lines)
- `scripts/example_scripts/graphql_ws_demo.js` - Example script with demo page
- `scripts/example_scripts/README.md` - Updated documentation

## Future Enhancements

Potential improvements:

- Connection metrics and monitoring
- Configurable limits via config file
- Subscription filtering/authorization hooks
- Compression support (permessage-deflate)
- Rate limiting per connection
