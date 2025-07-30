# AG1 Goose Agents Development Documentation

## Redis Streams Consumer Groups Implementation

### Current Limitations
1. **No Consumer Groups**: 
   - Current implementation uses basic XREAD without consumer groups
   - No message acknowledgment or retry mechanism
   - No load balancing between multiple consumers
   - No tracking of pending messages

2. **Scalability Issues**:
   - All consumers see all messages (broadcast pattern)
   - No way to scale processing of a single stream
   - No consumer group rebalancing

### Implementation Plan 

#### 1. Required Code Changes

**bus/src/lib.rs**:
- Add new `recv_block_group` method for consumer group support
- Implement message acknowledgment (XACK)
- Add consumer group management (XGROUP CREATE)
- Add pending message tracking (XPENDING)

**ag1_meta/src/lib.rs**:
- Update `delegate_with_opts` to support consumer groups
- Add consumer group configuration
- Implement proper error handling for group operations

**goose-cli/src/commands/web.rs**:
- Update WebSocket handler to use consumer groups
- Implement proper message acknowledgment
- Add consumer group metrics and monitoring

#### 2. New Methods to Add

```rust
// In bus/src/lib.rs
impl Bus {
    // Create consumer group
    pub async fn create_consumer_group(
        &self,
        stream: &str,
        group: &str,
    ) -> Result<(), BusError> {
        // Implementation using XGROUP CREATE
    }

    // Read from consumer group
    pub async fn read_group(
        &self,
        group: &str,
        consumer: &str,
        stream: &str,
        block_ms: u64,
    ) -> Result<Option<Envelope>, BusError> {
        // Implementation using XREADGROUP
    }

    // Acknowledge message processing
    pub async fn ack_message(
        &self,
        stream: &str,
        group: &str,
        message_id: &str,
    ) -> Result<(), BusError> {
        // Implementation using XACK
    }
}
```

#### 3. Required AetherBus Updates

1. **Envelope Extensions**:
   - Add `consumer_group` field
   - Add `consumer_id` field
   - Add `delivery_count` for retry tracking

2. **New Metadata**:
   - Consumer group configuration
   - Retry policies
   - Dead letter queue configuration

3. **Monitoring**:
   - Consumer lag metrics
   - Pending messages count
   - Consumer group status

## Python AetherBus Implementation

The Python implementation of AetherBus is located in `goose/AETHERBUSPYTHON/` and provides a Python-native way to interact with the Redis-based message bus.

### Key Components

1. **Core Files**:
   - `agent_bus_minimal.py`: Main entry point for agent bus functionality
   - `bus.py`: Core bus implementation
   - `envelope.py`: Envelope class definition
   - `bus_adapterV2.py`: Adapter for different bus implementations
   - `bus_connector.py`: Connection management
   - `bus_monitor.py`: Monitoring and metrics
   - `STREAM_KEYS.md`: Documentation for stream key patterns

2. **Key Features**:
   - Asynchronous Redis client using `redis.asyncio`
   - Support for consumer groups
   - Automatic stream discovery and subscription
   - Edge handler registration (Telegram, A2A, etc.)

### Integration with Rust Components

1. **Message Format Compatibility**:
   - Both implementations use the same envelope structure
   - JSON serialization/deserialization is compatible
   - Field names and types match between implementations

2. **Stream Naming**:
   - Uses consistent key patterns defined in `STREAM_KEYS.md`
   - Follows the same conventions as Rust implementation

3. **Consumer Groups**:
   - Supports the same consumer group model as Rust
   - Can participate in the same consumer groups as Rust clients

### Usage Example

```python
from agent_bus_minimal import subscribe_agent_bus, get_redis

async def handle_message(envelope):
    print(f"Received message: {envelope}")
    # Process message...
    return True  # Return True to ack the message

async def main():
    config = {
        "agent_name": "python_agent",
        "redis_url": "redis://localhost:6379"
    }
    
    redis = await get_redis()
    await subscribe_agent_bus(config, handle_message)

if __name__ == "__main__":
    import asyncio
    asyncio.run(main())
```


