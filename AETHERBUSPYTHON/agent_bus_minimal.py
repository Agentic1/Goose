from AG1_AEtherBus.bus import ensure_group, subscribe, build_redis_url, publish_envelope
from AG1_AEtherBus.keys import StreamKeyBuilder
from AG1_AEtherBus.envelope import Envelope
from redis.asyncio import Redis
import redis.asyncio as aioredis
from redis.exceptions import ConnectionError as RedisBaseConnectionError
from redis.asyncio import Redis as AsyncRedis

import asyncio
import datetime

current_subscriptions = set()

def extract_envelope_fields(env):
    """
    Returns all fields of the envelope as a dict, supporting both objects and dicts.
    """
    if hasattr(env, "__dict__"):
        return dict(env.__dict__)
    elif isinstance(env, dict):
        return dict(env)
    else:
        # Fallback: try to extract common attributes
        return {attr: getattr(env, attr, None) for attr in dir(env) if not attr.startswith("__")}

def get_patterns(agent_name):
    """Return Redis stream patterns for the agent inbox."""
    kb = StreamKeyBuilder()
    return [kb.agent_inbox(agent_name)]

async def get_redis():
    """Return a Redis connection using the configured URL."""
    return Redis.from_url(build_redis_url())

async def register_with_tg_handler(config, redis):
    """
    Send a registration envelope to the TG handler.

    TODO: Generalize registration to support multiple edge types (not just Telegram).
    - In the future, config should have an 'edges' or 'channels' list, e.g.
      {
        "agent_name": "Muse1",
        "edges": [
          {"type": "telegram", "handle": "@AG1_muse_bot", "key": "..."},
          {"type": "nostr", "pubkey": "npub1..."},
          {"type": "matrix", "id": "@muse:matrix.org", "token": "..."}
        ]
      }
    - The registration envelope should include the relevant edge config in 'content'.
    - Each edge handler should process only registrations for its type.
    - This will make the agent platform modular and extensible for new protocols.
    """
    envelope = Envelope(
        role="agent",
        envelope_type="register",
        agent_name=config["agent_name"],
        content={
            "tg_handle": config.get("tg_handle"),
            "key": config.get("key")
        },
        timestamp=datetime.datetime.utcnow().isoformat() + "Z"
    )
    channel = "AG1:tg:register"
    await publish_envelope(redis, channel, envelope)
    print(f"[AG1_AEtherBus][REGISTER] Sent registration envelope for {config['agent_name']} to {channel}")


async def register_with_a2a_handler(config: dict, redis: Redis) -> None:
    """
    Register an agent with the A2A edge handler.
    
    Args:
        config: Dictionary containing agent configuration with:
            - agent_name: Name of the agent
            - a2a_endpoint: Endpoint URL for A2A communication
            - auth_type: Type of authentication (e.g., 'bearer', 'apikey')
            - auth_key: Authentication key or token
        redis: Redis client instance
        
    Raises:
        KeyError: If required config keys are missing
        RedisError: If registration fails
    """
    keys = StreamKeyBuilder()
    envelope = Envelope(
        role="agent",
        envelope_type="register",
        agent_name=config["agent_name"],
        content={
            "a2a_endpoint": config.get("a2a_endpoint"),
            "auth_type": config.get("auth_type"),
            "auth_key": config.get("auth_key")
        },
        timestamp=datetime.datetime.utcnow().isoformat() + "Z"
    )
    
    try:
        await publish_envelope(redis, keys.a2a_register(), envelope)
        print(f"[{config['agent_name']}] Successfully registered with A2A edge handler")
    except Exception as e:
        print(f"[{config['agent_name']}] Failed to register with A2A handler: {str(e)}")
        raise

async def subscribe_agent_bus(config, handle_bus_envelope):
    """
    Subscribe the agent to its inbox stream and register with TG handler.
    Handles Redis connection, pattern setup, and subscription.
    """
    redis = await get_redis()
    patterns = get_patterns(config["agent_name"])
    group = f"{config['agent_name']}_agent"
    await register_with_tg_handler(config, redis)

    async def handler_with_redis(env):
        await handle_bus_envelope(env, redis)

    await start_bus_subscriptions(
        redis=redis,
        patterns=patterns,
        group=group,
        handler=handler_with_redis
    )
    print("[AG1_AEtherBus][BUS] Subscribed to:", patterns)
    await asyncio.Event().wait()

async def old_before_mcp_discover_and_subscribe(redis, pattern, group, handler, poll_delay=5):
    while True:
        print(f"[Bus_minimal][DISCOVERY] Scanning for: {pattern}")
        cursor = "0"
        while True:
            cursor, keys = await redis.scan(cursor=cursor, match=pattern)
            for key in keys:
                key = key.decode() if isinstance(key, bytes) else key
                if key not in current_subscriptions:
                    print(f"[DISCOVERY] Subscribing to stream: {key}")
                    await ensure_group(redis, key, group)

                    import functools
                        
                    # Create a new handler that has the stream key "baked in"
                    handler_with_key = functools.partial(handler, stream_key=key)


                    asyncio.create_task(subscribe(redis, key, handler, group))
                    current_subscriptions.add(key)
            if cursor == "0":
                break
        await asyncio.sleep(poll_delay)

        
