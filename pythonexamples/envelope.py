"""Envelope data model for AEtherBus messages."""
from __future__ import annotations

from datetime import datetime
import time
import uuid
from typing import Any, Dict, List, Optional, Union

from pydantic import BaseModel, Field, ConfigDict


class LargeContentRef(BaseModel):
    """Reference to offloaded large content"""

    blob_url: str
    storage_account: str
    container: str
    blob_name: str
    content_type: Optional[str] = None


class Envelope(BaseModel):
    """Message wrapper for all bus communications.
    
    This version is backward compatible with the old relay by:
    1. Using extra="ignore" to allow unknown fields
    2. Making new fields optional with defaults
    3. Handling both old and new message formats in from_dict
    """

    # Core fields from original version
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
    
    # New fields for blob storage support - all optional with defaults for backward compatibility
    content_ref: Optional[Union[Dict[str, Any], LargeContentRef]] = None
    is_offloaded: bool = False

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

    model_config = ConfigDict(extra="ignore")

    def to_dict(self) -> Dict[str, Any]:
        """Convert envelope to dictionary, including None values for backward compatibility.
        
        Returns:
            Dict[str, Any]: A dictionary representation of the envelope, including all fields
            even if they're None, to maintain backward compatibility.
        """
        result = self.model_dump()
        # Ensure content_ref is included as None if not set
        if 'content_ref' not in result:
            result['content_ref'] = None
        # Ensure is_offloaded is included as False if not set
        if 'is_offloaded' not in result:
            result['is_offloaded'] = False
        return result

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "Envelope":
        """Create an Envelope from a dictionary with validation.
        
        This handles backward compatibility by:
        1. Accepting both old and new message formats
        2. Stripping out any unknown fields before validation
        3. Setting defaults for new fields when not present
        4. Converting content_ref dict to LargeContentRef object if needed
        """
        # Create a copy to avoid modifying the input
        data = data.copy()
        
        # Convert content_ref dict to LargeContentRef if it exists
        if 'content_ref' in data and isinstance(data['content_ref'], dict):
            data['content_ref'] = LargeContentRef(**data['content_ref'])
        
        # List of all valid fields in the model
        valid_fields = {
            # Original fields
            'role', 'content', 'session_code', 'agent_name', 'usage', 
            'billing_hint', 'trace', 'user_id', 'task_id', 'target', 
            'reply_to', 'envelope_type', 'tools_used', 'auth_signature', 
            'timestamp', 'headers', 'meta', 'envelope_id', 'correlation_id',
            # New fields for blob storage
            'content_ref', 'is_offloaded'
        }
        
        # Create a clean dict with only the valid fields
        clean_data = {}
        for field in valid_fields:
            if field in data:
                clean_data[field] = data[field]
        
        # Ensure required fields are present
        if 'role' not in clean_data:
            clean_data['role'] = 'user'  # Default role if not specified
            
        # Handle content_ref if it's None or empty dict
        if 'content_ref' in clean_data and clean_data['content_ref'] in (None, {}):
            clean_data.pop('content_ref')
            
        # Ensure is_offloaded is a boolean if present
        if 'is_offloaded' in clean_data and not isinstance(clean_data['is_offloaded'], bool):
            clean_data['is_offloaded'] = False
            
        # Ensure envelope_id is set
        if 'envelope_id' not in clean_data:
            clean_data['envelope_id'] = str(uuid.uuid4())
            
        # Ensure timestamp is set
        if 'timestamp' not in clean_data:
            clean_data['timestamp'] = datetime.utcnow().isoformat()
            
        return cls.model_validate(clean_data)

    def add_hop(self, who: str) -> None:
        """Record a hop in the trace with epoch seconds."""
        self.trace.append(f"{who}:{int(time.time())}")

    def __repr__(self) -> str:
        return (
            f"Envelope(id={self.envelope_id}, role={self.role}, "
            f"agent={self.agent_name}, type={self.envelope_type})"
        )

    __str__ = __repr__
