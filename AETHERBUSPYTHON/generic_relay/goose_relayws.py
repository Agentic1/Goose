 # Filename: generic_relay.py
# A simple, generic WebSocket relay for connecting any web frontend
# to any agent on the AEtherBus using a standardized UI directive format.

import asyncio
import json
import os
import uuid
from pathlib import Path
import sys

from aiohttp import web, WSMsgType

# --- Standard AEtherBus Imports ---
PROJECT_ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(PROJECT_ROOT))

from AG1_AEtherBus.envelope import Envelope
from AG1_AEtherBus.bus import publish_envelope, build_redis_url, subscribe
from AG1_AEtherBus.keys import StreamKeyBuilder
from redis.asyncio import Redis

# --- Configuration ---
RELAY_CHANNEL_NAME = os.getenv("RELAY_CHANNEL_NAME", "goose_relay")
WEBSOCKET_PORT = int(os.getenv("WEBSOCKET_PORT", 4011))

# --- Boilerplate Setup ---
KEYS = StreamKeyBuilder()
active_websockets: dict[str, web.WebSocketResponse] = {}

# --- UI Directive Handler (from aetherdeck_relay_handlerV1.py) ---
# This is the "feedback" mechanism. It translates agent messages into
# simple commands that any frontend can understand.
async def handle_agent_message(ws_connection: web.WebSocketResponse, env: Envelope):
    """
    Processes an envelope from an agent and sends a standardized
    UI directive to the frontend.
    """
    if ws_connection.closed:
        return

    content = env.content or {}
    directive = {}

    # Handle thinking messages
    if content.get("type") == "thinking":
        directive = {
            "directive_type": "THINKING",
            "message": content.get("message", "Thinking..."),
            "is_transient": True
        }
    # Standardize the output format for text messages
    elif "text" in content and isinstance(content, dict):
        directive = {
            "directive_type": "APPEND_TO_CHAT",
            "payload": {
                "source": env.agent_name or "Agent",
                "text": content["text"]
            }
        }
    # If the agent sends a pre-formatted directive, we pass it through.
    elif "directive_type" in content and isinstance(content, dict):
        directive = content
    # Fallback for unknown formats
    else:
        directive = {
            "directive_type": "APPEND_TO_CHAT",
            "payload": {
                "source": "System",
                "text": f"(Received non-standard response: {json.dumps(content)})"
            }
        }
    
    try:
        await ws_connection.send_json(directive)
    except Exception as e:
        print(f"[{env.user_id}][ERROR] Could not forward directive to WebSocket: {e}")

# --- Core Relay Logic ---

async def listen_for_agent_replies(
    user_id: str,
    ws_connection: web.WebSocketResponse,
    redis_client: Redis
):
    """Listens on the user's response stream and processes agent replies."""
    response_stream = KEYS.edge_response(RELAY_CHANNEL_NAME, user_id)
    print(f"[{user_id}] LISTENING for agent replies on: {response_stream}")

    await subscribe(redis_client, response_stream, lambda env: handle_agent_message(ws_connection, env))


