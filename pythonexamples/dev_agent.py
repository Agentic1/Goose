import asyncio
import json
import sys
import uuid
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence

from redis.asyncio import Redis as IoRedis

# --- Project Path Setup ---
PROJECT_ROOT = Path(__file__).resolve().parents[2]
if str(PROJECT_ROOT) not in sys.path:
    sys.path.insert(0, str(PROJECT_ROOT))

# --- AEtherBus and AutoGen Imports ---
from AG1_AEtherBus.bus import build_redis_url, publish_envelope
from AG1_AEtherBus.bus_adapterV2 import BusAdapterV2
from AG1_AEtherBus.envelope import Envelope
from AG1_AEtherBus.keys import StreamKeyBuilder
from AG1_Muse.muse_agent.hot_swap_proxy_agent import HotSwapProxyAgent
from typing import Dict, Any, Optional
from AG1_Muse.orchestrator_agent import dev_tools

from autogen_agentchat.agents import AssistantAgent, UserProxyAgent
from autogen_agentchat.messages import TextMessage, BaseChatMessage
from autogen_agentchat.teams import RoundRobinGroupChat
from autogen_agentchat.base import Response
from autogen_core.tools import FunctionTool
from autogen_ext.models.openai import AzureOpenAIChatCompletionClient

# --- LLM Configuration ---
AZURE_DEPLOYMENT_NAME = "gpt-4o-mini"
AZURE_ENDPOINT = "https://sean-m7497eun-swedencentral.openai.azure.com/openai/deployments/gpt-4o-mini-haka/chat/completions?api-version=2024-08-01-preview"
API_KEY = "E5Af5mwzenBJI2pciYRqknSSJSGAoBTtSoeidFW2fQ2h4iWW8SfrJQQJ99BBACfhMk5XJ3w3AAAAACOGTCnr"



# Add these methods to the DevAgent class:



