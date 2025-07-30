"""Envelope data model for AEtherBus messages."""
from __future__ import annotations

from datetime import datetime
import time
import uuid
from typing import Any, Dict, List, Optional

from pydantic import BaseModel, Field, ConfigDict


class Envelope(BaseModel):
    """Message wrapper for all bus communications."""

    role: str
    content: Any = None
    session_code: Optional[str] = None
    agent_name: Optional[str] = None
    usage: Dict[str, Any] = Field(default_factory=dict)
    billing_hint: Optional[str] = None
    trace: List[str] = Field(default_factory=list)
    user_id: Optional[str] = None
    task_id: Optional[str] = None
    target: Optional[str] = None
    reply_to: Optional[str] = None
    envelope_type: Optional[str] = "message"
    tools_used: List[str] = Field(default_factory=list)
    auth_signature: Optional[str] = None
    timestamp: str = Field(default_factory=lambda: datetime.utcnow().isoformat())
    headers: Dict[str, str] = Field(default_factory=dict)
    meta: Dict[str, Any] = Field(default_factory=dict)
    envelope_id: str = Field(default_factory=lambda: str(uuid.uuid4()))
    correlation_id: Optional[str] = None

    # ------------------------------------------------------------------
    # v3.1 compatibility: support ``conversation_id`` as an alias for
    # ``correlation_id``. Agents use ``conversation_id`` in the new
    # asynchronous conversation model described in ``AGENTS.md``.
    # ------------------------------------------------------------------

    @property
    def conversation_id(self) -> Optional[str]:
        """Alias for ``correlation_id`` used throughout the v3.1 spec."""
        return self.correlation_id

    @conversation_id.setter
    def conversation_id(self, value: Optional[str]) -> None:
        self.correlation_id = value

    model_config = ConfigDict(extra="forbid")

    def to_dict(self) -> Dict[str, Any]:
        """Return a dictionary representation of this envelope."""
        return self.model_dump()

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "Envelope":
        """Create an Envelope from a dictionary with validation."""
        return cls.model_validate(data)

    def add_hop(self, who: str) -> None:
        """Record a hop in the trace with epoch seconds."""
        self.trace.append(f"{who}:{int(time.time())}")

    def __repr__(self) -> str:
        return (
            f"Envelope(id={self.envelope_id}, role={self.role}, "
            f"agent={self.agent_name}, type={self.envelope_type})"
        )

    __str__ = __repr__
