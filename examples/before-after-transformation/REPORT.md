# Fitness Before/After Transformation

**Format:** A two-shot before-and-after sequence showing a physique transformation, driven by a hard cut and a trending audio track.

`13.0s` · `720x1280` · `9:16` · 2 shot(s) · harness `9b22a9e2c4d8`

## Shots

| # | Time | What happens | Camera | Text |
|---|---|---|---|---|
| 1 | 0.00–7.07s | Subject stands in a bathroom, looking relaxed, and slowly turns around. | Static, eye-level, UGC phone style | {{caption_text}} |
| 2 | 7.07–13.03s | Subject, now toned, gives a thumbs up and turns to show back muscles. | Static, eye-level, UGC phone style | none |

## The fal flow

- **base_image_before** · `TextToImage` · `fal-ai/nano-banana-pro`
  > Vertical iPhone photo, side profile, full torso to mid-thigh in frame. {{character}}. Her body is {{before_state}}. She stands in {{setting_before}}. Warm dim bathroom light, slight grain, casual UGC mirror-check look, no text.
- **base_image_after** · `ImageEdit` · `fal-ai/nano-banana-pro/edit` ← base_image_before
  > Same woman: keep her exact face, hair bun, black sports bra and black shorts, same side-profile camera framing. She is now {{after_state}}. Make the transformation dramatic and obvious at a glance — do NOT keep any of the old belly, love handles or soft arms. Move her to {{setting_after}}. Bright overhead bathroom light, vertical iPhone UGC look, no text.
- **shot1** · `ImageToVideo` · `fal-ai/kling-video/v3/pro/image-to-video` ← base_image_before · 7.1s
  > She slowly turns from side profile toward the camera and back, letting her soft belly and arms show. Static handheld phone camera, natural pace. Her body shape stays exactly the same the whole shot.
- **shot2** · `ImageToVideo` · `fal-ai/kling-video/v3/pro/image-to-video` ← base_image_after · 6.0s
  > She gives two thumbs up, then turns sideways and stretches both arms straight out in front, showing her flat stomach and lean arms. Static handheld phone camera. Her lean body shape stays exactly the same the whole shot.

**Edit:** shot1 (7.07s) → shot2 (5.96s)
- caption "{{caption_text}}" 0.0–7.1s (center)
- audio: music Upbeat hip-hop/rap trending audio track.

## Variables (scale the format)

- `{{after_state}}` = "about 15 kg lighter after 3 months of training: much narrower waist, completely flat stomach with visible ab lines and obliques, lean toned arms with defined shoulders, slimmer face and sharper jawline, upright confident posture" — Every visible change for the 'after' edit (written as a delta).
- `{{before_state}}` = "visibly out of shape and carrying roughly 15 kg of extra weight: a soft rounded lower belly that spills slightly over the waistband, soft love handles, thick soft upper arms, fuller round face and soft jawline, no muscle definition anywhere, relaxed slightly slouched posture" — Visible 'before' body — concrete, so the model doesn't default to a fit person.
- `{{caption_text}}` = "watch me lock in for 3 months" — Hook text displayed in the first shot.
- `{{character}}` = "East Asian woman in her mid 20s, dark hair in a messy high bun, black sports bra and loose black shorts, skin shiny with sweat" — Base description of the person and outfit.
- `{{setting_after}}` = "bathroom with glass shower door and white subway tiles" — Background for the second shot.
- `{{setting_before}}` = "bathroom with shower curtain and wooden shelves" — Background for the first shot.

## Contrast check

The first draft said "slightly soft physique" and the after edit came out almost identical. The before/after prompts now state every visible change (see `analyst.md` rule 5). Test stills from this flow (AI-generated, ~$0.30):

![before and after stills](before-after.jpg)

## Evidence

- _observed_: 13.03s, 720x1280 (9:16), 30 fps (ffprobe)
- _observed_: 2 shot(s): 0.00–7.07s, 7.07–13.03s (ffmpeg scene detection (threshold 0.3))
- _observed_: audio track: yes (mean -19.3 dB) (ffprobe / volumedetect)
- _observed_: TikTok re-encode (aigc_info / vid tags): original generator metadata was stripped (container tags / byte scan)
- _observed_: encoder: Lavf58.76.100 (container tags / byte scan)
- _observed_: no C2PA / generator manifest found (container tags / byte scan)
- _inferred_: AI generation (shot 1 / 0:04 (morphing shower curtain during turn))
- _inferred_: Before/after format (shot 2 / 7.06s (hard cut to toned physique))
- _inferred_: Likely format: Before/after transformation (tools: Kling or Minimax for video, Midjourney/Flux for base images; confidence high) (Classic 2-shot structure with identity consistency but distinct physique changes. Slight AI morphing on hands and background details.)

## Unknowns

- Exact generation prompts
- Original seed values
- Name of the specific audio track used
- Which model Higgsfield used for each shot (inferred from visuals only)

## Next

```bash
reh plan examples/before-after-transformation/flow.json                 # cost estimate (~$2.26 per run, rough)
reh build examples/before-after-transformation/flow.json --max-usd 5    # rebuild on fal
reh batch examples/before-after-transformation/flow.json --vars variants.jsonl --max-usd 20
reh publish examples/before-after-transformation/flow.json              # create it as a fal Workflow
```