class DevAgent(AssistantAgent):
    """
    An interactive CLI agent for developers to directly interact with the AEtherBus.
    """

    def __init__(self, name: str = "DevAgent", redis_client: IoRedis = None, model_client=None):
        self.redis = redis_client
        self.kb = StreamKeyBuilder()
        self.ongoing_conversations: Dict[str, asyncio.Future] = {}

        self.adapter = BusAdapterV2(
            agent_id=name,
            core_handler=self._handle_reply,
            redis_client=self.redis,
            patterns=[self.kb.agent_inbox(name)],
        )

        self.hot_swap_proxy = HotSwapProxyAgent(f"{name}_Proxy", self.redis)

        tools = [
            FunctionTool(self._call_agent, name="call_agent", 
                        description="Send a query to any agent on the bus and get a reply. Args: target_agent (str), query (str)"),
            FunctionTool(self._hotswap_agent, name="hotswap", 
                        description="Manage the HotSwapProxy to connect or disconnect from an agent. Args: action ('add' or 'remove'), agent_name (str)"),
            FunctionTool(self._search_docs, name="search_docs", 
                        description="Search the documentation. Args: query (str)"),
            FunctionTool(self.list_agents, name="list_agents",
                        description="List all registered agents on the bus. No arguments."),
            FunctionTool(self.search_agents, name="search_agents",
                        description="Search for agents by name or keywords. Args: query (str)"),
            FunctionTool(self.get_agent_details, name="get_agent_details",
                        description="Get detailed information about an agent. Args: agent_id (str)"),
            FunctionTool(self.start_session, name="start_session",
                        description="Start a new UI session with an agent. Args: agent_id (str), user_id (str, optional), initial_query (str, optional)"),
            FunctionTool(self.route_task, name="route_task",
                        description="Send a raw payload to an agent. Args: agent_id (str), payload (dict)"),
            FunctionTool(self.get_status, name="get_status",
                        description="Get status of an agent. Args: agent_id (str)"),
        ]

        system_message = "You are a Developer's Console assistant. Your job is to execute commands to interact with the AEtherBus. Use `call_agent` for single requests, `hotswap` to manage direct chats, and `search_docs` for documentation."

        super().__init__(
            name=name,
            system_message=system_message,
            model_client=model_client,
            tools=tools,
        )

    async def start(self):
        await self.adapter.start()
        await self.hot_swap_proxy.start()
        print(f"[{self.name}] Online. Listening for replies on: {self.kb.agent_inbox(self.name)}")

    async def stop(self):
        await self.adapter.stop()
        print(f"[{self.name}] Offline.")

    
    async def list_agents(self) -> Dict[str, Any]:
        """List all registered agents on the bus."""
        return await dev_tools.list_agents(self.redis)

    async def search_agents(self, query: str) -> Dict[str, Any]:
        """Search for agents by name or keywords."""
        return await dev_tools.search_agents(self.redis, query)

    async def get_agent_details(self, agent_id: str) -> Dict[str, Any]:
        """Get detailed information about a specific agent."""
        return await dev_tools.get_agent_details(self.redis, agent_id)

    async def start_session(self, agent_id: str, user_id: str = "dev_console", 
                        initial_query: str = "") -> Dict[str, Any]:
        """Start a new UI session with an agent."""
        meta = {"initial_query": initial_query} if initial_query else None
        return await dev_tools.start_agent_session(
            self.redis, agent_id, user_id, meta
        )

    async def stop_session(self, session_id: str, agent_id: str, 
                        user_id: str = "dev_console") -> Dict[str, Any]:
        """Terminate an active session."""
        return await dev_tools.stop_agent_session(
            self.redis, session_id, agent_id, user_id
        )

    async def route_task(self, agent_id: str, payload: Dict[str, Any]) -> Dict[str, Any]:
        """Send a raw payload to an agent."""
        return await dev_tools.route_task(
            self.redis, agent_id, payload, 
            reply_to=self.kb.agent_inbox(self.name)
        )

    async def get_status(self, agent_id: str) -> Dict[str, Any]:
        """Get status of an agent."""
        return await dev_tools.get_agent_status(self.redis, agent_id)

    async def _handle_reply(self, env: Envelope, redis: IoRedis):
        def format_content(content):
            """Format the content for better readability."""
            if isinstance(content, (dict, list)):
                # Pretty print JSON with sorted keys and consistent indentation
                return json.dumps(content, indent=2, sort_keys=True)
            return str(content)

        cid = env.correlation_id
        if cid and cid in self.ongoing_conversations:
            future = self.ongoing_conversations.pop(cid, None)
            if future and not future.done():
                future.set_result(env.content)
        else:
            # Format the timestamp if available
            timestamp = f" [{env.timestamp}]" if hasattr(env, 'timestamp') else ""
            
            if self.hot_swap_proxy.is_active:
                agent_name = self.hot_swap_proxy.target_agent_name
                print(f"\n{'='*80}")
                print(f"[{agent_name}{timestamp}]")
                print("-"*80)
                print(format_content(env.content))
                print(f"{'='*80}")
                print(f"(dev-agent) > ", end="", flush=True)
            else:
                print(f"\n{'!'*30} UNSOLICITED MESSAGE {'!'*30}")
                print(f"From: {env.agent_name}{timestamp}")
                print("-"*80)
                print(format_content(env.content))
                print(f"{'!'*80}")
                print(f"(dev-agent) > ", end="", flush=True)


    async def _call_agent(self, target_agent: str, query: str) -> str:
        print(f"--> Calling agent '{target_agent}' with query: '{query}'")
        correlation_id = str(uuid.uuid4())
        future = asyncio.get_running_loop().create_future()
        self.ongoing_conversations[correlation_id] = future

        try:
            content_payload = json.loads(query)
        except json.JSONDecodeError:
            content_payload = {"text": query}

        request_env = Envelope(
            role="user_request",
            agent_name=self.name,
            content=content_payload,
            correlation_id=correlation_id,
            reply_to=self.kb.agent_inbox(self.name),
        )

        if ":" in target_agent:
            service, instance = target_agent.split(":", 1)
            target_inbox = self.kb.edge_inbox(service, instance)
        elif target_agent.lower() in {"fetch", "fetch_edge"}:
            # fetch_edge handler still listens on the "fetch" inbox
            target_inbox = self.kb.edge_inbox("fetch")
        else:
            target_inbox = self.kb.agent_inbox(target_agent)

        try:
            #await self.adapter.publish(self.kb.agent_inbox(target_agent), request_env)
            await self.adapter.publish(target_inbox, request_env)
            response = await asyncio.wait_for(future, timeout=30.0)
            return json.dumps(response, indent=2)
        except asyncio.TimeoutError:
            return f"Error: Timeout waiting for reply from {target_agent}."
        except Exception as e:
            return f"Error calling agent {target_agent}: {e}"
        finally:
            self.ongoing_conversations.pop(correlation_id, None)

    async def _search_docs(self, query: str) -> str:
        return await self._call_agent("DocReferenceAgent", query)

    async def _hotswap_agent(self, action: str, agent_name: str) -> str:
        if action.lower() == 'add':
            print(f"--> Adding '{agent_name}' to conversation...")
            details_str = await self._call_agent("Orchestrator", json.dumps({
                "action": "get_agent_details_for_hotswap",
                "agent_concept_name": agent_name
            }))
            details = json.loads(details_str)

            if details.get("status") != "success":
                return f"Error getting details for '{agent_name}': {details.get('error', 'Unknown')}"

            self.hot_swap_proxy.activate_proxy(
                agent_name=details.get("agent_name_for_display", agent_name),
                agent_inbox=details.get("target_inbox"),
                connector_type=details.get("connector_type", "delegate"),
                connector_details=details.get("connector_details", {})
            )
            
            # *** THE CRITICAL FIX IS HERE ***
            # We set the context for the proxy, telling it who the "user" is (us)
            # and where to send final replies.
            self.hot_swap_proxy.set_context(
                user_id=self.name,
                reply_to=self.kb.agent_inbox(self.name),
                meta={"source_cli": "dev_agent"}
            )
            
            return f"Success. Connected to {agent_name}. You can now talk to it directly."
        
        elif action.lower() == 'remove':
            disconnected_agent = self.hot_swap_proxy.target_agent_name or "the current agent"
            self.hot_swap_proxy.deactivate_proxy()
            return f"Success. Disconnected from {disconnected_agent}."
        else:
            return "Error: Invalid hotswap action. Use 'add' or 'remove'."

    
    async def _print_help(self):
        """Display help information for all available commands."""
        help_text = """
            Developer Console Help
            ====================

            Core Commands:
            --------------
            - help: Show this help message
            - exit/quit: Exit the console

            Agent Interaction:
            -----------------
            - call <agent> <query>: Send a one-time query to an agent
            Example: call Orchestrator "list all agents"

            - hotswap <add|remove> <agent>: Connect/disconnect from an agent for multi-turn chat
            Example: hotswap add Orchestrator

            Agent Discovery:
            ---------------
            - list: List all registered agents
            - search <query>: Search for agents by name or keywords
            Example: search doc
            - details <agent_id>: Get detailed info about an agent
            Example: details Orchestrator

            Session Management:
            ------------------
            - start <agent_id> [user_id] [initial_query]: Start a new session
            Example: start DocReferenceAgent user123 "Help me find docs"
            - stop <session_id> <agent_id> [user_id]: End a session
            Example: stop session123 DocReferenceAgent user123

            Advanced:
            --------
            - route <agent_id> <json_payload>: Send raw JSON to an agent
            Example: route Orchestrator '{"action":"list_agents"}'
            - status <agent_id>: Check agent status
            Example: status Orchestrator

            Documentation:
            -------------
            - docs <query>: Search documentation
            Example: docs how to create an agent
            """
        print(help_text)

    async def run_cli(self):
        """Run the interactive command-line interface."""
        print("\nAEtherBus Developer Console")
        print("=" * 50)
        print("Type 'help' for commands, 'exit' to quit\n")
        
        loop = asyncio.get_running_loop()
        
        while True:
            try:
                # Get user input with appropriate prompt
                prompt = f"\n({self.hot_swap_proxy.target_agent_name})> " if self.hot_swap_proxy.is_active else "\ndev> "
                command_str = await loop.run_in_executor(None, lambda: input(prompt).strip())
                
                if not command_str:
                    continue
                    
                # Handle core commands first
                if command_str.lower() in ["exit", "quit"]:
                    print("Goodbye!")
                    break
                    
                if command_str.lower() == "help":
                    await self._print_help()
                    continue
                
                # If proxy is active, forward command directly to the agent
                if self.hot_swap_proxy.is_active and not command_str.startswith('.'):
                    print(f"--> Forwarding to {self.hot_swap_proxy.target_agent_name}...")
                    await self.hot_swap_proxy.ask(command_str)
                    continue
                
                # Handle local commands (prefixed with . when in hotswap mode)
                if command_str.startswith('.') and self.hot_swap_proxy.is_active:
                    command_str = command_str[1:].strip()
                
                # Parse command
                parts = command_str.split()
                if not parts:
                    continue
                    
                cmd = parts[0].lower()
                args = parts[1:]
                
                # Command routing
                try:
                    if cmd == "list":
                        result = await self.list_agents()
                        
                    elif cmd == "search" and len(args) > 0:
                        result = await self.search_agents(" ".join(args))
                        
                    elif cmd == "details" and len(args) > 0:
                        result = await self.get_agent_details(args[0])
                        
                    elif cmd == "start" and len(args) > 0:
                        agent_id = args[0]
                        user_id = args[1] if len(args) > 1 else "dev_console"
                        initial_query = " ".join(args[2:]) if len(args) > 2 else ""
                        result = await self.start_session(agent_id, user_id, initial_query)
                        
                    elif cmd == "stop" and len(args) > 1:
                        session_id = args[0]
                        agent_id = args[1]
                        user_id = args[2] if len(args) > 2 else "dev_console"
                        result = await self.stop_session(session_id, agent_id, user_id)
                        
                    elif cmd == "route" and len(args) > 1:
                        agent_id = args[0]
                        try:
                            payload = json.loads(" ".join(args[1:]))
                            result = await self.route_task(agent_id, payload)
                        except json.JSONDecodeError:
                            result = {"status": "error", "error": "Invalid JSON payload"}
                            
                    elif cmd == "status" and len(args) > 0:
                        result = await self.get_status(args[0])
                        
                    elif cmd == "call" and len(args) > 1:
                        target_agent = args[0]
                        query = " ".join(args[1:])
                        result = await self._call_agent(target_agent, query)
                        
                    elif cmd == "hotswap" and len(args) > 1:
                        action = args[0]
                        agent_name = args[1]
                        result = await self._hotswap_agent(action, agent_name)
                        
                    elif cmd == "docs" and len(args) > 0:
                        query = " ".join(args)
                        result = await self._search_docs(query)
                        
                    else:
                        print(f"Unknown command: {cmd}. Type 'help' for available commands.")
                        continue
                        
                    # Print result
                    print("\n--- RESPONSE ---")
                    if isinstance(result, str):
                        try:
                            result = json.loads(result)
                        except (json.JSONDecodeError, TypeError):
                            pass
                    if isinstance(result, dict) or isinstance(result, list):
                        print(json.dumps(result, indent=2))
                    else:
                        print(result)
                    print("----------------")
                        
                except Exception as e:
                    print(f"\nError executing command: {str(e)}\n")
                    
            except (KeyboardInterrupt, EOFError):
                print("\nUse 'exit' or 'quit' to exit the console.")
            except Exception as e:
                print(f"\nUnexpected error: {str(e)}")
    

async def main():
    redis_client = IoRedis.from_url(build_redis_url(), decode_responses=True)
    
    llm_client = AzureOpenAIChatCompletionClient(
        azure_deployment=AZURE_DEPLOYMENT_NAME,
        azure_endpoint=AZURE_ENDPOINT,
        api_key=API_KEY,
        model=AZURE_DEPLOYMENT_NAME,
        api_version="2024-08-01-preview"
    )

    dev_agent = DevAgent(redis_client=redis_client, model_client=llm_client)
    await dev_agent.start()
    
    try:
        await dev_agent.run_cli()
    finally:
        await dev_agent.stop()
        if hasattr(redis_client, "aclose"):
            await redis_client.aclose()

if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print("\nExiting DevAgent.")