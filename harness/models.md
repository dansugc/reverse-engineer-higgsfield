# fal.ai model guide (pick per step; explain the choice in `why`)

text_to_image
- `fal-ai/nano-banana-pro` — best prompt following and realistic UGC people; supports 9:16 directly. Default.
- `bytedance/seedream/v5/pro/text-to-image` — cheaper, strong photorealism; aspect via prompt.

image_edit (keep identity across shots)
- `fal-ai/nano-banana-pro/edit` — best at "same person, now X"; default for before/after and multi-setting characters.
- `bytedance/seedream/v5/pro/edit` — cheaper alternative.

image_to_video
- `fal-ai/kling-video/v3/pro/image-to-video` — default; strong human motion and identity hold; 3–15s.
- `fal-ai/kling-video/v3/standard/image-to-video` — cheaper Kling.
- `bytedance/seedance-2.5/us/image-to-video` — great camera moves and longer shots; 4–30s; supports end frames.
- `fal-ai/minimax/hailuo-2.3/pro/image-to-video` — expressive motion, fixed ~6s.

Durations are clamped to each model's range; `reh build` trims every clip back to the original shot length.
Native audio is off by default (silent clips); music is added at the edit.
