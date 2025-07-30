# AEtherBus Stream Key Management

## Overview

`StreamKeyBuilder` is a utility class that provides consistent key generation for Redis streams used in the AEtherBus messaging system. It ensures all components use the same key naming conventions for message routing and RPC patterns.

## Key Structure

All keys follow this general pattern:
```
{prefix}:{namespace}:{entity_type}:{entity_id}:{suffix}
```

### Components:
- `prefix`: Typically `AG1` for AgentGrid 1.0
- `namespace`: Logical grouping (e.g., `agent`, `edge`, `a2a`)
- `entity_type`: Type of the entity (e.g., `inbox`, `outbox`, `rpc`)
- `entity_id`: Identifier for the specific entity
- `suffix`: Additional qualifiers (e.g., `replies`, `stream`)

## Standard Key Types

### 1. Agent Keys

#### Agent Inbox
```python
# Format: AG1:agent:{agent_name}:inbox
StreamKeyBuilder.agent_inbox("muse_agent")
# Returns: "AG1:agent:muse_agent:inbox"
```

#### Agent Outbox
```python
# Format: AG1:agent:{agent_name}:outbox
StreamKeyBuilder.agent_outbox("muse_agent")
# Returns: "AG1:agent:muse_agent:outbox"
```

#### Agent RPC Reply Channel
```python
# Format: AG1:agent:{agent_name}:rpc:{correlation_id}
StreamKeyBuilder.agent_rpc_reply("muse_agent", "req_12345")
# Returns: "AG1:agent:muse_agent:rpc:req_12345"
```

#### User-Agent Conversation Channel
```python
# Format: AG1:agent:{agent_name}:{user_id}:inbox
StreamKeyBuilder.agent_user_inbox("Muse1", "user123")
# Returns: "AG1:agent:Muse1:user123:inbox"
```

### 2. Edge Service Keys

#### Edge Service Inbox
```python
# Format: AG1:edge:{service_name}:{instance}:inbox
StreamKeyBuilder.edge_inbox("mcp", "main")
# Returns: "AG1:edge:mcp:main:inbox"
```

#### Edge Service Status
```python
# Format: AG1:edge:{service_name}:{instance}:status
StreamKeyBuilder.edge_status("mcp", "main")
# Returns: "AG1:edge:mcp:main:status"
```

### 3. A2A (Agent-to-Agent) Keys

#### A2A Registration
```python
# Format: AG1:a2a:registration
StreamKeyBuilder.a2a_registration()
# Returns: "AG1:a2a:registration"
```

#### A2A Response Channel
```python
# Format: AG1:a2a:response:{agent_name}:{task_id}
StreamKeyBuilder.a2a_response("muse_agent", "task_67890")
# Returns: "AG1:a2a:response:muse_agent:task_67890"
```

## Best Practices

1. **Always use StreamKeyBuilder**
   - Never hardcode stream keys
   - Ensures consistency across the codebase

2. **For RPC patterns**:
   ```python
   # Good
   reply_channel = StreamKeyBuilder.agent_rpc_reply(
       self.name, 
       f"{tool_name}_{uuid.uuid4().hex[:8]}"
   )
   
   # Bad
   reply_channel = f"AG1:agent:{self.name}:rpc:{tool_name}_{random_string()}"
   ```

3. **For service discovery**:
   - Use well-known keys from StreamKeyBuilder
   - Document any custom keys in your service's README

4. **Error Handling**:
   - Validate keys when they come from external sources
   - Use the provided validation methods

## Common Patterns

### 1. RPC Request-Response
```python
async def make_rpc_call(agent_name: str, payload: dict):
    redis = get_redis_connection()
    reply_channel = StreamKeyBuilder.agent_rpc_reply(agent_name, str(uuid.uuid4()))
    
    request = Envelope(
        role="rpc_request",
        content=payload,
        reply_to=reply_channel,
        correlation_id=str(uuid.uuid4())
    )
    
    await redis.xadd(
        StreamKeyBuilder.agent_inbox(agent_name),
        {"data": request.json()}
    )
    
    # Wait for response
    response = await wait_for_response(redis, reply_channel)
    return response
```

### 2. Service Registration
```python
async def register_service(service_name: str, instance: str, endpoint: str):
    redis = get_redis_connection()
    reg_channel = StreamKeyBuilder.edge_registration()
    
    registration = {
        "service": service_name,
        "instance": instance,
        "endpoint": endpoint,
        "timestamp": datetime.utcnow().isoformat()
    }
    
    await redis.xadd(reg_channel, {"data": json.dumps(registration)})
```

## Migration Guide

### From Hardcoded to StreamKeyBuilder

**Before**:
```python
inbox = f"AG1:agent:{agent_name}:inbox"
rpc_reply = f"AG1:agent:{agent_name}:rpc:{req_id}"
edge_inbox = "AG1:edge:mcp:main:inbox"
```

**After**:
```python
from AG1_AEtherBus.keys import StreamKeyBuilder as keys

inbox = keys.agent_inbox(agent_name)
rpc_reply = keys.agent_rpc_reply(agent_name, req_id)
edge_inbox = keys.edge_inbox("mcp", "main")
```

## Troubleshooting

### Common Issues

1. **Message Not Delivered**
   - Verify the key matches exactly between sender and receiver
   - Check Redis monitor: `redis-cli monitor | grep -i "your_key"`

2. **Permission Denied**
   - Ensure Redis user has permissions to access the key pattern
   - Check Redis ACLs if using authentication

3. **Key Not Found**
   - Verify the key generation logic
   - Check for typos in service/agent names

## See Also
- `AG1_AEtherBus/keys.py` - Implementation of StreamKeyBuilder
- `AG1_AEtherBus/bus.py` - Core bus implementation
- `AG1_AEtherBus/rpc.py` - RPC patterns and utilities
