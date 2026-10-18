# Security

- **Keys:** `FAL_KEY` is read from the environment or `.env` (gitignored) and sent only to fal (`queue.fal.run`, `api.fal.ai`) as an `Authorization` header. It is never written to reports, flows, build receipts or logs.
- **Spending:** every build computes the cost first (fal pricing API) and refuses to run over `--max-usd`. The CLI asks before spending; the MCP `build_flow` tool requires `confirm: true`.
- **Your video:** analysis sends the video to fal (as a data URI) for the video-model reading. `--no-ai` keeps everything local.
- **Web app:** binds `127.0.0.1`, rejects foreign `Host` headers and cross-origin writes, and serves files only from inside its own run folders (no path traversal).
- **Subprocesses:** ffmpeg/ffprobe are run with argument arrays (no shell); captions are passed via text files, not interpolated into filter strings.

Report vulnerabilities via a private GitHub security advisory.
