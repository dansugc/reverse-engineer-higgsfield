# Format catalog (what creators make on Higgsfield, and how to rebuild it on fal)

Use these as lenses, not templates. Name the closest one in `higgsfield_guess.format`.

- **Before/after transformation** (fitness, skincare, glow-up, room makeover): 2 shots, same character/place, hard cut, caption like "watch me lock in for 3 months". Flow: t2i character → image_edit "after" state (keeps identity) → i2v per shot with the same motion (slow turn, mirror check) → captions. Identity consistency is everything.
- **AI UGC talking head / testimonial**: one creator to camera, handheld phone feel, lip-sync. Flow: t2i creator → i2v with speaking motion (or a lip-sync model on a TTS track — mark as unknown/lipsync). Captions burned in.
- **Product showcase / packshot**: product in hand or on surface, orbit or push-in. Flow: t2i or image_edit of a product photo → i2v with camera orbit/push.
- **Camera-preset shots** (Higgsfield's signature: crash zoom, dolly zoom, 360 orbit, bullet time, FPV, snorricam, crane up, whip pan): describe the move precisely in the video prompt ("fast crash zoom into the face", "camera orbits 180° around the subject at chest height, subject stays still").
- **AI influencer lifestyle / day-in-the-life**: 3–6 shots, same character, varied settings. Flow: one character t2i → image_edit per setting → i2v per shot.
- **POV / skit**: 2–4 shots with reaction beats and text. Keep the character consistent; prompts carry the acting beat.
- **Viral effects** (earth zoom-out, melt, explode, morph, "turn into"): usually one i2v shot with a strong transformation prompt, sometimes start+end frames (`params.end_image_url` via an image_edit end frame).

Pacing rules of thumb: shots 2–8s; hook in the first second (a motion or a caption); vertical 9:16 for TikTok/Reels/Shorts.
