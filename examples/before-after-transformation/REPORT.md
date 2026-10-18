# Probe-only skeleton

**Format:** 2 shot(s), 13.0s, 9:16 — fill the variables from the keyframes or re-run with FAL_KEY

`13.0s` · `720x1280` · `9:16` · 2 shot(s) · harness `9b22a9e2c4d8`

## The fal flow

- **character** · `TextToImage` · `fal-ai/nano-banana-pro`
  > {{subject}} in {{setting}}, {{style}}
- **shot1** · `ImageToVideo` · `fal-ai/kling-video/v3/pro/image-to-video` ← character · 7.1s
  > {{shot1_motion}}
- **shot2** · `ImageToVideo` · `fal-ai/kling-video/v3/pro/image-to-video` ← character · 6.0s
  > {{shot2_motion}}

**Edit:** shot1 (7.07s) → shot2 (5.96s)
- audio: music 

## Variables (scale the format)

- `{{setting}}` = "a bright room" — where it happens
- `{{shot1_motion}}` = "subtle natural motion, static camera" — subject + camera motion in shot 1
- `{{shot2_motion}}` = "subtle natural motion, static camera" — subject + camera motion in shot 2
- `{{style}}` = "vertical iPhone UGC footage, natural light" — look and feel
- `{{subject}}` = "a person" — who/what is on screen

## Evidence

- _observed_: 13.03s, 720x1280 (9:16), 30 fps (ffprobe)
- _observed_: 2 shot(s): 0.00–7.07s, 7.07–13.03s (ffmpeg scene detection (threshold 0.3))
- _observed_: audio track: yes (mean -19.3 dB) (ffprobe / volumedetect)
- _observed_: TikTok re-encode (aigc_info / vid tags): original generator metadata was stripped (container tags / byte scan)
- _observed_: encoder: Lavf58.76.100 (container tags / byte scan)
- _observed_: no C2PA / generator manifest found (container tags / byte scan)

## Unknowns

- Everything visual: no video model ran (set FAL_KEY)

## Next

```bash
reh plan reh-runs/demo/flow.json                 # cost estimate (~$2.11 per run, rough)
reh build reh-runs/demo/flow.json --max-usd 5    # rebuild on fal
reh batch reh-runs/demo/flow.json --vars variants.jsonl --max-usd 20
reh publish reh-runs/demo/flow.json              # create it as a fal Workflow
```
