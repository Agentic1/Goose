 # Filename: generic_relay.py
# A simple, generic WebSocket relay for connecting any web frontend
# to any agent on the AEtherBus using a standardized UI directive format.

import asyncio
import json
import os
import uuid
from pathlib import Path
import sys
import logging
import aiohttp_cors
from aiohttp import web, WSMsgType

from aiohttp import web, WSMsgType

# --- Standard AEtherBus Imports ---
PROJECT_ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(PROJECT_ROOT))

from AG1_AEtherBus.envelope import Envelope
from AG1_AEtherBus.bus import publish_envelope, build_redis_url, subscribe
from AG1_AEtherBus.keys import StreamKeyBuilder
from redis.asyncio import Redis

# --- Configuration ---
RELAY_CHANNEL_NAME = os.getenv("RELAY_CHANNEL_NAME", "generic_relay")
WEBSOCKET_PORT = int(os.getenv("WEBSOCKET_PORT", 4012))

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

    # Standardize the output format.
    # If the agent sends a simple {"text": "..."}, we wrap it.
    if "text" in content and isinstance(content, dict):
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

async def oldlisten_for_agent_replies(
    user_id: str,
    ws_connection: web.WebSocketResponse,
    redis_client: Redis
):
    """Listens on the user's response stream and processes agent replies."""
    response_stream = KEYS.edge_response(RELAY_CHANNEL_NAME, user_id)
    print(f"[{user_id}] LISTENING for agent replies on: {response_stream}")

    await subscribe(redis_client, response_stream, lambda env: handle_agent_message(ws_connection, env))


async def listen_for_agent_replies(
    user_id: str,
    ws_connection: web.WebSocketResponse,
    redis_client: Redis
):
    """Listens on the user's response stream and processes agent replies."""
    response_stream = KEYS.edge_response(RELAY_CHANNEL_NAME, user_id)
    print(f"[{user_id}] Listening for agent replies on: {response_stream}")

    async def message_handler(envelope: Envelope):
        try:
            if not ws_connection.closed:
                await handle_agent_message(ws_connection, envelope)
        except Exception as e:
            print(f"[{user_id}] Error handling agent message: {str(e)}")

    try:
        await subscribe(redis_client, response_stream, message_handler)
    except asyncio.CancelledError:
        print(f"[{user_id}] Stopped listening for agent replies")
    except Exception as e:
        print(f"[{user_id}] Error in message handler: {str(e)}")

async def oldwebsocket_handler(request: web.Request):
    """Handles a new WebSocket connection from a user's browser."""
    target_agent_name = request.match_info.get("agent_name")
    if not target_agent_name:
        return web.Response(status=400, text="Agent name required in URL: /chat/{agent_name}")

    target_agent_inbox = KEYS.agent_inbox(target_agent_name)
    print(f"[RELAY][CONNECT] New connection for agent: '{target_agent_name}'")

    user_id = request.query.get("user_id", f"user_{uuid.uuid4().hex[:8]}")
    ws = web.WebSocketResponse()
    await ws.prepare(request)
    active_websockets[user_id] = ws
    print(f"  -> User '{user_id}' connected. Forwarding messages to: {target_agent_inbox}")

    redis_client: Redis = request.app["redis"]
    listener_task = asyncio.create_task(
        listen_for_agent_replies(user_id, ws, redis_client)
    )

    try:
        async for msg in ws:
            if msg.type == WSMsgType.TEXT:
                try:
                    payload = json.loads(msg.data)
                    envelope = Envelope(
                        role="user",
                        user_id=user_id,
                        content=payload,
                        reply_to=KEYS.edge_response(RELAY_CHANNEL_NAME, user_id),
                        agent_name=f"user_via_{RELAY_CHANNEL_NAME}",
                    )
                    await publish_envelope(redis_client, target_agent_inbox, envelope)
                except Exception as e:
                    print(f"[{user_id}][ERROR] Failed to process message: {e}")
            elif msg.type == WSMsgType.ERROR:
                print(f"[{user_id}] WebSocket closed with exception: {ws.exception()}")

    finally:
        print(f"[{user_id}] DISCONNECTING from agent '{target_agent_name}'.")
        if user_id in active_websockets:
            del active_websockets[user_id]
        listener_task.cancel()
        print(f"  -> Stopped listener and cleaned up resources.")

    return ws

async def websocket_handler(request: web.Request):
    """Handles a new WebSocket connection from a user's browser."""
    ws = web.WebSocketResponse()
    await ws.prepare(request)
    
    user_id = request.query.get("user_id", f"user_{uuid.uuid4().hex[:8]}")
    target_agent_name = request.match_info.get("agent_name", "default_agent")
    
    print(f"[{user_id}] New WebSocket connection for agent: {target_agent_name}")
    
    # Store WebSocket connection
    active_websockets[user_id] = ws
    
    # Set up Redis client
    redis_client = Redis.from_url(build_redis_url())
    
    try:
        # Start listening for agent replies in the background
        agent_replies_task = asyncio.create_task(
            listen_for_agent_replies(user_id, ws, redis_client)
        )
        
        # Handle incoming messages
        async for msg in ws:
            if msg.type == WSMsgType.TEXT:
                try:
                    message = json.loads(msg.data)
                    # Forward message to agent
                    envelope = Envelope(
                        to=target_agent_name,
                        sender=user_id,
                        content=message,
                        msg_type="user_message"
                    )
                    await publish_envelope(envelope, redis_client)
                except json.JSONDecodeError:
                    print(f"[{user_id}] Received invalid JSON: {msg.data}")
                    await ws.send_json({
                        "error": "Invalid JSON",
                        "received": msg.data
                    })
            elif msg.type == WSMsgType.ERROR:
                print(f"[{user_id}] WebSocket error: {ws.exception()}")
                
    except asyncio.CancelledError:
        print(f"[{user_id}] WebSocket connection cancelled")
    except Exception as e:
        print(f"[{user_id}] Error in WebSocket handler: {str(e)}")
    finally:
        # Clean up
        agent_replies_task.cancel()
        await redis_client.aclose()
        if user_id in active_websockets:
            del active_websockets[user_id]
        await ws.close()
        print(f"[{user_id}] WebSocket connection closed")
    
    return ws

async def oldmain():
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

async def main():
    app = web.Application()
    
    # Add CORS middleware
    cors = aiohttp_cors.setup(app, defaults={
        "*": aiohttp_cors.ResourceOptions(
            allow_credentials=True,
            expose_headers="*",
            allow_headers="*",
        )
    })
    
    # Add WebSocket route
    resource = cors.add(app.router.add_resource('/chat/{agent_name}'))
    cors.add(resource.add_route('GET', websocket_handler))
    
    # Configure logging
    logging.basicConfig(level=logging.INFO)
    
    # Start the server
    runner = web.AppRunner(app)
    await runner.setup()
    site = web.TCPSite(runner, '0.0.0.0', WEBSOCKET_PORT)
    
    print(f"Starting WebSocket server on ws://0.0.0.0:{WEBSOCKET_PORT}")
    await site.start()
    
    # Keep the server running
    while True:
        await asyncio.sleep(3600)  # Sleep forever

if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print("\n[RELAY] Server shutting down.")