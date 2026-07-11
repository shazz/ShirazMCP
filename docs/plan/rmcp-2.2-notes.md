# rmcp 2.2.0 API notes (verified against installed crate source)

Authoritative reference for P0+. Verified by reading
`~/.cargo/registry/src/index.crates.io-*/rmcp-2.2.0/src`. The key gotcha that must
not recur: **`Implementation`, `ServerInfo`/`InitializeResult`, and
`ServerCapabilities` are `#[non_exhaustive]` — never use struct literals. Use the
constructors/builders below.**

## Construction paths (the non_exhaustive workarounds)

```rust
// Implementation (server identity)
Implementation::new("shiraz-mcp", env!("CARGO_PKG_VERSION"))   // name, version
Implementation::from_build_env()                                // CARGO_CRATE_NAME + CARGO_PKG_VERSION
// builder: .with_title(..), .with_description(..)

// ServerInfo == InitializeResult
pub type ServerInfo = InitializeResult;
InitializeResult::new(capabilities)          // -> Self, protocol_version::default(), server_info from build env
    .with_server_info(Implementation::new("shiraz-mcp", env!("CARGO_PKG_VERSION")))
    .with_instructions("Drive Hatari (Atari ST emulator): debug, screenshot, rip.")

// ServerCapabilities
ServerCapabilities::builder().enable_tools().build()
```

## Tool registration — the macro pattern (modern rmcp)

Macros: `#[tool_router]`, `#[tool]`, `#[tool_handler]` (from `rmcp` re-exporting
`rmcp-macros`). Wrappers: `rmcp::handler::server::wrapper::{Parameters, Json}`,
`rmcp::handler::server::tool::ToolRouter`.

```rust
use rmcp::{tool, tool_router, ErrorData as McpError,
           handler::server::wrapper::{Parameters, Json}};

#[derive(Clone)]
struct HatariServer { /* Arc<Mutex<AnySession>>, config, ... */ }

#[tool_router(server_handler)]   // `server_handler` auto-emits #[tool_handler] for a tools-only server
impl HatariServer {
    #[tool(name = "status", description = "Report emulator session state.")]
    async fn status(&self) -> Result<Json<StatusOutput>, McpError> {
        // impl: read AnySession state, return Json(StatusOutput { .. })
    }
    // a tool WITH input:
    // #[tool(name="read_memory", description="...")]
    // async fn read_memory(&self, Parameters(args): Parameters<ReadMemArgs>)
    //     -> Result<Json<..>, McpError> { .. }
}
```

- Input structs derive `serde::Deserialize` + `schemars::JsonSchema`; passed as
  `Parameters<T>`. A no-input tool just takes `&self`.
- Output: `Json<T>` (T: `Serialize + JsonSchema`) or a `CallToolResult`. For images
  (screenshots), return a `CallToolResult` with an image content block.
- `#[tool_router(server_handler)]` generates the `ToolRouter` and the
  `ServerHandler` tool plumbing. Provide `get_info` yourself (below).

### Gotcha (verified in P0): `#[tool_router]` generates a PRIVATE fn

If you use the **explicit** split — `#[tool_router]` on the tool `impl` in one
module and `#[tool_handler] impl ServerHandler` in the parent module (needed when
you want a custom `get_info`) — the macro's generated `tool_router()` associated fn
is **private to the module it's declared in**, so the parent `#[tool_handler]`
fails with `E0624: associated function tool_router is private`. Fix: widen it with
`#[tool_router(vis = "pub(crate)")]` (the macro accepts a `vis` arg; verified in
`rmcp-macros-2.2.0/src/tool_router.rs`). No type/signature change needed.

## get_info (advertise capabilities + instructions)

If not using the `server_handler` auto-impl's default, implement:

```rust
impl rmcp::ServerHandler for HatariServer {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::ServerInfo::new(
            rmcp::model::ServerCapabilities::builder().enable_tools().build(),
        )
        .with_server_info(rmcp::model::Implementation::new(
            "shiraz-mcp", env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions("Drive Hatari: debug / screenshot / memory / rip.")
    }
}
```

## Serving over stdio

```rust
use rmcp::{ServiceExt, transport::io::stdio};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let running = HatariServer::new(/* .. */).serve(stdio()).await?;
    running.waiting().await?;   // -> Result<QuitReason, JoinError>
    Ok(())
}
```

- `stdio()` = `rmcp::transport::io::stdio()` → `(tokio::io::Stdin, tokio::io::Stdout)`.
- `.serve(..)` comes from `ServiceExt` (`use rmcp::ServiceExt;`).
- `RunningService::waiting()` blocks until the peer disconnects.

## Cargo features needed

`rmcp = { version = "2.2", features = ["server", "transport-io", "macros"] }`
(`macros` gives the `#[tool*]` attrs; `transport-io` gives `stdio()`; `server` the
handler traits). Also `schemars` for `JsonSchema` derives on tool I/O structs
(rmcp has a `schemars` feature, or depend on `schemars` directly — confirm one path
during P0 and pin it).

## Error type

Tool methods return `Result<_, rmcp::ErrorData>` (aliased `McpError`). Map internal
`crate::Error` → `McpError` at the tool boundary (generic message + code; no internal
detail leakage).
