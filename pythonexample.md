# social_agent_6_tools.py
import os
import re
import json
import asyncio
import logging
import queue
import uuid
import sys
from pathlib import Path
from typing import Dict, Any, Optional, List, Union, Callable

# Configure logging
logging.basicConfig(
    level=logging.DEBUG,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s',
    handlers=[
        logging.StreamHandler(sys.stdout)
    ]
)
logger = logging.getLogger('social_agent')

# Add project root to path
PROJECT_ROOT = Path(__file__).resolve().parents[2]
if str(PROJECT_ROOT) not in sys.path:
    sys.path.insert(0, str(PROJECT_ROOT))

# Ensure the project root is on sys.path
PROJECT_ROOT = Path(__file__).resolve().parents[2]
if str(PROJECT_ROOT) not in sys.path:
    sys.path.insert(0, str(PROJECT_ROOT))

from redis.asyncio import Redis as AsyncRedis
from AG1_AEtherBus.keys import StreamKeyBuilder
from AG1_AEtherBus.envelope import Envelope
from AG1_AEtherBus.bus import publish_envelope
from AG1_AEtherBus.bus_adapterV2 import BusAdapterV2

# Import AutoGen components
from autogen_agentchat.agents import AssistantAgent, UserProxyAgent
from autogen_agentchat.teams import RoundRobinGroupChat
from autogen_agentchat.messages import TextMessage, ToolCallSummaryMessage

from autogen_agentchat.conditions import MaxMessageTermination, TextMentionTermination
from autogen_ext.models.openai import AzureOpenAIChatCompletionClient
from autogen_core.tools import FunctionTool

# Import social clients
from .social_agent_05 import TwitterClient, TelegramClient, LinkedInClient
from .social_tools.networks.linkedin_client import LinkedInClient
from .social_tools.networks.x_client import XClient
from .social_tools.core.models import DraftPost


async def post_to_linkedin(text: str, organization_id: str = None) -> dict:
    """
    Post content to LinkedIn using the existing LinkedIn client.
    
    Args:
        text: The text content to post
        organization_id: Optional organization ID (will use from credentials if not provided)
    """
    # Get credentials
    with open('path/to/credentials.json') as f:
        creds = json.load(f)
    
    # Initialize client
    client = LinkedInClient(
        access_token=creds['linkedin']['access_token'],
        organization_id=organization_id or creds['linkedin'].get('organization_id')
    )
    
    # Create post
    post = DraftPost(
        text=text,
        media_items=[],  # Can add media if needed
        scheduled_time=None  # Post immediately
    )
    
    # Post to LinkedIn
    result = await client.create_post(post)
    return result

def get_cred(key: str, default: Any = None) -> Any:
    """Get credential from environment or credentials.json"""
    # First try environment variables
    value = os.getenv(key)
    if value is not None:
        return value
    
    # Then try credentials.json
    creds_file = Path(__file__).parent / "credentials.json"
    if creds_file.exists():
        with open(creds_file) as f:
            creds = json.load(f)
            return creds.get(key, default)
    return default

class PatchedUserProxy(UserProxyAgent):
    def __init__(self, *args, shared_memory=None, **kwargs):
        super().__init__(*args, **kwargs)
        self.shared_memory = shared_memory
        
    async def get_human_input(self, prompt: str) -> str:
        if self._input_func:
            return await self._input_func(prompt)
        return ""


