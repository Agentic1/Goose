import json
import urllib.request
from dotenv import load_dotenv
load_dotenv()

import uuid
import json
from AG1_AEtherBus.envelope import Envelope
import os
from redis.asyncio import Redis
from redis.exceptions import ResponseError
from AG1_AEtherBus.keys import StreamKeyBuilder


import asyncio
import inspect
import traceback 

# --- Configurable Redis connection ---
REDIS_HOST = os.getenv("REDIS_HOST", "forge.agentic1.xyz")
REDIS_PORT = int(os.getenv("REDIS_PORT", 8081))
REDIS_USERNAME = os.getenv("REDIS_USERNAME")
REDIS_PASSWORD = os.getenv("REDIS_PASSWORD")

STREAM_MAXLEN = int(os.getenv("BUS_STREAM_MAXLEN", 10000))  # For dev/demo, tune as needed
ENVELOPE_SIZE_LIMIT = 1024 * 1024  # 128 KB

key_builder = StreamKeyBuilder()

def extract_user_id_from_channel(ch):
    return ch.split(".")[1] if ch.startswith("user.") else "unknown"



# --- Utility: Ensure a Redis Stream Group Exists ---
async def ensure_group(redis, channel: str, group: str):
    try:
        await redis.xgroup_create(name=channel, groupname=group, id='0-0', mkstream=True)
        print(f"[BUS][Group] Created consumer group '{group}' for channel '{channel}'.")
    except ResponseError as e:
        if "BUSYGROUP" in str(e):
            # Group already exists — no issue
            pass
        else:
            raise


# --- Envelope Publisher (with Discovery + Size Guard) ---
async def publish_envelope(redis, channel: str, env: Envelope):
    """Publish an Envelope to a Redis stream."""
    if not channel:
        raise ValueError("stream name required")

    try:
        data = json.dumps(env.to_dict())
    except Exception as err:
        print(f"[BUS][ERROR] envelope serialization failed: {err}")
        raise

    # Enforce payload size limit
    if len(data.encode("utf-8")) > ENVELOPE_SIZE_LIMIT:
        raise ValueError(
            f"Envelope exceeds {ENVELOPE_SIZE_LIMIT} bytes. Offload large payloads to object storage."
        )

    # Check if stream exists *before* publishing
    stream_exists = await redis.exists(channel)

    # Publish the envelope to Redis stream
    await redis.xadd(channel, {"data": data}, maxlen=STREAM_MAXLEN)

    # Trigger discovery if this is a new stream
    if not stream_exists:
        print('[BUS][Publish] no stream')
        discovery_env = Envelope(
            role="user",
            content={"stream": channel},
            user_id=env.user_id or "unknown",
            agent_name="bus_discovery",
            envelope_type="discovery"
        )
        await redis.xadd("user.discovery", {"data": json.dumps(discovery_env.to_dict())})

# --- Core Subscriber (Consumer Group Model) ---

