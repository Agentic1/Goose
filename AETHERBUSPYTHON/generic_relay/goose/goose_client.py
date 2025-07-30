 # Filename: generic_relay.py
# A simple, generic WebSocket relay for connecting any web frontend
# to any agent on the AEtherBus using a standardized UI directive format.

import asyncio
import json
import os
import uuid
from pathlib import Path
import websockets
import time
import sys

from aiohttp import web, WSMsgType
# goose_client.py
import os, json, httpx, asyncio

GOOSE_URL = os.getenv("GOOSE_URL", "http://127.0.0.1:8800/orchestrate")
WS_URL = os.getenv("GOOSE_WS_URL", "ws://127.0.0.1:8800/ws")

async def stream_goose1(prompt: dict, cid: str):
    """Yield Goose deltas via the /ws channel."""
    async with websockets.connect(WS_URL) as sock:
        print("[goose_client] WS connected to", WS_URL)  
        # browser UI expects a wrapper {"context":{…}}; follow that
        start_msg = {"context": prompt, "cid": cid}
        await sock.send(json.dumps(start_msg))
        print("[goose_client] sent", start_msg)

        async for raw in sock:
            print("[goose_client] recv", raw) 
            yield json.loads(raw)

async def stream_goose(prompt_text: str, sid: str | None = None):
    session_id = sid or uuid.uuid4().hex
    start_msg = {
        "type": "message",
        "content": prompt_text,
        "session_id": session_id,
        "timestamp": int(time.time() * 1000)
    }

    async with websockets.connect(WS_URL) as sock:
        await sock.send(json.dumps(start_msg))

        async for raw in sock:
            msg = json.loads(raw)
            yield msg
            if msg.get("type") == "complete":
                break

# --- Standard AEtherBus Imports ---
PROJECT_ROOT = Path(__file__).resolve().parent.parent.parent.parent
sys.path.insert(0, str(PROJECT_ROOT))

from AG1_AEtherBus.envelope import Envelope
from AG1_AEtherBus.bus import publish_envelope, build_redis_url, subscribe
from AG1_AEtherBus.keys import StreamKeyBuilder
from redis.asyncio import Redis

# --- Configuration ---
RELAY_CHANNEL_NAME = os.getenv("RELAY_CHANNEL_NAME", "generic_relay_2")
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
    print(f"[handle agent message] → Goose ")
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

async def listen_for_agent_replies(
    user_id: str,
    ws_connection: web.WebSocketResponse,
    redis_client: Redis
):
    """Listens on the user's response stream and processes agent replies."""
    print(f"[listen rplies] → Goose ")
    response_stream = KEYS.edge_response(RELAY_CHANNEL_NAME, user_id)
    print(f"[{user_id}] LISTENING for agent replies on: {response_stream}")

    await subscribe(redis_client, response_stream, lambda env: handle_agent_message(ws_connection, env))


async def websocket_handler(request: web.Request):
    """Handles a new WebSocket connection from a user's browser."""
    print(f"[] → Goose ")
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
                        session_id = correlation_id  
                        # --- NEW: send straight to Goose and stream deltas back to the browser ---
                        try:
                            
                            async for goose_msg in stream_goose(payload["text"], session_id):
                                msg_type = goose_msg.get("type")

                                if msg_type == "response":
                                    await safe_send_json({
                                        "directive_type": "APPEND_TO_CHAT",
                                        "payload": {
                                            "source": target_agent_name,
                                            "text": goose_msg["content"]
                                        }
                                    })

                                elif msg_type == "complete":
                                    # Optionally show a typing‑done indicator or nothing
                                    break

                        except Exception as e:
                            print(f"[{user_id}] Goose stream error: {e}")
                            await safe_send_json({
                                "directive_type": "ERROR",
                                "payload": { "error": "Goose processing failed" }
                            })
                        
                        """envelope = Envelope(
                            role="user",
                            user_id=user_id,
                            content=payload,
                            reply_to=KEYS.edge_response(RELAY_CHANNEL_NAME, user_id),
                            agent_name=f"user_via_{RELAY_CHANNEL_NAME}",
                            correlation_id=correlation_id
                        )
                        await publish_envelope(redis_client, target_agent_inbox, envelope)"""
                        
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