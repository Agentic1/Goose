# =============================================================================
# AEtherBus Simple Service Agent Boilerplate (v4.3)
# =============================================================================
# This is the standard template for creating a new, independent service agent.
# This agent can receive a task, perform a specific action, and send a reply.
# It does NOT delegate tasks to other agents.
#
# To create a new agent (e.g., a WeatherAgent):
# 1. Copy this file and rename it (e.g., weather_agent.py).
# 2. Change the AGENT_NAME variable in the Configuration section.
# 3. Implement your custom logic in the "YOUR AGENT'S CUSTOM LOGIC" block.
# =============================================================================

import asyncio
import os
import sys
from pathlib import Path
from redis.asyncio import Redis

# --- Standard AEtherBus Imports ---
# This block ensures that the script can find the AG1_AEtherBus library
# when run directly, without needing it to be installed.
try:  
    PROJECT_ROOT = Path(__file__).resolve().parents[1] 
    if str(PROJECT_ROOT) not in sys.path:
        sys.path.insert(0, str(PROJECT_ROOT))
    from AG1_AEtherBus.envelope import Envelope
    from AG1_AEtherBus.bus import publish_envelope, build_redis_url
    from AG1_AEtherBus.bus_adapterV2 import BusAdapterV2
    from AG1_AEtherBus.keys import StreamKeyBuilder
except ImportError:
    print("ERROR: Could not import AEtherBus modules. Make sure this script is placed correctly in the project structure.")
    sys.exit(1)


# --- Agent-Specific Configuration ---
# TODO: Change this to the unique name of your new agent.
AGENT_NAME = "MySimpleService" 
KEYS = StreamKeyBuilder()
AGENT_INBOX = KEYS.agent_inbox(AGENT_NAME)


# --- The Agent Class (Self-Contained Service Pattern) ---
class MySimpleServiceAgent:
    """
    A self-contained agent that listens on its inbox, performs a task,
    and sends a reply.
    """
    def __init__(self, redis_client: Redis):
        # All state is encapsulated within the instance.
        self.name = AGENT_NAME
        self.redis = redis_client
        
        # The BusAdapter is created here, with a bound method as its handler.
        # This is critical for ensuring state is not lost.
        self.adapter = BusAdapterV2(
            agent_id=self.name,
            core_handler=self.handle_bus_envelope,
            redis_client=self.redis,
            patterns=[AGENT_INBOX],
            group=self.name
        )
        print(f"[{self.name}] Initialized.")

    async def start(self):
        """Starts the agent's bus adapter to begin listening for messages."""
        await self.adapter.start()
        print(f"[{self.name}] Listening for tasks on inbox: {AGENT_INBOX}")

    async def handle_bus_envelope(self, env: Envelope, redis: Redis):
        """
        The core handler for all incoming messages.
        This function is called by the BusAdapter for each message.
        """
        print(f"[{self.name}] Received message with CID '{env.correlation_id}' from: {env.agent_name}")
        
        # Adhere to the "Reply-To Contract": only process if a reply is expected.
        if not env.reply_to or not env.correlation_id:
            print(f"[{self.name}] Message is for notification only (no reply_to or correlation_id). Ignoring.")
            return

        try:
            # =================================================================
            # --- YOUR AGENT'S CUSTOM LOGIC GOES HERE ---
            # =================================================================
            # 1. Extract the specific task from the incoming envelope's content.
            task_details = (env.content or {}).get("task", "No task specified.")
            
            # 2. Perform the agent's unique work.
            #    (e.g., call an external API, run a calculation, query a database)
            #    For this template, we just create a simple success message.
            result_data = f"Successfully completed task: '{task_details}'."
            
            # 3. Create the result payload.
            reply_payload = {"status": "ok", "data": result_data}
            # =================================================================
            # --- END OF CUSTOM LOGIC ---
            # =================================================================

        except Exception as e:
            # If your custom logic fails, create a standard error payload.
            print(f"[{self.name}][ERROR] An error occurred while processing the task: {e}")
            reply_payload = {"status": "error", "error": f"An internal error occurred: {str(e)}"}

        # Adhere to the "Reply-To Contract": create a reply envelope.
        reply_env = Envelope(
            role="agent_response",
            agent_name=self.name,
            content=reply_payload,
            correlation_id=env.correlation_id # CRITICAL: Echo the original ID back.
        )
        
        # Publish the reply directly back to the original requester.
        print(f"[{self.name}] Sending reply with CID '{env.correlation_id}' to: {env.reply_to}")
        await publish_envelope(redis, env.reply_to, reply_env)


# --- Standard Startup Boilerplate (Do not change this part) ---
async def main():
    """Initializes and starts the agent, keeping it running."""
    try:
        # decode_responses=True is mandatory for all agents to prevent silent crashes.
        redis_client = Redis.from_url(build_redis_url(), decode_responses=True)
        await redis_client.ping() # Check the connection before starting
        print(f"[{AGENT_NAME}] Successfully connected to Redis.")
    except Exception as e:
        print(f"[FATAL] Could not connect to Redis. Please check your .env configuration. Error: {e}")
        return

    # Create the single, authoritative instance of the agent.
    agent = MySimpleServiceAgent(redis_client)
    
    # Start the agent and let it run forever.
    await agent.start()
    await asyncio.Future()

if __name__ == "__main__":
    print(f"--- Starting {AGENT_NAME} ---")
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print(f"\n--- {AGENT_NAME} shutting down. ---")