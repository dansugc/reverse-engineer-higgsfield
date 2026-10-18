---
name: reverse-engineer-higgsfield
description: Reverse engineer an AI-generated video (Higgsfield, Kling, Seedance, Veo, Hailuo…) into a fal.ai flow that recreates the format, then rebuild and scale it with REH. Use when someone shares an AI video and asks how it was made, wants to recreate a format on fal, or wants many variations of it.
---

# REH

1. `analyze_video({ path })` (or `reh analyze video.mp4`). Read the REPORT: shots are measured, readings are inferred, prompts/seed/model are unknown.
2. Show the flow and `plan_flow` cost. Edit prompts/variables if the user wants changes (`get_flow`).
3. Only after the user agrees on a budget: `build_flow({ flow_path, max_usd, confirm: true })`.
4. `verify_rebuild({ original, rebuild, flow_path, apply: true })` → explain scores and fixes; rebuild with flow.v2.json only with the user's go.
5. To scale: propose variable sets (new characters/products/captions), show total cost, then build each.

Rules: rebuild formats, never a real person's identity; label AI content; music must be the user's own licensed track; never claim the original prompt or model as fact.