async def subscribe(
    redis,
    channel: str,
    callback,
    group: str = "corebus",
    consumer: str = None,
    block_ms: int = 1000,
    dead_letter_max_retries: int = 3
):
    """
    Subscribes to a Redis Stream using a consumer group.
    Messages are passed to the callback as deserialized Envelope objects.
    Handles retries and acknowledges messages.
    """
    
    await ensure_group(redis, channel, group)
    consumer = consumer or f"{group}-default"
    print(f"[BUS][subscribe]")
    retry_counts = {}

    while True:
        try:
            #print('x')
            results = await redis.xreadgroup(
                group, consumer, streams={channel: '>'}, count=1, block=block_ms
            )
            #print(f"subscribe: results={results}")
            if not results:
                continue
            
            for stream, messages in results:
                #print(f"Received messages from stream: {stream} messages {messages}")
                for msg_id, fields in messages:
                    #print(f"Received messages in stream results id: {msg_id}")
                    
                    #raw = fields.get("data") or fields.get(b"data")
                    raw = fields.get("envelope") or fields.get("data") or fields.get(b"envelope") or fields.get(b"data")
                    print(f"Received messages in stream results: {str(raw)[:90]}")
                    if not raw:
                        continue


                    # --- START DIAGNOSTIC CHANGES ---
                    json_string_to_parse = None
                    if isinstance(raw, bytes):
                        try:
                            # Try decoding as UTF-8 first
                            json_string_to_parse = raw.decode('utf-8').strip()
                            # If successful, check for unexpected null bytes that print() might hide
                            if '\x00' in json_string_to_parse:
                                print(f"[BUS][WARN] Decoded string contains NULL bytes. Original len: {len(raw)}, Decoded len: {len(json_string_to_parse)}")
                                # Optionally, replace or escape null bytes if that's desired,
                                # though this usually indicates a deeper data corruption issue.
                                # json_string_to_parse = json_string_to_parse.replace('\x00', '[NULL_BYTE]')
                        except UnicodeDecodeError as ude:
                            print(f"[BUS][ERROR][Subscribe] UnicodeDecodeError for raw bytes: {ude}")
                            print(f"  Problematic raw bytes (first 100): {raw[:100]}")
                            # Attempt to decode with 'latin-1' or 'replace' to see the content if UTF-8 fails
                            try:
                                fallback_str = raw.decode('latin-1', errors='replace')
                                print(f"  Fallback decoded (latin-1, replace errors) (first 100): {fallback_str[:100]}")
                            except:
                                pass # Fallback decoding also failed
                            # For now, let it fall through so json.loads() fails and we see the problematic string
                            # If it's not valid UTF-8, json.loads(raw) (if raw is bytes) would also fail.
                            # We need to ensure json_string_to_parse is set for the next block if we want to attempt parsing.
                            # If we can't decode, we probably shouldn't try to json.loads it.
                            # Let's make json_string_to_parse the raw bytes representation for logging if decode fails.
                            json_string_to_parse = str(raw) # So it's not None and gets logged by the except block

                    elif isinstance(raw, str): 
                        json_string_to_parse = raw.strip()
                        if '\x00' in json_string_to_parse:
                            print(f"[BUS][WARN] Raw string contains NULL bytes. Len: {len(json_string_to_parse)}")
                            # json_string_to_parse = json_string_to_parse.replace('\x00', '[NULL_BYTE]')
                    else:
                        print(f"[BUS][ERROR][Subscribe] 'raw' data is of unexpected type: {type(raw)}")
                        continue 

                    # This debug print should now be more revealing if there are hidden chars
                    print(f"[BUS][DEBUG] String to parse. Len: {len(json_string_to_parse)}. Data (repr): '{repr(json_string_to_parse[:120])}'...") # Use repr()

                    if json_string_to_parse is None: 
                        print(f"[BUS][WARN] json_string_to_parse is None after processing 'raw'. Skipping.")
                        continue
                    # --- END DIAGNOSTIC CHANGES ---



                    try:
                        payload_dict = json.loads(json_string_to_parse)
                        env = Envelope.from_dict(payload_dict)
                        env.add_hop("bus_subscribe")
                        
                        # --- START: THIS IS THE CORRECT FIX IN THE CORRECT PLACE ---
                        
                        # The `channel` variable here holds the stream name we need.
                        # We will inject it into the envelope's meta dictionary
                        # before passing it to the callback.
                        if env.meta is None:
                            env.meta = {} # Ensure meta is a dict if it's None
                        env.meta['x_stream_key'] = channel

                        # --- END: THE CORRECT FIX ---

                        print(f"In subscribes: {msg_id}")
                        await callback(env) # Now we call the callback with the modified envelope
                        await redis.xack(channel, group, msg_id)
                        if msg_id in retry_counts:
                            del retry_counts[msg_id]

                    except json.JSONDecodeError as e: # Catch specifically JSONDecodeError
                        print(f"[BUS][ERROR][Subscribe] Malformed envelope (JSONDecodeError) on {channel}: {e}")
                        print(f"--- PROBLEMATIC JSON STRING (Len: {len(json_string_to_parse)}) ---")
                        print(json_string_to_parse) # PRINT THE ENTIRE STRING
                        print(f"--- END PROBLEMATIC JSON STRING ---")
                    except Exception as e:
                        print(f"[BUS][ERROR][Subsribe] Malformed envelope on {channel}: {e}")
                        traceback.print_exc()


                        retry_counts[msg_id] = retry_counts.get(msg_id, 0) + 1
                        if retry_counts[msg_id] > dead_letter_max_retries:
                            print(f"[BUS][DEAD] Giving up on {msg_id} after {dead_letter_max_retries} retries.")
                            await redis.xack(channel, group, msg_id)
                            retry_counts.pop(msg_id)

        except Exception as err:
            print(f"[BUS][ERROR] Subscribe error on {channel}: {err}")
            traceback.print_exc()


# Simple non-group subscriber
async def PREDEBUGsubscribe_simple(redis, stream: str, callback, poll_delay=1):
    print(f'subscibe {redis} stream {stream}')
    last_id = "$"  # Only get new messages
    while True:
        try:
            #print('---->>>>x') #xreadgroup
            results = await redis.xread({stream: last_id}, block=poll_delay * 1000, count=10)
            #results = await redis.xreadgroup({stream: last_id}, block=poll_delay * 1000, count=10)
            #print(f"subscribe_simple: results={results} s{stream}")
            for s, messages in results:
                for msg_id, fields in messages:
                    data = fields.get("data")
                    if data:
                        try:
                            env = Envelope.from_dict(json.loads(data))
                            await callback(env)
                            last_id = msg_id
                        except Exception as e:
                            print(f"[BUS][ERROR] Malformed envelope: {e}")
        except Exception as e:
            print(f"[BUS][ERROR] Failed to read from {stream}: {e}")

