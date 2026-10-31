# Grok subscription voice

A Grok voice call uses the Grok Bot session already signed in on this computer.
There is no xAI API key. The viewport mints the call; the engine does not.

The mint replays Grok Bot's signed request to
`GrokBotService/MintSandVoiceCallSecret`, then the viewport opens the realtime
socket at `wss://api.x.ai`. The minted secret lasts about 300 seconds. That
expiry is not a hang-up: the socket can outlive it. When the socket drops, mint
again and resume with `conversation_id`. If that resume is unavailable, seed the
recent transcript with `conversation.item.create` and do not send
`response.create`.

The microphone stays open while the assistant speaks, so the caller can
interrupt. When the server reports that the caller started speaking, queued
playback is dropped and an open response is cancelled. There is no acoustic
echo canceller. Headphones keep the speaker out of the microphone.

Audio stays in the viewport (`zeron-audio`). The engine only prepares this
device's Grok orchestrator chat (`PrepareGrokVoice`) and commits final
transcript lines (`AppendGrokVoiceTranscript`). It does not start a harness,
open a WebRTC lease, or forward the call to another computer. Codex voice is
unchanged and still uses the Codex WebRTC helper.
