import asyncio
import time
from typing import Callable, Dict, List, Any
import uuid
from AG1_AEtherBus.bus import publish_envelope, subscribe  # low-level xadd helper and direct subscriber
from AG1_AEtherBus.envelope import Envelope
from AG1_AEtherBus.blob_offloader import BlobStorageOffloader
from AG1_AEtherBus.keys import StreamKeyBuilder
from AG1_AEtherBus.agent_bus_minimal import start_bus_subscriptions
import json
# Redis specific imports
from redis.asyncio import Redis as AsyncRedis  # For type hinting and explicit async Redis client
from redis.exceptions import ConnectionError as RedisConnectionError  # For specific exception handling

class BusAdapterV2:
    """Unified adapter around the minimal AgentBus helpers.

    ``request_response`` was removed in favor of the asynchronous conversation
    pattern described in ``AGENTS.md``. Agents should manage their own
    ``asyncio.Future`` objects to await replies on their static inboxes.
    """
    def __init__(
        self,
        agent_id: str,
        core_handler: Callable[[Envelope, AsyncRedis], None],
        redis_client: AsyncRedis,
        patterns: List[str] = None,
        group: str = None,
        registration_profile: Dict[str, Any] = None,
        offload_threshold: int = 1024 * 1024,
        blob_offloader: BlobStorageOffloader | None = None,
    ):
        self.agent_id = agent_id
        self.core     = core_handler
        self.redis : AsyncRedis    = redis_client
        self.group    = group or agent_id
        self.patterns = patterns or []
        # pattern -> handler mapping
        self._registry: Dict[str, Callable] = {}
        self._running_subscription_tasks: Dict[str, asyncio.Task] = {}
        self.registration_profile = registration_profile
        self.offloader = blob_offloader or BlobStorageOffloader()
        self.offload_threshold = offload_threshold

    async def start(self):
        """
        Subscribe to all statically configured patterns and perform optional
        registration.
        """
        print(f"[BusAdapterV2] Starting adapter for '{self.agent_id}'")
        for pattern in self.patterns:
            await self._subscribe_pattern(pattern, self.core)

        # Automatic registration with Orchestrator if a profile is provided
        if self.registration_profile:
            print(f"[{self.agent_id}] Registration profile found. Registering with Orchestrator...")
            try:
                kb = StreamKeyBuilder()
                orchestrator_inbox = kb.agent_inbox("Orchestrator")

                auth_key = self.registration_profile.pop("auth_key", None)
                if not auth_key:
                    print(f"[{self.agent_id}][WARN] Registration profile provided but missing 'auth_key'.")
                    return

                registration_envelope = Envelope(
                    role="agent",
                    agent_name=self.agent_id,
                    envelope_type="register",
                    content={"action": "register_agent", "profile": self.registration_profile},
                    meta={"auth_key": auth_key},
                )
                await self.publish(orchestrator_inbox, registration_envelope)
                print(f"[{self.agent_id}] Registration envelope sent to {orchestrator_inbox}.")
            except Exception as e:
                print(f"[{self.agent_id}][ERROR] Automatic registration failed: {e}")

    async def _subscribe_pattern(
        self,
        pattern: str,
        handler: Callable[[Envelope, any], None]
    ):
        """
        Internal: register handler and spawn a background subscribe loop.
        """
        # record handler
        self._registry[pattern] = handler
        print(f"[BusAdapterV2] Subscribing to pattern '{pattern}'")

        # build a callback that normalizes data to Envelope
        async def callback(raw):
            print(f"[BusAdapterV2][{self.agent_id}][CALLBACK_RAW_INPUT] Type: {type(raw)}, Data: {str(raw)[:200]}")
            if isinstance(raw, Envelope):
                env = raw
            else:
                env = Envelope.from_dict(raw)

            sender_id = env.headers.get("x-aetherbus-sender-id")
            if sender_id == self.agent_id:
                return

            if env.is_offloaded and env.content_ref:
                try:
                    env.content = await self.offloader.download_content(env.content_ref)
                    env.is_offloaded = False
                    print(f"Retrieved offloaded payload from Azure: {env.content_ref.blob_name}")
                except Exception as e:
                    print(f"Payload retrieval failed: {e}")

            try:
                await handler(env, self.redis)
            except TypeError:
                await handler(env, self.redis)

        # start the subscribe loop (never returns)
        task = asyncio.create_task(
            start_bus_subscriptions(
                redis=self.redis,
                patterns=[pattern],
                group=self.group,
                handler=callback
            )
        )
        self._running_subscription_tasks[pattern] = task
        print(f"[BusAdapterV2] Subscription task created for '{pattern}'")

    async def _subscribe_stream(
        self,
        stream: str,
        handler: Callable[[Envelope, any], None]
    ):
        """Directly subscribe to a single stream without discovery."""
        self._registry[stream] = handler
        print(f"[BusAdapterV2] Subscribing directly to stream '{stream}'")

        async def callback(raw):
            if isinstance(raw, Envelope):
                env = raw
            else:
                env = Envelope.from_dict(raw)
            sender_id = env.headers.get("x-aetherbus-sender-id")
            if sender_id == self.agent_id:
                return

            # Inject the destination stream key into the meta field so the handler knows where it came from.
            env.meta['x_stream_key'] = stream
            # --- END: THIS IS THE ONE-LINE FIX ---

            if env.is_offloaded and env.content_ref:
                try:
                    env.content = await self.offloader.download_content(env.content_ref)
                    env.is_offloaded = False
                    print(f"Retrieved offloaded payload from Azure: {env.content_ref.blob_name}")
                except Exception as e:
                    print(f"Payload retrieval failed: {e}")

            try:
                await handler(env, self.redis)
            except TypeError:
                await handler(env)

        task = asyncio.create_task(
            subscribe(
                self.redis,
                stream,
                callback,
                group=self.group,
                consumer=f"{self.agent_id}_{uuid.uuid4().hex[:6]}"
            )
        )
        self._running_subscription_tasks[stream] = task
        print(f"[BusAdapterV2] Stream subscription task created for '{stream}'")

    async def add_subscription(
        self,
        pattern: str,
        handler: Callable[[Envelope, any], None]
    ):
        """
        Dynamically subscribe to a new pattern (returns immediately).
        """
        if any(ch in pattern for ch in ["*", "?", "["]):
            await self._subscribe_pattern(pattern, handler)
        else:
            await self._subscribe_stream(pattern, handler)

    async def remove_subscription(self, pattern: str):
        self._registry.pop(pattern, None)
        task_to_cancel = self._running_subscription_tasks.pop(pattern, None)
        if task_to_cancel:
            if not task_to_cancel.done():
                print(f"[BusAdapterV2] Attempting to cancel task for pattern '{pattern}' (task: {id(task_to_cancel)})...")
                task_to_cancel.cancel()
                try:
                    # Wait for the task to actually finish after cancellation request
                    # This allows its internal try/except/finally blocks for CancelledError to run
                    await asyncio.wait_for(task_to_cancel, timeout=5.0) # Add a timeout
                    print(f"[BusAdapterV2] Subscription task for pattern '{pattern}' completed after cancellation request.")
                except asyncio.CancelledError:
                    print(f"[BusAdapterV2] Subscription task for pattern '{pattern}' successfully cancelled and awaited.")
                except asyncio.TimeoutError:
                    print(f"[BusAdapterV2] Timeout awaiting cancelled task for '{pattern}'. It might not have handled cancellation cleanly.")
                except RedisConnectionError: 
                    print(f"[BusAdapterV2] Redis connection was closed while awaiting cancelled task for '{pattern}'. This is usually okay during shutdown.")
                except Exception as e: 
                    print(f"[BusAdapterV2] Error awaiting cancelled task for '{pattern}': {type(e).__name__} - {e}")   

    def list_subscriptions(self) -> List[str]:
        """Return all currently registered patterns."""
        return list(self._registry.keys())

    async def stop(self):
        """Cancel all active subscription tasks."""
        print(f"[BusAdapterV2] Stopping adapter for '{self.agent_id}'")
        for pattern in list(self._running_subscription_tasks.keys()):
            await self.remove_subscription(pattern)

    async def publish(self, stream: str, env: Envelope):
        """
        Publish an Envelope to a Redis stream.
        """
        content_size = 0
        content = env.content
        print(f"[BusAdapterV2] Preparing to publish to {stream}")

        if isinstance(content, bytes):
            content_size = len(content)
        elif isinstance(content, str):
            content_size = len(content.encode("utf-8"))
        elif content is not None:
            content_size = len(json.dumps(content).encode("utf-8"))

        print(f"[BusAdapterV2] Content size: {content_size} bytes")
        if content_size > self.offload_threshold:
            try:
                env.content_ref = await self.offloader.upload_content(content, container_name="aetherbus-payloads")
                env.is_offloaded = True
                env.content = None
                print(f"Offloaded large payload to Azure: {env.content_ref.blob_url}")
            except Exception as e:
                print(f"Payload offload failed: {e}. Sending original content")

        env.headers["x-aetherbus-sender-id"] = self.agent_id
        print(
            f"[BusAdapterV2][publish] -> {stream} | reply_to={env.reply_to} | meta={env.meta}"
        )
        await publish_envelope(self.redis, stream, env)
        print(f"[BusAdapterV2] Publish to {stream} complete")


    def dump_wiring(self) -> List[Dict[str, str]]:
        """
        Returns a list of dicts like:
          [{"pattern": "<stream>", "handler": "<handler name>"}…]
        """
        return [
            {"pattern": pat, "handler": getattr(h, "__name__", repr(h))}
            for pat, h in self._registry.items()
        ]

    async def wait_for_next_message(
        self,
        pattern: str,
        predicate: Callable[[Envelope], bool]=lambda e: True,
        timeout: float=60.0
        ) -> Envelope:
        """Wait for the next message on ``pattern`` that matches ``predicate``."""
        print(f"[WaitForNext] subscribing to '{pattern}' for a single message")

        history = await self.redis.xrange(pattern, count=None)
        last_id = history[-1][0] if history else "0-0"
        deadline = time.time() + timeout

        while time.time() < deadline:
            entries = await self.redis.xrange(pattern, start=last_id, end="+", count=2)
            for msg_id, fields in entries:
                if msg_id == last_id:
                    continue
                last_id = msg_id
                raw = (
                    fields.get("data")
                    or fields.get(b"data")
                    or fields.get("envelope")
                    or fields.get(b"envelope")
                )
                if raw is not None:
                    env = Envelope.from_dict(
                        raw if isinstance(raw, dict) else json.loads(raw)
                    )
                    sender_id = env.headers.get("x-aetherbus-sender-id")
                    if sender_id == self.agent_id and predicate(env):
                        return env
            await asyncio.sleep(0.05)
        raise asyncio.TimeoutError

