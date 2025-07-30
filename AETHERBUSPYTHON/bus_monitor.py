import asyncio
import os
from datetime import datetime
from typing import List, Tuple

from redis.asyncio import Redis
from rich.console import Console
from rich.live import Live
from rich.table import Table

import sys
from pathlib import Path

# Add the project root to Python path
PROJECT_ROOT = Path(__file__).parent.parent  # Adjust based on your actual project structure
sys.path.insert(0, str(PROJECT_ROOT))

from AG1_AEtherBus.bus_adapterV2 import BusAdapterV2
from AG1_AEtherBus.envelope import Envelope
from AG1_AEtherBus.bus import build_redis_url


class BusMonitor:
    """Simple TUI monitor for AEtherBus envelopes."""

    def __init__(self, redis_url: str, patterns: List[str] | None = None, max_events: int = 20):
        self.redis = Redis.from_url(redis_url)
        self.events: List[Tuple[str, str, str, str, str]] = []
        self.max_events = max_events
        self.adapter = BusAdapterV2(
            agent_id="BusMonitor",
            core_handler=self.handle_bus_envelope,
            redis_client=self.redis,
            patterns=patterns or ["AG1:*", "user.*"],
        )

    async def handle_bus_envelope(self, env: Envelope, redis: Redis):
        timestamp = datetime.utcnow().strftime("%H:%M:%S")
        preview = str(env.content)[:40]
        self.events.append((timestamp, env.role or "", env.agent_name or "", env.envelope_type or "", preview))
        if len(self.events) > self.max_events:
            self.events.pop(0)

    def _render(self) -> Table:
        table = Table(title="AEtherBus Monitor")
        table.add_column("Time", width=8)
        table.add_column("Role", style="cyan", width=8)
        table.add_column("Agent", style="green", width=15)
        table.add_column("Type", style="magenta", width=12)
        table.add_column("Content", style="yellow")
        for t, role, agent, etype, cont in self.events:
            table.add_row(t, role, agent, etype, cont)
        return table

    async def run(self):
        console = Console()
        await self.adapter.start()
        with Live(self._render(), console=console, refresh_per_second=2) as live:
            while True:
                await asyncio.sleep(0.5)
                live.update(self._render())


def run():
    redis_url = os.environ.get("REDIS_URL", build_redis_url())
    monitor = BusMonitor(redis_url)
    asyncio.run(monitor.run())


if __name__ == "__main__":
    run()
