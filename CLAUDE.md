# Project overview 
remix-agent-runtime is one piece of a three-part product strategy built around [remix-browser](https://github.com/hkd987/remix-browser), a Rust-native MCP server for headless Chrome automation via CDP.

## Web Searches
 - Never include the date or year in your web searches, its weird and doesn't help

## Work as a team
When asked to do work always spin up multiple agents and work as a team to get the job done as fast as possible. High quality code written fast as a team is the goal here. Working together and sharing when needed.

 ## Code Quality
 - Always unit test code that we write ALWAYS.
 - Always ensure the code is linted and or type checked
 - Always ensure the project builds
 - Always ensure the code is DRY
 - Always ensure you never use an `Any` type, we will always use high quality types in our code

 ## Code Reuse Rules (prevent recurring audit findings)
 - **Search for existing utilities before writing new ones.** Before implementing truncation, path resolution, string parsing, ANSI stripping, or output filtering — check `src/local_tools/tools/output_filter.rs` and other shared modules first. Duplicate utility code is a DRY violation.
 - **Use the correct `AgentError` variant.** The enum is in `src/error.rs` — read it before using error types. Common mistake: using a non-existent `AgentError::Tool()` instead of `AgentError::ToolExecution()`.
 - **Extract shared patterns across sibling files.** When two files in the same module (e.g., two tool handlers) share 10+ lines of identical logic, extract a helper function immediately — don't leave it for a cleanup pass.
 - **Make struct fields derived when possible.** If a struct field can be computed from other fields (e.g., summary counts from a Vec of results), use computed methods instead of storing redundant state. This prevents data inconsistency.
 - **Shared argument/parameter building.** When multiple tools need framework-specific argument construction (e.g., test filters), put the logic on the enum/type itself (`impl TestFramework { fn apply_filters(...) }`) rather than duplicating match blocks in each call site.
 - **Strip ANSI codes from subprocess output.** Always apply `strip_ansi()` to stdout/stderr from child processes before returning to the agent — raw terminal escape codes waste context window tokens.
 - **Use typed deserialization over stringly-typed JSON.** When working with protocols like LSP, prefer deserializing into typed structs (e.g., `lsp_types::*`) over manual `.get("field")` chains where feasible.
 - **Validate threshold ordering in multi-phase configs.** When a config has ordered thresholds (e.g., `planning_threshold < verification_threshold` in `ReasoningStagesConfig`), always add a `validate_config()` function and call it at startup. Inverted thresholds silently produce wrong behavior (unreachable phases) without validation.
 - **Use `..default_config()` not `..Default::default()` in tests.** Test modules that define a `default_config()` helper should use it consistently for all test AgentConfig construction to prevent missing new fields when the struct grows.
 
## When Planning or testing
 - Always see how you can validate a change you have made to ensure its correct
     - Examples
         - When asked to optimize code or make code faster, always have a performance benchmark you can run before and after
         - When asked to write a new feature, or extend code write supporting unit test if needed first then add the new feature then add more unit test as needed
 - If you are unsure about an ask, always use the AskUserQuestion tool and get the answers your need