class SocialAgent(AssistantAgent):
    def __init__(self, name: str, redis_client: AsyncRedis, model_client_instance):
        # Set environment variables for XClient
        logger.info("Initializing Twitter client with credentials...")
        try:
            os.environ["X_API_KEY"] = get_cred("TW_API_KEY")
            os.environ["X_API_SECRET"] = get_cred("TW_API_SECRET")
            os.environ["X_ACCESS_TOKEN"] = get_cred("TW_ACCESS_TOKEN")
            os.environ["X_ACCESS_SECRET"] = get_cred("TW_ACCESS_SECRET")
            os.environ["X_BEARER_TOKEN"] = get_cred("TW_BEARER")
            
            # Log that we've set the credentials (without showing values)
            logger.info("Twitter credentials set successfully")
            logger.debug(f"Using API Key: {os.environ['X_API_KEY'][:5]}...")
            logger.debug(f"Using Bearer Token: {os.environ['X_BEARER_TOKEN'][:10]}..." if 'X_BEARER_TOKEN' in os.environ else "No Bearer Token found")
            
        except Exception as e:
            logger.error(f"Failed to set Twitter credentials: {e}")
            raise
        
        # Initialize XClient (uses environment variables)
        self.twitter = XClient()
        
        # Keep old client as fallback
        self._legacy_twitter = TwitterClient(
            bearer=get_cred("TW_BEARER"),
            ck=get_cred("TW_API_KEY"),
            cs=get_cred("TW_API_SECRET"),
            at=get_cred("TW_ACCESS_TOKEN"),
            ats=get_cred("TW_ACCESS_SECRET")
        )
        # Initialize Telegram client with credentials
        self.telegram = TelegramClient(
            bot_token=get_cred("TG_BOT_TOKEN"),
            default_chat_id=get_cred("TG_DEFAULT_CHAT_ID")
        )
        try:
            creds = get_cred("linkedin", {})
            if not creds.get("access_token"):
                print("[LinkedIn] Warning: No access token found in credentials")
                self.linkedin = None
            else:
                # Get organization ID from credentials or use default from curl example
                org_id = creds.get("organization_id", "88073473")  # Using the org ID from your working curl
                print(f'[LinkedIn] Initializing client with org_id: {org_id}')
                
                self.linkedin = LinkedInClient(
                    access_token=creds["access_token"].strip(),
                    organization_id=org_id
                )
                print(f'[LinkedIn] Client initialized successfully with org_id: {org_id}')
        except Exception as e:
            import traceback
            print(f'[LinkedIn] Error initializing client: {str(e)}')
            print(f'[LinkedIn] Stack trace: {traceback.format_exc()}')
            self.linkedin = None
