# Media task endpoints

LLMConduit proxies dedicated media APIs to a compatible model backend. Every
creation/transcription request must include a `model` naming a gateway model or
configured alias, even when the backend itself permits omitting it. Routing and
model remapping, provider credentials, inference authorization and session admission
use the existing gateway mechanisms. Unknown media models are rejected rather than
falling back to the first text model. Endpoint support does not load a media model;
prepare its recipe, deploy it through Fleet and configure the model's gateway route.

| Task | Gateway endpoint | Request / response |
| --- | --- | --- |
| Generate images | `POST /v1/images/generations` | JSON / JSON or image bytes |
| Edit images | `POST /v1/images/edits` | Multipart / JSON or SSE |
| Generate speech | `POST /v1/audio/speech` | JSON / audio bytes or SSE |
| Generate sound/music | `POST /v1/audio/generate` | JSON / audio bytes |
| Transcribe audio | `POST /v1/audio/transcriptions` | Multipart / backend text, JSON or SSE |
| Translate audio to text | `POST /v1/audio/translations` | Multipart / backend text or JSON |
| Generate video | `POST /v1/videos` | Multipart / asynchronous job JSON |
| Generate video synchronously | `POST /v1/videos/sync` | Multipart / video bytes |
| Poll a video | `GET /v1/videos/{id}` | Job JSON |
| Download a video | `GET /v1/videos/{id}/content` | Video bytes |
| Delete/cancel a video | `DELETE /v1/videos/{id}` | Backend deletion JSON |

Vendor fields, uploaded file bytes, response content types and streaming bodies are
preserved. Multipart parsing rejects duplicate/non-textual `model` fields; alias
rewriting replaces only that model field. The configured request-body limit applies
to uploads; increase it explicitly when larger media files are required. Backend
and client timeouts must suit the generation duration. Unsupported tasks return the
backend's error; there is no chat-to-speech or chat-to-video adapter.

Video creation requires the SQL store. The gateway returns an opaque job ID and
persists its original backend, upstream job ID, requested model and owner. Poll,
download and delete use that same backend and recheck current inference permissions.
Another principal/key receives 404 for a private job. Controller restarts retain job
routing. Backend jobs/content themselves must survive backend restarts if needed;
the gateway does not copy generated video files. Async creation is not retried on
another backend after an ambiguous failure. Job metadata is bounded to 2,048 records;
deleting jobs frees records. Listing across backend job stores and realtime media
WebSockets are not implemented.

Access policies can select `images`, `image_edits`, `videos`, `audio_speech`,
`audio_generate`, `audio_transcriptions` and `audio_translations` in the existing
policy editor. They also constrain requested/served models and provider candidates.
Media and raw embedding/rerank/completion requests add body-free request history,
including terminal status and serving model/provider. Binary input/output is not
added to durable history by these handlers. Usage/token counts remain absent when
the backend does not report them. Dedicated media endpoints are independent of
Responses `image_generation` tool handling, which remains unsupported.

The wire paths follow [vLLM-Omni's task API overview](https://docs.vllm.ai/projects/vllm-omni/en/latest/serving/)
and [video API](https://docs.vllm.ai/projects/vllm-omni/en/latest/serving/videos_api/).
Transcription/translation require a backend implementing the standard OpenAI audio
parsing APIs. Our Qwen-Image, MiniMax-H3 and AuK source recipes describe compatible
image, video and speech deployments; importing a source does not start it.
