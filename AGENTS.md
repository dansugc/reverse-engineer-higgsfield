# Repository guidelines

REH turns an AI video into a fal.ai flow. One Rust binary: CLI (`src/main.rs`), web (`src/web.rs`), MCP (`src/mcp.rs`).

- **Evidence contract:** `probe.rs` produces only measured facts. Model readings are `kind: "inferred"`. Anything not knowable (prompt, seed, model) goes in `unknowns`. Never present an inference as fact.
- **Flow is the contract:** `flow.rs` owns the schema, validation, per-model input adapters (`build_input`), cost estimates and the fal Workflow export. Add a model = add a `ModelInfo` + an adapter arm + a test.
- **Spending is gated:** anything that calls a generation endpoint goes through `build::build`, which checks `--max-usd` first.
- **Harness:** prompts live in `harness/*.md` and are compiled in (`include_str!`). Their SHA is stamped on reports.
- `cargo test` must pass (includes a real-ffmpeg assembly test). No `unsafe`, no shelling out through a shell.