#LEFT OFF BOX XRASH. we unpdated init methods here !!!!
        # Initialize the base AssistantAgent
        super().__init__(
            name=name,
            system_message=f"""You are {name}, a social media assistant. Your primary function is to help users post to social media.
            
            TOOLS:
            - Use 'post_to_twitter' to post tweets
            - Use 'send_telegram' to send Telegram messages
            - Always confirm before posting
            
            TELEGRAM INSTRUCTIONS:
            - To send a message: send_telegram("your message")
            - The chat ID is automatically taken from credentials (TG_DEFAULT_CHAT_ID)
            - Just provide the message text - no chat ID needed
            - Example: send_telegram("Hello from the bot!")
            
            GENERAL GUIDELINES:
            - If the request is unclear, ask for clarification
            - When done, include 'COMPLETE' in your final message
            """,
            model_client=model_client_instance,
            tools=[
                self._create_twitter_tool(),
                self._create_telegram_tool(),
                self._create_linkedin_tool(),
            ]
        )
        
        # Store Redis client for bus communication
        self.redis = redis_client
        self.kb = StreamKeyBuilder("AG1")
        self.adapter = BusAdapterV2(
            agent_id=name,
            redis_client=redis_client,
            core_handler=self.handle_bus_envelope,
            patterns=[self.kb.agent_inbox(name)],
            group=name,
            registration_profile=self.profile(),
        )
        
        
        print(f"[{name}] Social Agent initialized with tools: {[t.name for t in self._tools]}")

    def _create_twitter_tool(self) -> FunctionTool:
        """Create and return the Twitter posting tool"""
        async def post_to_twitter(text: str) -> str:
            """Post a tweet to Twitter. Handles multi-part tweets by detecting part indicators (e.g., '1/3', '2/3').
            
            Args:
                text (str): The text content to tweet. Can be a single tweet or part of a multi-part tweet.
                
            Returns:
                str: Success message with tweet IDs or error message
            """
            try:
                # Check if this is part of a multi-part tweet (format: "1/3 text...", "2/3 text...", etc.)
                import re
                part_match = re.match(r'^(\d+)/(\d+)\s+(.+)', text.strip())
                
                if not part_match:
                    # Single tweet - use XClient
                    try:
                        from .social_tools.core.models import DraftPost
                        draft = DraftPost(caption=text)
                        result = await self.twitter.publish(draft)
                        if result.success:
                            return f"Successfully posted tweet: {result.url}"
                        else:
                            # Fallback to legacy client if XClient fails
                            print(f"XClient failed: {result.error}, falling back to legacy client")
                            tweet_id = await self._legacy_twitter.post_tweet(text)
                            return f"Successfully posted tweet with ID: {tweet_id}"
                    except Exception as e:
                        print(f"Error with XClient: {e}, falling back to legacy client")
                        tweet_id = await self._legacy_twitter.post_tweet(text)
                        return f"Successfully posted tweet with ID: {tweet_id}"
                
                # Multi-part tweet
                current_part = int(part_match.group(1))
                total_parts = int(part_match.group(2))
                tweet_text = part_match.group(3)
                
                if current_part == 1:
                    # First part - store it and wait for other parts
                    if not hasattr(self, '_tweet_parts'):
                        self._tweet_parts = {}
                    self._tweet_parts[total_parts] = {}
                    self._tweet_parts[total_parts][current_part] = tweet_text
                    return f"Received part 1/{total_parts}. Please send the remaining {total_parts-1} parts."
                
                # Subsequent parts
                if not hasattr(self, '_tweet_parts'):
                    return "Error: Please start with part 1/"
                
                # Find the total_parts key that matches our part
                matching_key = None
                for key in self._tweet_parts.keys():
                    if key >= current_part and 1 in self._tweet_parts[key]:
                        matching_key = key
                        break
                
                if not matching_key:
                    return f"Error: Please start with part 1/"
                
                # Store this part
                self._tweet_parts[matching_key][current_part] = tweet_text
                
                # Check if we have all parts
                if len(self._tweet_parts[matching_key]) == matching_key:
                    # We have all parts, post them in order
                    try:
                        tweet_ids = []
                        in_reply_to = None
                        
                        # Sort parts by part number and post in order
                        try:
                            # First try with XClient
                            for part_num in sorted(self._tweet_parts[matching_key].keys()):
                                part_text = self._tweet_parts[matching_key][part_num]
                                draft = DraftPost(caption=part_text)
                                if in_reply_to:
                                    draft.caption = f"{part_text}\n(Replying to previous part)"
                                result = await self.twitter.publish(draft)
                                if not result.success:
                                    raise Exception(f"XClient error: {result.error}")
                                tweet_ids.append(str(result.post_id))
                                in_reply_to = result.post_id
                        except Exception as e:
                            print(f"Error with XClient for multi-part tweet: {e}, falling back to legacy client")
                            # Fallback to legacy client
                            for part_num in sorted(self._tweet_parts[matching_key].keys()):
                                part_text = self._tweet_parts[matching_key][part_num]
                                if in_reply_to:
                                    part_text = f"{part_text} (in reply to previous part)"
                                tweet_id = await self._legacy_twitter.post_tweet(part_text, in_reply_to_tweet_id=in_reply_to)
                                tweet_ids.append(str(tweet_id))
                                in_reply_to = tweet_id
                        
                        # Clean up
                        del self._tweet_parts[matching_key]
                        if not self._tweet_parts:
                            delattr(self, '_tweet_parts')
                        
                        return f"Successfully posted {len(tweet_ids)} tweets with IDs: {', '.join(tweet_ids)}"
                    except Exception as e:
                        # Clean up on error
                        if hasattr(self, '_tweet_parts') and matching_key in self._tweet_parts:
                            del self._tweet_parts[matching_key]
                            if not self._tweet_parts:
                                delattr(self, '_tweet_parts')
                        raise
                
                return f"Received part {current_part}/{matching_key}. Waiting for {matching_key - len(self._tweet_parts[matching_key])} more parts..."
                
            except Exception as e:
                error_msg = f"Error posting to Twitter: {str(e)}"
                print(f"[ERROR] {error_msg}")
                return error_msg

        return FunctionTool(
            name="post_to_twitter",
            description="Post a tweet to Twitter. Args: text (str): The text content to tweet (max 280 chars)",
            func=post_to_twitter
        )

    def _create_telegram_tool(self) -> FunctionTool:
        """Create and return the Telegram messaging tool"""
        async def send_telegram(text: str) -> str:
            """Send a message to the Telegram chat configured in credentials.
            
            Args:
                text (str): The message text to send.
                
            Returns:
                str: Success or error message.
            """
            try:
                # Get chat ID from credentials (already loaded in TelegramClient)
                chat_id = self.telegram.default_chat_id
                
                # Send the message using the chat ID from credentials
                message_id = await self.telegram.send(text, chat_id)
                
                return f"✅ Message sent to Telegram chat (ID: {chat_id})"
                
            except Exception as e:
                return f"Error sending Telegram message: {str(e)}"

        return FunctionTool(
            name="send_telegram",
            description="Send a message to a Telegram chat. Args: chat_id (str): The chat ID, text (str): The message text",
            func=send_telegram
        )

    def _create_linkedin_tool(self) -> FunctionTool:
        """Create and return the LinkedIn posting tool"""
        async def post_to_linkedin(text: str) -> str:
            """Post content to LinkedIn.
            
            Args:
                text (str): The text content to post on LinkedIn
                
            Returns:
                str: Success message with post ID or error message
            """
            if not hasattr(self, 'linkedin') or not self.linkedin:
                return "Error: LinkedIn client is not properly initialized. Please check your credentials."
                
            try:
                # Create post with correct parameters
                post = DraftPost(
                    caption=text,
                    media=[],  # Empty list for text-only posts
                    tags=[]    # Empty list for no tags
                )
                
                # Post to LinkedIn using the pre-initialized client
                result = await self.linkedin.publish(post)
                
                if result and result.success and result.post_id:
                    return f"Successfully posted to LinkedIn. Post ID: {result.post_id}"
                error_msg = result.error if result and hasattr(result, 'error') else "Unknown error"
                return f"Failed to post to LinkedIn: {error_msg}"
                
            except Exception as e:
                return f"Error posting to LinkedIn: {str(e)}"

        return FunctionTool(
            name="post_to_linkedin",
            description="Post content to LinkedIn. Args: text (str): The text content to post",
            func=post_to_linkedin
        )

    async def handle_bus_envelope(self, env: Envelope, redis: AsyncRedis) -> None:
        """Handle incoming message from the bus"""
        try:
            # Get the message content
            message_content = env.content.get("text", "") if hasattr(env, 'content') else str(env)
            print(f"[{self.name}] Received message: {message_content[:100]}...")
            
            # Process the message using round-robin pattern
            response = await self.process_direct_request(message_content)
            
            # Create and send reply envelope if there's a reply_to address
            if hasattr(env, 'reply_to') and env.reply_to:
                reply_envelope = Envelope(
                    role="agent",
                    content={"text": response},
                    reply_to=env.reply_to,
                    correlation_id=getattr(env, 'correlation_id', str(uuid.uuid4())),
                    meta=getattr(env, 'meta', {}),
                    agent_name=self.name,
                    user_id=getattr(env, 'user_id', None)
                )
                await publish_envelope(redis, env.reply_to, reply_envelope)
                print(f"[{self.name}] Sent response to {env.reply_to}")
                
        except Exception as e:
            error_msg = f"Error processing message: {str(e)}"
            print(f"[{self.name}] {error_msg}")
            if hasattr(env, 'reply_to') and env.reply_to:
                error_envelope = Envelope(
                    role="agent",
                    content={"text": f"Error: {error_msg}"},
                    reply_to=env.reply_to,
                    correlation_id=getattr(env, 'correlation_id', str(uuid.uuid4())),
                    agent_name=self.name,
                    user_id=getattr(env, 'user_id', None)
                )
                await publish_envelope(redis, env.reply_to, error_envelope)

    async def _get_user_input(self, prompt: str = None) -> str:
        """Get user input for the conversation"""
        # In a real implementation, this would get input from the user
        # For now, we'll just return a default response
        return "ok"

    async def process_direct_request(self, query_text: str) -> str:
        """Process a direct request using a round-robin group chat pattern"""
        print(f"[{self.name}] Processing query: {query_text[:100]}...")
        
        # Create a queue for user input (using synchronous queue like eth_agent)
        input_queue = queue.Queue()
        input_queue.put(query_text)  # Seed the queue with the initial query
        
        # Track if we've processed a tool call
        tool_called = False
        
        def safe_input(prompt=None):
            try:
                if prompt:
                    print(f"\n{prompt}")
                # Get with a reasonable timeout and handle empty queue case
                try:
                    return input_queue.get(timeout=30)  # 30 second timeout
                except queue.Empty:
                    # Return empty string instead of raising an exception
                    return ""
            except Exception as e:
                print(f"[Input Error] {str(e)}")
                return ""
        
        # Create a UserProxyAgent to handle the conversation
        user_proxy = PatchedUserProxy(
            name="User",
            description="Human via bus",
            input_func=safe_input
        )
        
        print(f"[{self.name}] Starting round-robin conversation for: {query_text}")
        
        # Create the round-robin group chat with a higher max_turns
        group_chat = RoundRobinGroupChat(
            [user_proxy, self],  # User proxy and this agent
            max_turns=10,  # Keep this reasonable to prevent infinite loops
            termination_condition=TextMentionTermination("COMPLETE"),
        )
        
        # Process the stream of messages
        response = []
        stream = group_chat.run_stream(task=None)  # No task, we'll handle the initial message
        
        try:
            # Process the stream of messages
            async for message in stream:
                print(f"[{self.name}] Stream message from {message.source if hasattr(message, 'source') else 'unknown'}: {str(message)[:180]}")
                
                # Only process messages from this agent
                if hasattr(message, 'source') and message.source == self.name:
                    content = message.content if hasattr(message, 'content') else str(message)
                    
                    # Handle tool call responses
                    if isinstance(message, ToolCallSummaryMessage):
                        tool_called = True
                        
                        # Extract tool name from tool_calls list
                        tool_name = 'tool'
                        try:
                            if hasattr(message, 'tool_calls') and message.tool_calls and len(message.tool_calls) > 0:
                                tool_name = message.tool_calls[0].name
                            elif hasattr(message, 'tool_call') and hasattr(message.tool_call, 'function'):
                                tool_name = message.tool_call.function.name
                            elif hasattr(message, 'tool_name'):
                                tool_name = message.tool_name
                            elif hasattr(message, 'name'):
                                tool_name = message.name
                        except Exception as e:
                            print(f"[DEBUG] Error extracting tool name: {e}")
                            print(f"[DEBUG] Message structure: {message}")
                        
                        # Clean up the tool name for display
                        display_name = tool_name.replace('_', ' ').title()
                        
                        # Format the response with consistent styling
                        content = (
                            f"🔧 {display_name}\n"
                            f"{'─' * (len(display_name) + 2)}\n"
                            f"{content}"
                        )
                    
                    # Handle FunctionExecutionResult objects
                    if hasattr(content, 'content') and hasattr(content, 'name'):
                        display_name = content.name.replace('_', ' ').title()
                        content = (
                            f"🔧 {display_name}\n"
                            f"{'─' * (len(display_name) + 2)}\n"
                            f"{content.content}"
                        )
                    # Handle lists
                    elif isinstance(content, list):
                        content = str(content[0].content) if content and hasattr(content[0], 'content') else "No content"
                    # Handle other types
                    else:
                        content = str(content)
                    
                    print(f"[{self.name}] Final content to reply: {content}")
                    #@TODO publish reply
                    response.append(content)
                    
                    # Check for explicit completion
                    if hasattr(content, 'upper') and "COMPLETE" in content.upper():
                        break
        except Exception as e:
            print(f"[{self.name}] Error in message processing: {str(e)}")
            response.append(f"Error: {str(e)}")
        finally:
            # Clean up any resources if needed
            pass
        
        # Join all responses with newlines
        return "\n".join(response) if response else "No response generated"

    def profile(self) -> Dict[str, Any]:
        """Return agent profile for registration"""
        return {
            "name": self.name,
            "description": "Social media management agent with AutoGen integration",
            "capabilities": ["social_media_posting", "conversation"]
        }

    async def start(self):
        """Start the agent"""
        await self.adapter.start()
        print(f"[{self.name}] Agent started and listening...")
        try:
            while True:
                await asyncio.sleep(1)
        except asyncio.CancelledError:
            print(f"[{self.name}] Shutting down...")
            await self.adapter.stop()

