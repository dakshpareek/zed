# Azure OpenAI Integration Fixes - Decision Log

## ✅ Current Agreed Decisions (Updated: 2025-01-02)

- **Primary Issues Identified**: Response streaming cutoffs and tool recognition failures
- **Root Cause Analysis**: Multiple differences between Azure OpenAI and regular OpenAI implementations
- **Fix Strategy**: Address parsing, error handling, and model capability detection

---

## 📌 Decision History

### 📅 2025-01-02 - Initial Issue Analysis
- **Decision**: Investigate Azure OpenAI vs OpenAI implementation differences
- **Reasoning**: User reports indicate Azure integration has issues while OpenAI works fine
- **Issues Found**:
  1. Missing proper JSON parsing error handling in streaming
  2. Lack of logging for debugging malformed responses
  3. Tool capability detection differences
  4. Potential line buffering issues in stream processing

### 📅 2025-01-02 - Critical Issues Identified  
- **Decision**: Focus on 4 main areas for fixes
- **Reasoning**: Analysis revealed specific patterns causing failures

#### Issue 1: Stream Response Parsing
- **Problem**: Azure OpenAI may send malformed JSON chunks that cause stream termination
- **Location**: `azure_stream_completion` function in `crates/language_models/src/provider/azure_openai.rs:503-519`
- **Evidence**: Missing error handling and logging for JSON parse failures

#### Issue 2: Tool Recognition Logic
- **Problem**: `supports_tools()` always returns `true` regardless of model capabilities  
- **Location**: `crates/language_models/src/provider/azure_openai.rs:414-416`
- **Evidence**: Unlike OpenAI implementation, doesn't check model-specific tool support

#### Issue 3: Missing Parallel Tool Calls Support Check
- **Problem**: Azure doesn't check if model supports parallel tool calls before sending parameter
- **Location**: `convert_to_azure_request` function
- **Evidence**: Regular OpenAI checks `model.supports_parallel_tool_calls()` before setting parameter

#### Issue 4: Insufficient Error Context
- **Problem**: Error messages lack sufficient debugging information for troubleshooting
- **Location**: Multiple error handling sites in Azure implementation
- **Evidence**: Regular OpenAI provides better error context and response body details 