async def subscribe_simple(redis, stream: str, callback, poll_delay: int = 1, start_id: str = "$"):
    print(f"[BUS][subscribe_simple] ENTERING for stream '{stream}', start_id '{start_id}'")
    last_id = start_id
    loop_count = 0
    while True:
        loop_count += 1
        #print(f"  [BUS][subscribe_simple][{stream}] Loop iteration {loop_count}. last_id: '{last_id}'. Attempting XREAD...")
        try:
            response = await redis.xread(
                streams={stream: last_id},
                count=10,
                block=poll_delay * 1000
            )
            # print(f"  [BUS][subscribe_simple][{stream}] XREAD response: {response}") # Can be very verbose

            if not response:
                # print(f"  [BUS][subscribe_simple][{stream}] XREAD timed out (no new messages). Continuing loop.")
                await asyncio.sleep(0.01) # Tiny sleep to prevent tight loop on continuous timeouts
                continue

            print(f"  [BUS][subscribe_simple][{stream}] XREAD got {len(response[0][1]) if response else 0} message(s).")
            for stream_name_bytes, messages_in_stream in response:
                for message_id_bytes, message_data_dict_bytes in messages_in_stream:
                    current_message_id = message_id_bytes.decode('utf-8')
                    print(f"    [BUS][subscribe_simple][{stream}] Processing message_id: {current_message_id}")
                    last_id = current_message_id # Update last_id for the next XREAD

                    envelope_json_bytes = message_data_dict_bytes.get(b'data')
                    if envelope_json_bytes:
                        try:
                            envelope_json_str = envelope_json_bytes.decode('utf-8')
                            env_dict = json.loads(envelope_json_str)
                            env = Envelope.from_dict(env_dict)
                            print(f"      [BUS][subscribe_simple][{stream}] Calling callback for {current_message_id}...")
                            await callback(env)
                            print(f"      [BUS][subscribe_simple][{stream}] Callback finished for {current_message_id}.")
                        except json.JSONDecodeError as e_json:
                            print(f"      [BUS][subscribe_simple][ERROR][{stream}] JSONDecodeError for msg {current_message_id}: {e_json}")
                            print(f"        Problematic data: {envelope_json_bytes[:200]}")
                        except Exception as e_cb:
                            print(f"      [BUS][subscribe_simple][ERROR][{stream}] Error in callback for msg {current_message_id}: {e_cb}")
                            traceback.print_exc()
                    else:
                        print(f"    [BUS][subscribe_simple][WARN][{stream}] Message {current_message_id} has no 'data' field (b'data'). Fields: {message_data_dict_bytes}")
            
            await asyncio.sleep(0.01) # Slight pause after processing a batch

        except ConnectionError as e_conn:
            print(f"  [BUS][subscribe_simple][ERROR][{stream}] Redis ConnectionError: {e_conn}. Retrying in 5s...")
            await asyncio.sleep(5)
        except Exception as e_outer:
            print(f"  [BUS][subscribe_simple][ERROR][{stream}] Unexpected error in XREAD loop: {e_outer}")
            traceback.print_exc()
            print(f"  [BUS][subscribe_simple][{stream}] Continuing XREAD loop after error and {poll_delay}s sleep.")
            await asyncio.sleep(poll_delay)

# --- Helper: Build Redis URL with optional auth ---
def build_redis_url(access_key: str = None) -> str:
    """Construct the Redis connection URL.

    If ``access_key`` is provided it will be used as the password. This keeps
    backwards compatibility with existing environment variable based auth.
    """
    user = REDIS_USERNAME
    pwd = access_key or REDIS_PASSWORD
    print(f"Build url {user}  {REDIS_HOST} {REDIS_PORT}")
    if user and pwd:
        return f"redis://{user}:{pwd}@{REDIS_HOST}:{REDIS_PORT}"
    elif pwd:
        return f"redis://:{pwd}@{REDIS_HOST}:{REDIS_PORT}"
    else:
        return f"redis://{REDIS_HOST}:{REDIS_PORT}"

async def main():
    print(f'Host : {REDIS_HOST}')
    #redis = await aioredis.from_url(build_redis_url())
    #redis_conn = Redis.from_url(build_redis_url())
    # Example usage:
    # await publish_envelope(redis, "my_channel", Envelope())
    # await subscribe(redis, "my_channel", my_callback)
    #await redis_conn.aclose()

if __name__ == "__main__":
    asyncio.run(main())