async def main():
    # Initialize Redis client
    redis_cfg = get_cred("REDIS_URL", {"url": "redis://localhost:6379/0"})
    redis_url = redis_cfg.get("url") if isinstance(redis_cfg, dict) else redis_cfg
    redis = AsyncRedis.from_url(redis_url, decode_responses=True)
    
    # Initialize Azure OpenAI client with correct parameters
    azure_endpoint = get_cred("AZURE_OPENAI_ENDPOINT")
    if not azure_endpoint:
        raise ValueError("AZURE_OPENAI_ENDPOINT environment variable is required")
    
    model_name = get_cred("AZURE_OPENAI_MODEL", "gpt-4")

    azure_config = {
        "api_key": get_cred("AZURE_OPENAI_KEY"),
        "azure_endpoint": azure_endpoint,  # Changed from api_base to azure_endpoint
        "api_version": get_cred("AZURE_OPENAI_API_VERSION", "2024-02-01"),
        "azure_deployment": get_cred("AZURE_OPENAI_DEPLOYMENT", "o3-mini"),
        "model": model_name,
    }
    
    # Remove any None values from config
    azure_config = {k: v for k, v in azure_config.items() if v is not None}
    
    model_client = AzureOpenAIChatCompletionClient(**azure_config)
    
    # Create and start the agent
    agent = SocialAgent(name="social_agent", redis_client=redis, model_client_instance=model_client)
    await agent.start()

if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print("Social Agent stopped by user")