### Testing Plan
1. Unit tests for new consumer group functionality
2. Integration tests with multiple consumers
3. Load testing with high message volumes
4. Failure scenario testing (consumer crashes, network issues)

## Message Processing Enhancements

### Message ID Handling

#### Current Implementation
- Uses `0-0` to read all messages from the beginning of the stream
- No distinction between historical and new messages
- Potential performance impact with large message histories

#### Required Changes
- Use `$` to only read new messages by default
- Add configuration option to read from beginning when needed (e.g., on first connection)
- Implement message ID tracking per session

### Session Management

#### Requirements
1. **Session Creation**
   - Generate unique session ID for each WebSocket connection
   - Associate session with user/connection metadata
   - Set session timeout (e.g., 30 minutes of inactivity)

2. **Session Storage**
   - Store session state in Redis with TTL
   - Include:
     - Session ID
     - User/connection metadata
     - Last activity timestamp
     - Message history (optional, with size limit)

3. **Session Cleanup**
   - Implement periodic cleanup of expired sessions
   - Handle graceful session termination
   - Persist important session data if needed

### Thinking Indicator Implementation

#### Message Format
```json
{
  "type": "thinking",
  "content": "Thinking...",
  "timestamp": "2025-07-30T07:45:00Z",
  "session_id": "session-12345"
}
```

#### Implementation Notes
1. **Backend (Rust)**
   - Add `type` field to message content
   - Set `type: "thinking"` for thinking indicators
   - Include session ID for proper routing
   - Use existing WebSocket and bus infrastructure

2. **Frontend (JavaScript/HTML)**
   - Check for `type` field in incoming messages
   - Show/hide thinking indicator based on message type
   - Style thinking messages differently from regular content

3. **Relay (Python)**
   - Forward messages with `type` field intact
   - No special handling needed for thinking messages
   - Ensure message envelope preserves all fields

## Critical Issue: Null Content in Agent Communication

### Problem Description
Messages sent from the Goose WebSocket interface to the social agent are being received with `null` content, causing downstream processing failures. This issue is specific to the WebSocket interface, as the `ag1goose-bridge` implementation appears to work correctly.

### Affected Components
1. **Goose Web Interface** (`goose-cli/src/commands/web.rs`)
   - Handles WebSocket connections and message forwarding
   - Uses `process_message_streaming` to handle incoming messages

2. **Agent Communication Layer** (`ag1_meta/src/lib.rs`)
   - Manages message delegation between agents
   - Contains `delegate_with_opts` where content handling occurs

3. **MCP Server** (`ag1_mcp_server`)
   - Receives and routes messages between agents
   - Uses Redis streams for communication

### Root Cause Analysis
1. **Incorrect Message Routing in WebSocket Handler**:
   - The WebSocket interface is using `send_user` which creates envelopes with `target: null` and a fixed `reply_to: "AG1:agent:GooseAgent:inbox"`
   - This prevents proper routing of messages to the intended agent
   - Messages are only delivered to the GooseAgent's inbox instead of being forwarded to the target agent

2. **Working Implementation (ag1goose-bridge)**:
   - Uses `delegate_with_opts` which properly sets:
     - `target: Some(target.into())`
     - `reply_to: Some(in_stream.to_string())`
   - This ensures proper routing to the intended agent

3. **Required Fix**:
   - WebSocket handler needs to use `delegate` or `delegate_to_name` instead of `send_user`
   - This will ensure proper message routing to the target agent

4. **Content Propagation Path**:
   - WebSocket message → `process_message_streaming` → `agent.reply()` → MCP Server → `delegate_with_opts`
   - Content is preserved when proper routing is used