async def websocket_handler(request: web.Request):
    """Handles a new WebSocket connection from a user's browser."""
    target_agent_name = request.match_info.get("agent_name")
    if not target_agent_name:
        return web.Response(status=400, text="Agent name required in URL: /chat/{agent_name}")

    target_agent_inbox = KEYS.agent_inbox(target_agent_name)
    print(f"[RELAY][CONNECT] New connection for agent: '{target_agent_name}'")

    user_id = request.query.get("user_id", f"user_{uuid.uuid4().hex[:8]}")
    # Create WebSocket with appropriate timeouts and settings
    ws = web.WebSocketResponse(
        heartbeat=15.0,  # Send ping every 15 seconds
        receive_timeout=30.0,  # Wait up to 30s for a message
        autoping=True,  # Enable automatic pings
        max_msg_size=10*1024*1024  # 10MB max message size
    )
    
    # Set additional headers for better WebSocket handling
    ws.headers.update({
        'Server': 'AG1-WebSocket-Relay/1.0',
        'X-Request-ID': str(uuid.uuid4())
    })
    
    await ws.prepare(request)
    active_websockets[user_id] = ws
    print(f"  -> User '{user_id}' connected. Forwarding messages to: {target_agent_inbox}")

    redis_client: Redis = request.app["redis"]
    listener_task = None
    ping_task = None
    
    async def safe_send_json(data):
        try:
            if not ws.closed:
                await ws.send_json(data)
                return True
        except (ConnectionResetError, RuntimeError) as e:
            print(f"[{user_id}] Send error: {e}")
        return False

    async def handle_agent_message_safe(env: Envelope):
        try:
            await handle_agent_message(ws, env)
        except Exception as e:
            print(f"[{user_id}] Error in message handler: {e}")

    try:
        # Start listener task
        listener_task = asyncio.create_task(
            listen_for_agent_replies(user_id, ws, redis_client)
        )
        
        # Main message loop
        while not ws.closed:
            try:
                msg = await ws.receive(timeout=30.0)  # Add timeout to prevent hanging
                
                if msg.type == WSMsgType.TEXT:
                    try:
                        payload = json.loads(msg.data)
                        
                        # Handle ping/pong internally
                        if payload.get('type') == 'ping':
                            await safe_send_json({'type': 'pong'})
                            continue
                            
                        # Process regular message
                        # Generate a UUID for correlation
                        correlation_id = str(uuid.uuid4())
                        
                        
                        envelope = Envelope(
                            role="user",
                            user_id=user_id,
                            content=payload,
                            reply_to=KEYS.edge_response(RELAY_CHANNEL_NAME, user_id),
                            agent_name=f"user_via_{RELAY_CHANNEL_NAME}",
                            correlation_id=correlation_id,
                            envelope_id=str(uuid.uuid4())
                        )
                        await publish_envelope(redis_client, target_agent_inbox, envelope)
                        
                    except json.JSONDecodeError:
                        print(f"[{user_id}][ERROR] Received invalid JSON: {msg.data}")
                        await safe_send_json({
                            'directive_type': 'ERROR',
                            'payload': {'error': 'Invalid JSON received'}
                        })
                    except Exception as e:
                        print(f"[{user_id}][ERROR] Failed to process message: {e}")
                        
                elif msg.type == WSMsgType.ERROR:
                    error = ws.exception()
                    print(f"[{user_id}] WebSocket error: {error}")
                    break
                    
                elif msg.type in (WSMsgType.CLOSE, WSMsgType.CLOSING, WSMsgType.CLOSED):
                    print(f"[{user_id}] WebSocket connection closing")
                    break
                    
            except asyncio.TimeoutError:
                # No data received within timeout, send ping to keep connection alive
                if not await safe_send_json({'type': 'ping'}):
                    print(f"[{user_id}] Ping failed, connection lost")
                    break
                    
            except asyncio.CancelledError:
                print(f"[{user_id}] WebSocket task cancelled")
                break
                
            except Exception as e:
                print(f"[{user_id}] WebSocket error: {e}")
                break

    finally:
        print(f"[{user_id}] Cleaning up WebSocket connection to agent '{target_agent_name}'")
        
        # Cancel tasks if they exist
        tasks = []
        if listener_task and not listener_task.done():
            listener_task.cancel()
            tasks.append(listener_task)
        if ping_task and not ping_task.done():
            ping_task.cancel()
            tasks.append(ping_task)
        
        # Wait for tasks to complete with timeout
        if tasks:
            done, pending = await asyncio.wait(
                tasks,
                timeout=2.0,
                return_when=asyncio.ALL_COMPLETED
            )
            
            # Force cancel any pending tasks
            for task in pending:
                task.cancel()
        
        # Clean up resources
        if user_id in active_websockets:
            del active_websockets[user_id]
        
        # Close WebSocket if not already closed
        if not ws.closed:
            try:
                await ws.close()
            except Exception as e:
                print(f"[{user_id}] Error during WebSocket close: {e}")
        
        print(f"  -> Cleanup complete for user '{user_id}'")

    return ws


async def main():
    """Sets up and starts the web server."""
    app = web.Application()
    redis_client = Redis.from_url(build_redis_url(), decode_responses=True)
    app["redis"] = redis_client
    app.router.add_get("/chat/{agent_name}", websocket_handler)

    runner = web.AppRunner(app)
    await runner.setup()
    site = web.TCPSite(runner, "0.0.0.0", WEBSOCKET_PORT)
    await site.start()
    
    print(f"🚀 Generic AEtherBus Relay is running on port {WEBSOCKET_PORT}")
    print(f"   Connect your frontend at: ws://<your_server_ip>:{WEBSOCKET_PORT}/chat/<your_agent_name>")
    
    await asyncio.Event().wait()

if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print("\n[RELAY] Server shutting down.")