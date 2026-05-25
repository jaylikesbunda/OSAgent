v0.1.1 changes:

- Added missing context overflow detection patterns (Mistral, Ollama, z.ai, vLLM, llama.cpp, LM Studio, MiniMax, Kimi, Copilot, Bedrock, HTTP 413)
- Added is_openai_retryable() for 404 retry handling (OpenAI sometimes returns 404 for available models)
- Added schema transforms: Kimi/Moonshot ($ref sibling stripping, tuple items fix) and Google/Gemini (integer enum-to-string, required sanitization, non-object type cleanup)
- Added DeepSeek reasoning part fix (ensure all assistant messages have reasoning_content field)
- Added GPT-5 text_verbosity: "low" default option for non-Codex models
- Added GitHub Copilot store: false default option
- Added Google Vertex ADC auth token support (GCLOUD_ACCESS_TOKEN env var)
- Added AWS Bedrock profile-based credential loading (~/.aws/credentials)
- Added AWS Bedrock cross-region inference prefix support
- Added Azure env var fallback chain (AZURE_OPENAI_KEY / AZURE_OPENAI_API_KEY, AZURE_RESOURCE_NAME)
- Fixed OpenRouter header duplication (removed from provider_transforms, kept in provider_auth with consistent values)
- Added GCP service account file detection (GOOGLE_APPLICATION_CREDENTIALS)

v0.1.0-rc2 changes:

- Added discord bot setup to launcher
- Added file read before caching
- Added subagent coordinator
- Improved internal prompt with automated evaluation and testing
- Added read before write check
- Replaced completion heuristic with finish_reason based logic\
- Expanded parallel safe tools list
- Added updater logic
- Support for workspaces with multiple allowed dirs