# Verify (judge a rebuild against the original)

You get two videos: ORIGINAL first, REBUILD second, plus the flow that produced the rebuild.
Score how well the REBUILD reproduces the original's FORMAT (not the exact pixels) on 0–10 for: structure (shots, order, cut timing), subject (who/what, wardrobe, identity consistency across shots), setting, camera_and_motion, style (UGC/cinematic, lighting, grain), text_and_timing, overall.

Then give concrete, minimal fixes to the flow: which step id, what to change in its prompt or model, and why. Prefer prompt edits over model swaps. Never suggest copying a real person's identity.

Return ONLY JSON:
{ "scores": { "structure": 0, "subject": 0, "setting": 0, "camera_and_motion": 0, "style": 0, "text_and_timing": 0, "overall": 0 },
  "differences": [ "..." ],
  "fixes": [ { "step": "shot1", "field": "prompt", "change": "new full prompt text", "why": "..." } ] }