5. **Key Observations`:
   - The `agent_name` field is correctly set to `"ag1goose"` in outbound messages
   - The issue only occurs with the WebSocket interface, not with the bridge
   - Debug logs show the content is present in the WebSocket handler but lost before reaching Redis

3. **Suspected Issues**:
   - Incorrect message transformation in WebSocket handler
   - Missing content field in the envelope creation
   - Serialization/deserialization mismatch between components

### Agent Registry Configuration
From `orchestrator_registry.json`:
- **Social Agent**: Handles social interactions and content processing
- **Goose Agent**: Main agent handling WebSocket connections
- **AG1 MCP**: Message Control Protocol server for inter-agent communication

## Known Issues
PYTHON example : pythonexample.md
This shows wht json format and expected communications structure.
NB NB


### Message Format Issues
1. **Inbox Naming Convention**
   - All agent inboxes must follow the pattern: `AG1:agent:<AgentName>:inbox`
   - Example: `AG1:agent:SocialAgent:inbox`
   - The WebSocket handler was incorrectly using a hardcoded `AG1:agent:GooseAgent:inbox`
   - **Required Change**: Update all inbox references to use the proper agent name from the configuration

2. **Content Structure Mismatch"
   - Social agent expects `content` to be a JSON object with a `text` field
   - Current implementation sends content as a string instead of an object
   - Error: `'str' object has no attribute 'get'`

2. **JSONL Log Parsing**
   - Partial JSON lines in logs causing parsing errors
   - Need robust handling of incomplete JSON objects
   - MCP client warnings in logs interfering with parsing

3. **Session Management**
   - Session cleanup not properly implemented
   - Unused `is_ready` field in session struct
   - `wait_reply_raw` method not being used

### Error Handling
1. **Message Processing**
   - Incomplete error handling in message processing
   - Need better error propagation between agents
   - Missing retry mechanisms for failed operations

2. **Timeout Handling**
   - Inconsistent timeout behavior across agents
   - Need standardized timeout values
   - Missing timeout recovery strategies

## Wish List / Future Tasks

### Core Improvements
1. **Message Format Standardization**
   - Enforce strict envelope format validation
   - Add comprehensive message schema validation
   - Implement proper type checking for content fields

2. **Error Handling Framework**
   - Standardized error codes and messages
   - Centralized error handling system
   - Better error logging and reporting

3. **Session Management**
   - Implement proper session lifecycle management
   - Add session state tracking
   - Improve session cleanup procedures

### Agent-Specific Tasks
1. **Social Agent**
   - Fix content format handling
   - Add proper message validation
   - Implement better error recovery

2. **Heartbeat Agent**
   - Add proper message format validation
   - Implement health check reporting
   - Add proper error handling

### Infrastructure Improvements
1. **Logging**
   - Standardize logging format
   - Add structured logging
   - Improve debug logging

2. **Testing**
   - Add comprehensive unit tests
   - Implement integration tests
   - Add message format validation tests

3. **Documentation**
   - Add detailed API documentation
   - Document message formats
   - Add developer guides

### Performance Optimizations
1. **Message Processing**
   - Optimize JSON parsing
   - Improve message buffering
   - Add message batching

2. **Resource Management**
   - Implement proper resource cleanup
   - Add memory usage monitoring
   - Optimize session management

## Priority Tasks

1. **High Priority**
   - Fix message format issues in social agent
   - Implement proper error handling
   - Standardize session management

2. **Medium Priority**
   - Add comprehensive logging
   - Implement message validation
   - Add proper testing

3. **Low Priority**
   - Documentation improvements
   - Performance optimizations
   - Cleanup unused code

## Notes
- All changes should maintain backward compatibility where possible
- New features should be thoroughly tested
- Documentation should be updated with any changes
- Error handling should be consistent across all agents

## Next Steps
1. Fix the content format issue in social agent
2. Implement proper message validation
3. Add comprehensive error handling
4. Standardize session management
5. Add proper testing framework