async def discover_and_subscribe(redis, pattern, group, handler, poll_delay=5):
    print(f"[Bus_minimal][DISCOVERY] Starting discovery/subscription task for pattern: {pattern}")
    # Store tasks spawned by *this* discover_and_subscribe instance for specific keys found
    spawned_subscribe_tasks = {} 

    try:
        while True: # Outer loop for periodic scanning
            print(f"[Bus_minimal][DISCOVERY] Scanning for streams matching pattern: {pattern}")
            cursor = "0"
            while True: # Inner loop for iterating through `scan` results
                try:
                    cursor, keys = await redis.scan(cursor=cursor, match=pattern)
                except RedisBaseConnectionError as e:
                    print(f"[Bus_minimal][DISCOVERY] Redis connection error during SCAN for pattern '{pattern}': {e}. Retrying after delay.")
                    await asyncio.sleep(poll_delay * 2) # Longer delay on connection error
                    break # Break inner loop to restart scan after delay
                except asyncio.CancelledError:
                    print(f"[Bus_minimal][DISCOVERY] SCAN for pattern '{pattern}' cancelled during redis.scan.")
                    raise # Re-raise to be caught by outer try/except

                for key_bytes in keys:
                    key = key_bytes.decode() if isinstance(key_bytes, bytes) else key_bytes
                    
                    # current_subscriptions should ideally be instance-specific if BusAdapterV2 manages it
                    # or passed in if it's global and needs careful handling.
                    # For now, assuming it's a global/module-level set as in your original.
                    if key not in current_subscriptions and key not in spawned_subscribe_tasks:
                        print(f"[Bus_minimal][DISCOVERY] New stream found: {key}. Ensuring group and subscribing.")
                        try:
                            await ensure_group(redis, key, group) # Ensure group exists for this specific key
                            # Create and store the subscribe task
                            sub_task = asyncio.create_task(subscribe(redis, key, handler, group, f"{group}_{key.replace(':', '_')}_consumer"))
                            spawned_subscribe_tasks[key] = sub_task
                            current_subscriptions.add(key) # Mark as globally active
                            print(f"[Bus_minimal][DISCOVERY] Subscribed to new stream: {key}")
                        except RedisBaseConnectionError as e:
                            print(f"[Bus_minimal][DISCOVERY] Redis connection error during ensure_group/subscribe for key '{key}': {e}")
                            # Don't add to spawned_subscribe_tasks or current_subscriptions if setup failed
                        except asyncio.CancelledError:
                            print(f"[Bus_minimal][DISCOVERY] Task cancelled during setup for key '{key}'.")
                            raise # Re-raise
                        except Exception as e:
                            print(f"[Bus_minimal][DISCOVERY] Error setting up subscription for key '{key}': {e}")
                            
                
                if cursor == b"0" or cursor == "0": # SCAN returns bytes for cursor '0'
                    break # Finished this scan iteration
                
                await asyncio.sleep(0.01) # Yield control during long scans

            print(f"[Bus_minimal][DISCOVERY] Finished scan for pattern '{pattern}'. Sleeping for {poll_delay}s.")
            await asyncio.sleep(poll_delay) # Wait before next full scan cycle

    except asyncio.CancelledError:
        print(f"[Bus_minimal][DISCOVERY] Main loop for pattern '{pattern}' cancelled.")
        # Propagate cancellation to all subscribe tasks spawned by this discover_and_subscribe instance
        for key, task in spawned_subscribe_tasks.items():
            if not task.done():
                task.cancel()
                print(f"[Bus_minimal][DISCOVERY] Cancelled spawned subscribe task for key '{key}'.")
        # Await all spawned tasks to ensure they handle cancellation
        if spawned_subscribe_tasks:
            await asyncio.gather(*spawned_subscribe_tasks.values(), return_exceptions=True)
        print(f"[Bus_minimal][DISCOVERY] All spawned subscribe tasks for pattern '{pattern}' handled.")
        raise # Re-raise CancelledError so the caller (BusAdapterV2) knows

    finally:
        # Cleanup: remove keys managed by this discover_and_subscribe instance from global set
        # This part is tricky if current_subscriptions is truly global and shared.
        # If BusAdapterV2 manages its own set of active patterns, this cleanup is simpler.
        for key in spawned_subscribe_tasks.keys():
            current_subscriptions.discard(key)
        print(f"[Bus_minimal][DISCOVERY] Exiting task for pattern: {pattern}. Cleaned up its spawned subscriptions.")

async def start_bus_subscriptions(redis, patterns, group, handler):
    await asyncio.gather(*[
        discover_and_subscribe(redis, pattern, group, handler)
        for pattern in patterns
    ])
