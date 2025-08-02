# Thinking Changes and Revert Notes

## Changes Made (2025-08-01)
1. **Message Role Filtering**
   - Removed role-based filtering in `web.rs` to allow all message types (including "assistant" role)
   - Old code was filtering to only process messages with role "user" or "agent"
   - This change was made to ensure thinking messages are properly relayed

2. **Bus Handling**
   - Replaced direct `bus` usage with `Arc<Bus>` (`bus_arc`) to resolve ownership issues
   - Updated all bus send calls to use the Arc reference
   - Added proper error handling for bus operations

## Issues Encountered
1. **MCP Client Timeout**
   - Seeing timeouts in MCP client communication
   - Error: `Failed to read response: request or response body error: operation timed out`
   - Occurs in `mcp_client::transport::streamable_http`

2. **Message Processing**
   - Some messages might be getting lost or not properly acknowledged
   - Need to verify consumer group handling in Redis streams

## Revert Plan
1. **Immediate Reverts**
   - [ ] Restore role-based filtering in `web.rs`
     - Uncomment the role check code block
     - Ensure it properly handles all required message types
   - [ ] Review bus message handling
     - Verify proper error handling for all bus operations
     - Ensure proper message acknowledgment

2. **Testing**
   - [ ] Test with different message types
   - [ ] Verify message delivery and processing
   - [ ] Check Redis stream consumer group status

3. **Next Steps**
   - Investigate MCP client timeout issues
   - Review Redis stream configuration and consumer groups
   - Add more detailed logging for message flow debugging
