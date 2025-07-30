import asyncio
from typing import Callable, List
from redis.asyncio import Redis
from AG1_AEtherBus.bus_adapterV2 import BusAdapterV2
from AG1_AEtherBus.envelope import Envelope
from AG1_AEtherBus.bus import publish_envelope

class BusConnector:
    """
    A reusable connector around AEtherBus:
     - patterns: list of Redis streams to subscribe to
     - handler:  async fn(env: Envelope) called on each message
     - group:    consumer-group name (and agent_id if you like)
    """

    def __init__(
        self,
        redis_client: Redis,
        patterns: List[str],
        handler: Callable[[Envelope], None],
        group: str,
        agent_id: str = None,
    ):
        self.redis    = redis_client
        self.patterns = patterns
        self.handler  = handler
        self.group    = group
        self.agent_id = agent_id or group

        # build the adapter but don’t start it yet
        self._adapter = BusAdapterV2(
            agent_id     = self.agent_id,
            core_handler = self._on_message,
            redis_client = self.redis,
            patterns     = self.patterns,
            group        = self.group,
        )
        self._task: asyncio.Task = None

    async def start(self):
        """Begin listening in the background."""
        # start the adapter in its own task so start() doesn’t block forever
        self._task = asyncio.create_task(self._adapter.start())

    async def stop(self):
        """Cancel the background subscription."""
        if self._task:
            self._task.cancel()
        await self._adapter.stop()

    async def _on_message(self, env: Envelope, redis: Redis):
        # call your handler (you can catch/log exceptions here)
        try:
            await self.handler(env)
        except Exception:
            # Swallow or log—never let one bad handler kill the loop
            import traceback; traceback.print_exc()

    async def publish(self, stream: str, env: Envelope):
        """Helper to publish an envelope onto any stream."""
        await publish_envelope(self.redis, stream, env)
