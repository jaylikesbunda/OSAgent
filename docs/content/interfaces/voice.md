---
title: "Voice"
description: "Talk to OSAgent with Whisper STT and Piper TTS."
weight: 30
toc: true
---

## What this does

Voice input is transcribed with Whisper (STT) and replies can be spoken back
with Piper (TTS). Configure providers and models under `[voice]`
(`stt_provider`, `tts_provider`, `whisper_model`, `piper_voice`).

## Formats

OGG/Opus, MP3, M4A, FLAC, and friends are decoded in-process — Whisper always
receives 16-bit WAV. Discord voice messages and audio attachments are decoded
the same way (requires a Discord-enabled build).

## Related tasks

- [Web UI]({{< relref "web-ui.md" >}})
- [Discord bot]({{< relref "discord.md" >}})
